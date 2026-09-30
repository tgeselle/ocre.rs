//! `ocre generate model|scaffold|api|migration|mailer|mailbox|job|schedule|cache|locale|ci|pwa|...`.
//!
//! Generators collect every change in [`Edits`] and write nothing until the
//! whole generation succeeded, so a failure never leaves half a resource.
//! [`Edits::apply`] honours `--pretend`, `--force` and `--skip` and writes a
//! [`record`] of the run for `ocre destroy`.

mod api;
mod auth;
mod cache;
mod ci;
mod controller;
mod custom;
mod data;
mod fields;
mod job;
mod locale;
mod mailbox;
mod mailer;
mod migration;
mod model;
mod pwa;
mod realtime;
pub(crate) mod record;
mod scaffold;
mod schedule;
pub(crate) mod storage;
pub(crate) mod system_test;
mod templates;
mod test_files;

use std::{fmt::Write as _, path::PathBuf};

pub use api::api;
pub use auth::{AuthOptions, auth};
pub use cache::cache;
pub use ci::ci;
pub use controller::{controller, resource};
pub use custom::{custom, generator};
pub use data::data;
pub use job::job;
pub use locale::locale;
pub use mailbox::mailbox;
pub use mailer::mailer;
pub use migration::migration;
pub use model::model;
pub use pwa::pwa;
pub use scaffold::scaffold;
pub use schedule::schedule;
pub use system_test::system_test;
pub use templates::{TemplateInfo, override_templates};

use crate::{
    config::{self, Config},
    output::{CliError, Report},
    project::Project,
};

pub(crate) const MODULES_MARKER: &str = "// ocre:modules";
pub(crate) const ROUTES_MARKER: &str = "// ocre:routes";

/// Flags shared by every generator.
#[derive(Clone, Debug, Default)]
pub struct GenerateOptions {
    /// `--pretend`: report the changes, write nothing.
    pub pretend: bool,
    /// What to do with a file the generator creates when it already exists.
    pub existing: Existing,
    /// The command line after `ocre`, kept in the generation record.
    pub invocation: Vec<String>,
}

/// A generated file that already exists: fail (default), `--force`
/// overwrite it, or `--skip` keep it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Existing {
    #[default]
    Fail,
    Force,
    Skip,
}

/// Pending file changes, relative to the app root.
pub(crate) struct Edits<'a> {
    project: &'a Project,
    /// (path, contents, whether the file existed before)
    files: Vec<(String, String, bool)>,
    /// Existing files kept by `--skip`.
    skipped: Vec<String>,
}

impl<'a> Edits<'a> {
    pub fn new(project: &'a Project) -> Self {
        Self { project, files: Vec::new(), skipped: Vec::new() }
    }

    fn full(&self, path: &str) -> PathBuf {
        self.project.root.join(path)
    }

    /// Whether the file exists, on disk or among the pending creations.
    pub fn exists(&self, path: &str) -> bool {
        self.files.iter().any(|(p, _, _)| p == path) || self.full(path).exists()
    }

    /// Current contents: pending edits first, then the disk.
    pub fn read(&self, path: &str) -> Result<Option<String>, CliError> {
        if let Some((_, contents, _)) = self.files.iter().find(|(p, _, _)| p == path) {
            return Ok(Some(contents.clone()));
        }
        match std::fs::read_to_string(self.full(path)) {
            Ok(contents) => Ok(Some(contents)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Adds a new file. An existing file is an error, unless `--force`
    /// (overwrite it) or `--skip` (keep it) was given.
    pub fn create(&mut self, path: &str, contents: String) -> Result<(), CliError> {
        if self.exists(path) {
            let pending = self.files.iter().any(|(p, _, _)| p == path);
            match self.project.generate.existing {
                Existing::Force if !pending => self.update(path, contents),
                Existing::Skip if !pending => self.skipped.push(path.to_owned()),
                _ => {
                    return Err(CliError::new(format!("{path} already exists")).hint(
                        "generators create new files only: pass --skip to keep the existing file, --force to overwrite it, or edit it (`ocre g migration` for tables)",
                    ));
                }
            }
            return Ok(());
        }
        self.files.push((path.to_owned(), contents, false));
        Ok(())
    }

    /// Replaces the contents of a file (existing or created earlier).
    pub fn update(&mut self, path: &str, contents: String) {
        match self.files.iter_mut().find(|(p, _, _)| p == path) {
            Some(entry) => entry.1 = contents,
            None => {
                let existed = self.full(path).exists();
                self.files.push((path.to_owned(), contents, existed));
            }
        }
    }

    /// Whether `migrations/` already has a `*_create_<table>.sql`.
    pub fn has_create_migration(&self, table: &str) -> Result<bool, CliError> {
        let suffix = format!("_create_{table}.sql");
        Ok(self.migration_names()?.iter().any(|name| name.ends_with(&suffix)))
    }

    fn migration_names(&self) -> Result<Vec<String>, CliError> {
        let mut names: Vec<String> =
            self.files.iter().filter_map(|(p, _, _)| p.strip_prefix("migrations/").map(str::to_owned)).collect();
        let dir = self.full("migrations");
        if dir.is_dir() {
            for entry in std::fs::read_dir(dir)? {
                names.push(entry?.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(names)
    }

    /// Writes everything (nothing with `--pretend`) and reports it, in the
    /// order the changes were made; records the run for `ocre destroy`.
    pub fn apply(self, command: &'static str) -> Result<Report, CliError> {
        let options = &self.project.generate;
        let mut report = Report::new(command);
        let mut record = record::Record::new(command, &options.invocation);
        for (path, contents, existed) in self.files {
            let full = self.project.root.join(&path);
            // Generated Rust goes through rustfmt, so `cargo fmt --check` (`ocre ci`) passes.
            let contents = if path.ends_with(".rs") { rustfmt(&self.project.root, contents) } else { contents };
            if existed {
                let old = std::fs::read_to_string(&full)?;
                if old == contents {
                    continue;
                }
                record.updated(&path, &old, &contents);
                report.updated.push(path);
            } else {
                record.created(&path, &contents);
                report.created.push(path);
            }
            if !options.pretend {
                std::fs::create_dir_all(full.parent().expect("app files have a parent"))?;
                std::fs::write(&full, contents)?;
            }
        }
        report.skipped = self.skipped;
        report.pretend = options.pretend;
        if !options.pretend && !record.is_empty() {
            record.save(&self.project.root)?;
        }
        Ok(report)
    }
}

/// `contents` as rustfmt formats it (with the app's rustfmt.toml); unchanged
/// when rustfmt is missing or refuses it.
fn rustfmt(root: &std::path::Path, contents: String) -> String {
    use std::{
        io::Write as _,
        process::{Command, Stdio},
    };
    let child = Command::new("rustfmt")
        .args(["--edition", "2024"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return contents };
    let written = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(contents.as_bytes()).is_ok());
    match child.wait_with_output() {
        Ok(output) if written && output.status.success() => String::from_utf8(output.stdout).unwrap_or(contents),
        _ => contents,
    }
}

/// `migrations/NNNN_<name>.sql`, numbered after the highest existing migration.
pub(crate) fn next_migration_path(edits: &Edits, name: &str) -> Result<String, CliError> {
    let highest = edits
        .migration_names()?
        .iter()
        .filter_map(|file| file.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(format!("migrations/{:04}_{name}.sql", highest + 1))
}

/// cloudflare.config.ts as it will be: pending edits first, then the disk.
pub(crate) fn read_config(edits: &Edits) -> Result<Config, CliError> {
    Config::parse(edits.read(config::FILE)?.unwrap_or_default())
}

/// Inserts `line` after the line holding `marker`, with the marker's
/// indentation. `None` when the marker is missing; the text unchanged when
/// it already has those lines (a generator run again with `--force`).
pub(crate) fn insert_after_marker(text: &str, marker: &str, line: &str) -> Option<String> {
    let block: Vec<&str> = line.lines().map(str::trim).collect();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    if lines.contains(&marker) && lines.windows(block.len().max(1)).any(|window| window == block) {
        return Some(text.to_owned());
    }
    let mut out = String::with_capacity(text.len() + line.len() + 8);
    let mut found = false;
    for current in text.lines() {
        out.push_str(current);
        out.push('\n');
        if !found && current.trim() == marker {
            found = true;
            let indent = &current[..current.len() - current.trim_start().len()];
            for inserted in line.lines() {
                if !inserted.is_empty() && !inserted.starts_with(indent) {
                    out.push_str(indent);
                }
                out.push_str(inserted);
                out.push('\n');
            }
        }
    }
    found.then_some(out)
}

/// Adds `mod <module>;` and `.merge(<module>::routes())` to src/lib.rs.
pub(crate) fn register_routes(edits: &mut Edits, module: &str) -> Result<(), CliError> {
    let missing = || {
        CliError::new("src/lib.rs is missing the `// ocre:modules` or `// ocre:routes` marker").hint(
            "put `// ocre:modules` on its own line where `mod` declarations go, and `// ocre:routes` inside the `Router::new()` chain",
        )
    };
    let lib = edits.read("src/lib.rs")?.ok_or_else(missing)?;
    let lib = insert_after_marker(&lib, MODULES_MARKER, &format!("mod {module};")).ok_or_else(missing)?;
    let lib = insert_after_marker(&lib, ROUTES_MARKER, &format!(".merge({module}::routes())")).ok_or_else(missing)?;
    edits.update("src/lib.rs", lib);
    Ok(())
}

/// Turns on Ocre's `feature` in the one-line `ocre = { ... }` dependency of
/// Cargo.toml; unchanged when it is already on.
pub(crate) fn with_ocre_feature(cargo_toml: &str, feature: &str) -> Result<String, CliError> {
    let quoted = format!("\"{feature}\"");
    let mut out = String::with_capacity(cargo_toml.len() + quoted.len() + 16);
    let mut found = false;
    for line in cargo_toml.lines() {
        // The first one is the [dependencies] entry; the [dev-dependencies] one
        // (`ocre::testing`) inherits its features.
        if !found && line.starts_with("ocre = {") && line.ends_with('}') {
            found = true;
            if line.contains(&quoted) {
                out.push_str(line);
            } else if let Some((before, after)) = line.split_once("features = [") {
                write!(out, "{before}features = [{quoted}, {after}").expect("writing to a String");
            } else {
                write!(out, "{}, features = [{quoted}] }}", line.trim_end_matches('}').trim_end())
                    .expect("writing to a String");
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if found {
        Ok(out)
    } else {
        Err(CliError::new("Cargo.toml has no one-line `ocre = { ... }` dependency")
            .hint("declare Ocre as `ocre = { ... }` on one line under [dependencies], then run the command again"))
    }
}

#[cfg(test)]
#[path = "../../tests/generate/mod.rs"]
mod tests;

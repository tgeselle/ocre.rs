//! `ocre generate model|scaffold|api|migration|mailer|mailbox`.
//!
//! Generators collect every change in [`Edits`] and write nothing until the
//! whole generation succeeded, so a failure never leaves half a resource.

mod api;
mod auth;
mod fields;
mod mailbox;
mod mailer;
mod migration;
mod model;
mod scaffold;

use std::path::PathBuf;

pub use api::api;
pub use auth::auth;
pub use mailbox::mailbox;
pub use mailer::mailer;
pub use migration::migration;
pub use model::model;
pub use scaffold::scaffold;

use crate::{
    output::{CliError, Report},
    project::Project,
};

pub(crate) const MODULES_MARKER: &str = "// ocre:modules";
pub(crate) const ROUTES_MARKER: &str = "// ocre:routes";

/// Pending file changes, relative to the app root.
pub(crate) struct Edits<'a> {
    project: &'a Project,
    /// (path, contents, whether the file existed before)
    files: Vec<(String, String, bool)>,
}

impl<'a> Edits<'a> {
    pub fn new(project: &'a Project) -> Self {
        Self { project, files: Vec::new() }
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

    /// Adds a new file; refuses to overwrite anything.
    pub fn create(&mut self, path: &str, contents: String) -> Result<(), CliError> {
        if self.exists(path) {
            return Err(CliError::new(format!("{path} already exists")).hint(
                "generators create new files only; edit the existing file, or add a migration with `ocre g migration`",
            ));
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

    /// Writes everything and reports it, in the order the changes were made.
    pub fn apply(self, command: &'static str) -> Result<Report, CliError> {
        let mut report = Report::new(command);
        for (path, contents, existed) in self.files {
            let full = self.project.root.join(&path);
            std::fs::create_dir_all(full.parent().expect("app files have a parent"))?;
            std::fs::write(&full, contents)?;
            if existed { report.updated.push(path) } else { report.created.push(path) }
        }
        Ok(report)
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

/// Inserts `line` after the line holding `marker`, with the marker's
/// indentation. `None` when the marker is missing.
pub(crate) fn insert_after_marker(text: &str, marker: &str, line: &str) -> Option<String> {
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

#[cfg(test)]
#[path = "../../tests/generate/mod.rs"]
mod tests;

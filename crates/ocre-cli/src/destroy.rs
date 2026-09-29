//! `ocre destroy <generator> [name]` (alias `d`): undoes the latest matching
//! generator run, from its record in `.ocre/generated/`.
//!
//! Files the run created are deleted; lines it added to existing files are
//! taken out and the lines it replaced put back. Changes to Cargo.toml and
//! wrangler.toml stay (features and bindings that later code may use).
//! Nothing is touched when a created file changed since, or lines the run
//! added are no longer there, unless `--force`: then the files are deleted
//! anyway and the lines that cannot be found are left alone.

use std::path::Path;

use crate::{
    CliResult,
    generate::record::{self, KEPT, Record},
    output::{CliError, Report},
    project::Project,
};

pub fn destroy(project: &Project, generator: &str, name: Option<&str>, force: bool, pretend: bool) -> CliResult {
    let records = record::load_all(&project.root)?;
    let matches =
        |record: &Record| record.generator == generator && name.is_none_or(|name| key(&record.name) == key(name));
    let Some((_, record_path, record)) = records.iter().rev().find(|(_, _, record)| matches(record)) else {
        let wanted = format!("ocre g {generator}{}", name.map(|name| format!(" {name}")).unwrap_or_default());
        let recorded: Vec<&str> = records.iter().map(|(_, _, record)| record.command.as_str()).collect();
        return Err(CliError::new(format!("no recorded `{wanted}` run to destroy")).hint(if recorded.is_empty() {
            format!(
                "`ocre destroy` undoes runs recorded in {}/; this app has none, so remove the files by hand",
                record::DIR
            )
        } else {
            format!("recorded runs: {}", recorded.join(", "))
        }));
    };
    let root = &project.root;
    let mut report = Report::new("destroy");
    let mut conflicts = Vec::new();
    let mut deletions = Vec::new();
    for created in &record.created {
        let full = root.join(&created.path);
        let Ok(contents) = std::fs::read_to_string(&full) else { continue };
        if record::sha256(&contents) != created.sha256 {
            conflicts.push(format!("{} changed since it was generated", created.path));
        }
        deletions.push(created.path.clone());
    }
    let mut writes = Vec::new();
    for updated in &record.updated {
        if KEPT.contains(&updated.path.as_str()) {
            report.skipped.push(updated.path.clone());
            continue;
        }
        let Ok(mut text) = std::fs::read_to_string(root.join(&updated.path)) else {
            conflicts.push(format!("{} no longer exists", updated.path));
            continue;
        };
        let mut missing = false;
        for hunk in updated.hunks.iter().rev() {
            match record::revert(&text, hunk) {
                Some(reverted) => text = reverted,
                None => {
                    missing = true;
                    let line = hunk.added.first().or(hunk.after.as_ref()).cloned().unwrap_or_default();
                    conflicts.push(format!("{}: the generated `{}` changed", updated.path, line.trim()));
                }
            }
        }
        if missing {
            report.skipped.push(updated.path.clone());
        }
        writes.push((updated.path.clone(), text));
    }
    if !conflicts.is_empty() && !force {
        return Err(CliError::new(format!("cannot destroy `{}`: {}", record.command, conflicts.join("; "))).hint(
            "destroy later generator runs first (newest first), undo your edits, or pass --force to delete the generated files anyway and leave changed lines in place",
        ));
    }
    let record_file = format!("{}/{}", record::DIR, record_path.file_name().unwrap_or_default().to_string_lossy());
    if !pretend {
        for path in &deletions {
            std::fs::remove_file(root.join(path))?;
            remove_empty_parents(root, &root.join(path));
        }
        for (path, text) in &writes {
            std::fs::write(root.join(path), text)?;
        }
        std::fs::remove_file(record_path)?;
        remove_empty_parents(root, record_path);
    }
    report.updated = writes.into_iter().map(|(path, _)| path).collect();
    if let Some(migration) = deletions.iter().find(|path| path.starts_with("migrations/")) {
        report.next.push(format!(
            "if `ocre migrate` already applied {migration}, its tables and columns stay: undo them with a new migration (`ocre g migration ...`)"
        ));
    }
    report.removed = deletions;
    report.removed.push(record_file);
    report.pretend = pretend;
    Ok(report)
}

/// `BlogPost`, `blog_post` and `blog-post` are the same name.
fn key(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// Deletes the directories left empty by a removed file, up to the app root.
fn remove_empty_parents(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir {
        if current == root || std::fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}

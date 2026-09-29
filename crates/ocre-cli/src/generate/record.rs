//! Generation records: what each `ocre generate` run changed, so that
//! `ocre destroy` can take it back.
//!
//! One JSON file per run in `.ocre/generated/`, numbered like migrations
//! (`0003_scaffold_post.json`) and meant to be committed: files created
//! (with a SHA-256 of what was written, to notice later edits) and, for
//! files updated, the lines added and removed around a context line.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::output::CliError;

/// Directory of the records, relative to the app root.
pub(crate) const DIR: &str = ".ocre/generated";

/// Files whose changes `ocre destroy` keeps: features and bindings, which
/// later generators may rely on without having changed them.
pub(crate) const KEPT: [&str; 2] = ["Cargo.toml", "wrangler.toml"];

/// One generator run.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Record {
    /// The command as typed, e.g. `ocre g scaffold Post title:string`.
    pub command: String,
    /// `scaffold`, `model`, ...
    pub generator: String,
    /// First argument after the generator (`Post`), empty when there is none.
    pub name: String,
    pub created: Vec<Created>,
    pub updated: Vec<Updated>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Created {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct Updated {
    pub path: String,
    pub hunks: Vec<Hunk>,
}

/// Consecutive changed lines: `removed` became `added`, right after the
/// line `after` (`None`: at the start of the file).
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub(crate) struct Hunk {
    pub after: Option<String>,
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

impl Record {
    /// `generate api` + `["g", "scaffold", "Post", "title:string", "--json"]`
    /// (the typed generator wins: an API-only app's scaffold runs `api`).
    pub fn new(command: &str, invocation: &[String]) -> Self {
        let typed: Vec<&String> =
            invocation.iter().filter(|arg| !matches!(arg.as_str(), "--json" | "--force" | "--skip")).collect();
        let mut positional = invocation
            .iter()
            .filter(|arg| !arg.starts_with('-'))
            .skip_while(|arg| !matches!(arg.as_str(), "g" | "generate"))
            .skip(1);
        let generator = positional
            .next()
            .cloned()
            .unwrap_or_else(|| command.strip_prefix("generate ").unwrap_or(command).to_owned());
        let name = positional.next().cloned().unwrap_or_default();
        let command = if typed.is_empty() {
            format!("ocre {command}")
        } else {
            let quoted: Vec<String> = typed.iter().map(|arg| quote(arg)).collect();
            format!("ocre {}", quoted.join(" "))
        };
        Self { command, generator, name, created: Vec::new(), updated: Vec::new() }
    }

    pub fn created(&mut self, path: &str, contents: &str) {
        self.created.push(Created { path: path.to_owned(), sha256: sha256(contents) });
    }

    pub fn updated(&mut self, path: &str, old: &str, new: &str) {
        let hunks = diff(old, new);
        if !hunks.is_empty() {
            self.updated.push(Updated { path: path.to_owned(), hunks });
        }
    }

    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.updated.is_empty()
    }

    /// Writes `.ocre/generated/NNNN_<generator>[_<name>].json`, numbered
    /// after the highest record.
    pub fn save(&self, root: &Path) -> Result<String, CliError> {
        let highest = load_all(root)?.last().map_or(0, |(number, _, _)| *number);
        let mut slug = self.generator.clone();
        let name: String =
            self.name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
        if !name.is_empty() {
            slug = format!("{slug}_{name}");
        }
        let path = format!("{DIR}/{:04}_{slug}.json", highest + 1);
        std::fs::create_dir_all(root.join(DIR))?;
        let json = serde_json::to_string_pretty(self).expect("records serialize");
        std::fs::write(root.join(&path), json + "\n")?;
        Ok(path)
    }
}

/// Every record with its number and path, oldest first.
pub(crate) fn load_all(root: &Path) -> Result<Vec<(u32, PathBuf, Record)>, CliError> {
    let dir = root.join(DIR);
    let mut records = Vec::new();
    if !dir.is_dir() {
        return Ok(records);
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let Some(number) = file.split('_').next().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if !file.ends_with(".json") {
            continue;
        }
        let record = serde_json::from_str(&std::fs::read_to_string(&path)?).map_err(|err| {
            CliError::new(format!("{DIR}/{file} is not a valid generation record: {err}"))
                .hint("restore it from version control, or delete it if you no longer need `ocre destroy` for it")
        })?;
        records.push((number, path, record));
    }
    records.sort_by_key(|(number, _, _)| *number);
    Ok(records)
}

pub(crate) fn sha256(contents: &str) -> String {
    Sha256::digest(contents.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Single-quotes arguments a shell would split or expand.
fn quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_:.,/=?^@+".contains(c)) {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Line changes from `old` to `new`, as hunks in file order (longest common
/// subsequence of lines, after trimming the common start and end).
pub(crate) fn diff(old: &str, new: &str) -> Vec<Hunk> {
    let (old, new): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (a, b) = (&old[prefix..old.len() - suffix], &new[prefix..new.len() - suffix]);
    // lcs[i][j]: common subsequence length of a[i..] and b[j..].
    let width = b.len() + 1;
    let mut lcs = vec![0u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i * width + j] = if a[i] == b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let mut hunks = Vec::new();
    let mut current: Option<Hunk> = None;
    let (mut i, mut j) = (0, 0);
    // Line of `new` just before the current position.
    let before = |j: usize| (prefix + j).checked_sub(1).map(|k| new[k].to_owned());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            hunks.extend(current.take());
            i += 1;
            j += 1;
            continue;
        }
        let hunk = current.get_or_insert_with(|| Hunk { after: before(j), removed: Vec::new(), added: Vec::new() });
        if j < b.len() && (i == a.len() || lcs[i * width + j + 1] >= lcs[(i + 1) * width + j]) {
            hunk.added.push(b[j].to_owned());
            j += 1;
        } else {
            hunk.removed.push(a[i].to_owned());
            i += 1;
        }
    }
    hunks.extend(current);
    hunks
}

/// Undoes `hunk` in `text`: finds `added` right after `after` (or, when
/// later lines were inserted in between, the only place those lines are)
/// and puts `removed` back. `None` when those lines are no longer there.
pub(crate) fn revert(text: &str, hunk: &Hunk) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let added: Vec<&str> = hunk.added.iter().map(String::as_str).collect();
    let at = |from: usize| lines.len() >= from + added.len() && lines[from..from + added.len()] == added[..];
    let exact = match &hunk.after {
        None => at(0).then_some(0),
        Some(after) => (0..lines.len()).find(|&k| lines[k] == after && at(k + 1)).map(|k| k + 1),
    };
    let start = match exact {
        Some(start) => start,
        None if !added.is_empty() => {
            let mut found = (0..lines.len()).filter(|&k| at(k));
            match (found.next(), found.next()) {
                (Some(start), None) => start,
                _ => return None,
            }
        }
        None => return None,
    };
    let mut out: Vec<&str> = lines[..start].to_vec();
    out.extend(hunk.removed.iter().map(String::as_str));
    out.extend(&lines[start + added.len()..]);
    let mut joined = out.join("\n");
    if text.ends_with('\n') && !joined.is_empty() {
        joined.push('\n');
    }
    Some(joined)
}

#[cfg(test)]
#[path = "../../tests/generate/record.rs"]
mod tests;

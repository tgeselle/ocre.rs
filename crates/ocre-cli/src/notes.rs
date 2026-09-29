//! `ocre notes`: the TODO, FIXME and OPTIMIZE comments of the app, like
//! `rails notes`.

use serde::Serialize;

use crate::{CliResult, output::Report, project::Project, stats::files};

/// Tags listed when `--annotations` is not given.
pub const DEFAULT_TAGS: [&str; 3] = ["TODO", "FIXME", "OPTIMIZE"];

/// Directories searched, relative to the app root.
const DIRS: [&str; 6] = ["src", "templates", "migrations", "tests", "db", "locales"];

/// Comment openers of Rust, askama/HTML, SQL, YAML and TOML.
const OPENERS: [&str; 6] = ["//", "/*", "{#", "<!--", "--", "#"];

#[derive(Serialize, Debug, PartialEq)]
pub struct Note {
    pub path: String,
    pub line: usize,
    pub tag: String,
    pub text: String,
}

pub fn run(project: &Project, tags: &[String]) -> CliResult {
    let tags: Vec<&str> =
        if tags.is_empty() { DEFAULT_TAGS.to_vec() } else { tags.iter().map(String::as_str).collect() };
    let mut notes = Vec::new();
    for dir in DIRS {
        for path in files(&project.root.join(dir))? {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let relative = path.strip_prefix(&project.root).expect("under the root").to_string_lossy().into_owned();
            for (index, line) in text.lines().enumerate() {
                if let Some((tag, text)) = find(line, &tags) {
                    notes.push(Note { path: relative.clone(), line: index + 1, tag: tag.to_owned(), text });
                }
            }
        }
    }
    Ok(Report { notes: Some(notes), ..Report::new("notes") })
}

/// The first tag in the comment of `line`, with the text after it.
fn find<'t>(line: &str, tags: &[&'t str]) -> Option<(&'t str, String)> {
    let start = OPENERS.iter().filter_map(|opener| line.find(opener)).min()?;
    let comment = &line[start..];
    for (offset, _) in comment.char_indices() {
        let rest = &comment[offset..];
        let boundary = comment[..offset].chars().next_back().is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if !boundary {
            continue;
        }
        for tag in tags {
            let Some(after) = rest.strip_prefix(tag) else { continue };
            if after.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let text = after.trim_start_matches(':').trim();
            let text = ["#}", "-->", "*/"].iter().fold(text, |text, end| text.strip_suffix(end).unwrap_or(text));
            return Some((tag, text.trim().to_owned()));
        }
    }
    None
}

//! `ocre stats`: lines of code per part of the app, like `rails stats`.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    CliResult,
    output::{CliError, Report},
    project::Project,
};

/// One row of `ocre stats`.
#[derive(Serialize, Debug, Default, PartialEq)]
pub struct Row {
    pub name: String,
    pub files: usize,
    pub lines: usize,
    /// Lines that are neither blank nor only a comment.
    pub loc: usize,
    /// Rust functions (`fn` declarations).
    pub functions: usize,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Stats {
    pub rows: Vec<Row>,
    /// Rust lines of code outside tests/.
    pub code_loc: usize,
    /// Rust lines of code in tests/.
    pub test_loc: usize,
}

/// Parts of an app, by directory; the first match wins, so `Controllers`
/// is the rest of src/.
const PARTS: [(&str, &str); 8] = [
    ("Models", "src/models"),
    ("Jobs", "src/jobs"),
    ("Mailers", "src/mailers"),
    ("Schedules", "src/schedules"),
    ("Controllers and app", "src"),
    ("Templates", "templates"),
    ("Migrations", "migrations"),
    ("Tests", "tests"),
];

/// `extra`: more directories, each reported as its own row.
pub fn run(project: &Project, extra: &[String]) -> CliResult {
    let mut rows: Vec<Row> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let parts = PARTS.iter().map(|(name, dir)| ((*name).to_owned(), (*dir).to_owned()));
    let extra = extra.iter().map(|dir| (dir.trim_end_matches('/').to_owned(), dir.trim_end_matches('/').to_owned()));
    for (name, dir) in parts.chain(extra) {
        let full = project.root.join(&dir);
        if !full.is_dir() && !PARTS.iter().any(|(_, known)| *known == dir) {
            return Err(CliError::new(format!("{dir} is not a directory of the app"))
                .hint("pass directories relative to the app root, e.g. `ocre stats lib`"));
        }
        let mut row = Row { name, ..Row::default() };
        for file in files(&full)? {
            if seen.contains(&file) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            row.files += 1;
            for line in text.lines() {
                let line = line.trim();
                row.lines += 1;
                if !line.is_empty() && !is_comment(line) {
                    row.loc += 1;
                }
                if is_fn(line) {
                    row.functions += 1;
                }
            }
            seen.insert(file);
        }
        rows.push(row);
    }
    let rust_loc = |rows: &[Row], tests: bool| {
        rows.iter()
            .filter(|row| !matches!(row.name.as_str(), "Templates" | "Migrations"))
            .filter(|row| (row.name == "Tests") == tests)
            .map(|row| row.loc)
            .sum()
    };
    let stats = Stats { code_loc: rust_loc(&rows, false), test_loc: rust_loc(&rows, true), rows };
    Ok(Report { stats: Some(stats), ..Report::new("stats") })
}

/// `// ...`, `-- ...`, `{# ... #}`, `<!-- ... -->`, `# ...` and `/* ... */` lines.
fn is_comment(line: &str) -> bool {
    ["//", "--", "{#", "<!--", "#", "/*", "*"].iter().any(|start| line.starts_with(start)) && !line.starts_with("#[")
}

fn is_fn(line: &str) -> bool {
    let mut words = line.split_whitespace().skip_while(|word| {
        word.starts_with("pub") || matches!(*word, "async" | "const" | "unsafe" | "extern" | "\"C\"")
    });
    words.next() == Some("fn")
}

/// Every file under `dir`, sorted, skipping hidden entries (empty when `dir` is missing).
pub fn files(dir: &Path) -> Result<Vec<PathBuf>, CliError> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?.map(|e| e.map(|e| e.path())).collect::<Result<_, _>>()?;
    entries.sort();
    for path in entries {
        if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')) {
            continue;
        }
        if path.is_dir() { out.extend(files(&path)?) } else { out.push(path) }
    }
    Ok(out)
}

/// The table printed by `ocre stats`.
pub fn table(stats: &Stats) -> String {
    let mut out = format!("{:<22} {:>6} {:>7} {:>7} {:>9}\n", "Name", "Files", "Lines", "LOC", "Functions");
    for row in &stats.rows {
        out.push_str(&format!(
            "{:<22} {:>6} {:>7} {:>7} {:>9}\n",
            row.name, row.files, row.lines, row.loc, row.functions
        ));
    }
    let ratio = if stats.code_loc == 0 { 0.0 } else { stats.test_loc as f64 / stats.code_loc as f64 };
    out.push_str(&format!(
        "\nCode LOC: {}    Test LOC: {}    Code to test ratio: 1:{ratio:.1}\n",
        stats.code_loc, stats.test_loc
    ));
    out
}

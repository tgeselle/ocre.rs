//! Application templates: a text file of `ocre` commands applied to an app,
//! by `ocre new <name> --template <path or URL>` or later by
//! `ocre template <path or URL>` (Rails' `rails new -m` and `app:template`).
//!
//! ```text
//! # Blog with comments. One command per line; `#` starts a comment.
//! g scaffold Post title:string body:text published:boolean
//! g scaffold Comment body:text post:references
//! g job NotifyAuthor comment_id:integer
//! cargo add slug
//! migrate
//! ```
//!
//! Lines run in order, each as `ocre <line>` in the app (the leading `ocre`
//! is optional); `cargo add`/`cargo remove` lines add or remove crates.
//! Only commands that change the app locally are allowed: generators,
//! `destroy`, `migrate`, `db`, `sql`, `i18n`, `routes`; nothing with
//! `--remote`, and no `deploy`, `login` or shell commands. Every line is
//! checked before the first one runs; the first failing line stops the run.
//! A template runs generators and `cargo add` on your machine: apply only
//! templates you trust.

use std::{
    path::Path,
    process::{Command, Stdio},
};

use crate::{
    CliResult,
    output::{CliError, Report},
};

/// First words a template line may start with (after an optional `ocre`).
const ALLOWED: [&str; 10] = ["g", "generate", "d", "destroy", "migrate", "db", "sql", "i18n", "routes", "cargo"];

/// A parsed template: the lines to run, each split into arguments.
#[derive(Debug, PartialEq)]
pub struct Template {
    pub source: String,
    pub lines: Vec<Vec<String>>,
}

impl Template {
    /// Reads a file, or downloads an `http(s)://` URL, then checks every line.
    pub fn load(source: &str, cwd: &Path) -> Result<Self, CliError> {
        let text = if source.starts_with("https://") || source.starts_with("http://") {
            let failed = |err: &dyn std::fmt::Display| {
                CliError::new(format!("could not download the template {source}: {err}"))
                    .hint("check the URL (it must serve the template as plain text), or download it and pass its path")
            };
            let mut response = ureq::get(source).call().map_err(|err| failed(&err))?;
            response.body_mut().read_to_string().map_err(|err| failed(&err))?
        } else {
            std::fs::read_to_string(cwd.join(source)).map_err(|err| {
                CliError::new(format!("could not read the template {source}: {err}"))
                    .hint("pass the path of a text file of ocre commands, or an https:// URL")
            })?
        };
        Self::parse(source, &text)
    }

    fn parse(source: &str, text: &str) -> Result<Self, CliError> {
        let mut lines = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let invalid = |why: &str| {
                CliError::new(format!("{source} line {}: `{line}` {why}", index + 1)).hint(
                    "template lines are ocre commands that change the app locally: g/generate, destroy, migrate, db, sql, i18n, routes, or `cargo add`/`cargo remove`, without --remote",
                )
            };
            let mut words = split(line).ok_or_else(|| invalid("has an unclosed quote"))?;
            if words.first().is_some_and(|word| word == "ocre") {
                words.remove(0);
            }
            let allowed = words.first().is_some_and(|first| ALLOWED.contains(&first.as_str()));
            if !allowed {
                return Err(invalid("is not allowed in a template"));
            }
            if words[0] == "cargo" && !matches!(words.get(1).map(String::as_str), Some("add" | "remove")) {
                return Err(invalid("is not allowed in a template"));
            }
            if words.iter().any(|word| word == "--remote") {
                return Err(invalid("targets production (--remote)"));
            }
            lines.push(words);
        }
        Ok(Self { source: source.to_owned(), lines })
    }

    /// Runs every line in the app at `root`, merging their reports.
    pub fn apply(&self, root: &Path) -> CliResult {
        let mut report = Report::new("template");
        let exe = std::env::current_exe()?;
        for words in &self.lines {
            let shown = words.join(" ");
            if words[0] == "cargo" {
                let status = Command::new("cargo")
                    .args(words.iter().skip(1))
                    .current_dir(root)
                    .stdout(Stdio::from(std::io::stderr()))
                    .status()?;
                if !status.success() {
                    return Err(CliError::new(format!("template line `{shown}` failed ({status})"))
                        .hint("the cargo output above names the cause"));
                }
                report.updated.push("Cargo.toml".to_owned());
                report.ran.push(shown);
                continue;
            }
            let output = Command::new(&exe).args(words).arg("--json").current_dir(root).output()?;
            let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_default();
            if !output.status.success() {
                let error = result["error"].as_str().unwrap_or("it exited with an error");
                return Err(CliError::new(format!("template line `ocre {shown}` failed: {error}"))
                    .hint(result["hint"].as_str().unwrap_or("run the line by hand to see the error").to_owned()));
            }
            for (key, list) in [("created", &mut report.created), ("updated", &mut report.updated)] {
                let paths = result[key].as_array().into_iter().flatten().filter_map(|p| p.as_str());
                list.extend(paths.map(str::to_owned));
            }
            report.ran.push(format!("ocre {shown}"));
        }
        report.updated.sort();
        report.updated.dedup();
        report.updated.retain(|path| !report.created.contains(path));
        Ok(report)
    }
}

/// Splits a line like a shell: spaces separate words, '...' and "..." quote.
/// `None` for an unclosed quote.
fn split(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            (None, c) => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}

/// `ocre template <path or URL>`: applies a template to the current app.
pub fn run(source: &str) -> CliResult {
    let project = crate::project::Project::find()?;
    let cwd = std::env::current_dir()?;
    Template::load(source, &cwd)?.apply(&project.root)
}

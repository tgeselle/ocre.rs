//! Generator templates, and the app's overrides of them.
//!
//! Built-in templates live in `crates/ocre-cli/templates/generate/` and are
//! compiled into the CLI. An app overrides one by putting a file at the same
//! path under `.ocre/templates/` (`ocre g override controller/html.rs`
//! copies the built-in one there to start from); deleting it restores the
//! built-in template.
//!
//! Templates use minijinja with ERB-style delimiters, so the askama and Rust
//! braces of the generated code stay literal: `<%= name %>` prints a value,
//! `<% for action in actions %>...<% endfor %>` and `<% if api %>...<% endif %>`
//! are blocks, `<%# ... %>` is a comment. A newline right after a block tag
//! is dropped.

use minijinja::{Environment, Value, syntax::SyntaxConfig};
use serde::Serialize;

use super::Edits;
use crate::{
    CliResult,
    output::{CliError, Report},
    project::Project,
};

/// A generator template, as listed by `ocre g override`.
#[derive(Serialize)]
pub struct TemplateInfo {
    pub path: String,
    /// The app has its own copy in `.ocre/templates/`.
    pub overridden: bool,
}

/// Overrides, relative to the app root.
pub(crate) const OVERRIDES: &str = ".ocre/templates";

/// Every built-in template, by path.
pub(crate) const BUILTIN: [(&str, &str); 13] = [
    ("controller/api.rs", include_str!("../../templates/generate/controller/api.rs")),
    ("controller/html.rs", include_str!("../../templates/generate/controller/html.rs")),
    ("controller/view.html", include_str!("../../templates/generate/controller/view.html")),
    ("resource/api.rs", include_str!("../../templates/generate/resource/api.rs")),
    ("resource/html.rs", include_str!("../../templates/generate/resource/html.rs")),
    ("resource/index.html", include_str!("../../templates/generate/resource/index.html")),
    ("resource/show.html", include_str!("../../templates/generate/resource/show.html")),
    ("scaffold/_form.html", include_str!("../../templates/generate/scaffold/_form.html")),
    ("scaffold/_row.html", include_str!("../../templates/generate/scaffold/_row.html")),
    ("scaffold/edit.html", include_str!("../../templates/generate/scaffold/edit.html")),
    ("scaffold/index.html", include_str!("../../templates/generate/scaffold/index.html")),
    ("scaffold/new.html", include_str!("../../templates/generate/scaffold/new.html")),
    ("scaffold/show.html", include_str!("../../templates/generate/scaffold/show.html")),
];

/// Renders the template at `path` (the app's override when there is one)
/// with `context`.
pub(crate) fn render(edits: &Edits, path: &str, context: &Value) -> Result<String, CliError> {
    let override_path = format!("{OVERRIDES}/{path}");
    match edits.read(&override_path)? {
        Some(source) => render_source(&override_path, &source, context),
        None => {
            let (_, source) = BUILTIN.iter().find(|(p, _)| *p == path).expect("generators use built-in paths");
            render_source(&format!("built-in template {path}"), source, context)
        }
    }
}

/// Renders template text; `origin` names it in errors.
pub(crate) fn render_source(origin: &str, source: &str, context: &Value) -> Result<String, CliError> {
    let failed = |err: minijinja::Error| {
        let mut message = format!("{origin} failed to render: {err}");
        if let Some(detail) = err.detail() {
            message = format!("{message} ({detail})");
        }
        let hint = if origin.starts_with(OVERRIDES) {
            format!("fix {origin}, or delete it to use the built-in template again")
        } else {
            format!("fix {origin}")
        };
        CliError::new(message).hint(hint)
    };
    let mut env = Environment::new();
    env.set_syntax(
        SyntaxConfig::builder()
            .block_delimiters("<%", "%>")
            .variable_delimiters("<%=", "%>")
            .comment_delimiters("<%#", "%>")
            .build()
            .expect("valid delimiters"),
    );
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.set_keep_trailing_newline(true);
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.render_str(source, context).map_err(failed)
}

/// `ocre g override [<path>...]`: copies built-in templates (a file, or
/// every file of a generator such as `controller`) into `.ocre/templates/`;
/// without paths, lists them.
pub fn override_templates(project: &Project, paths: &[String]) -> CliResult {
    let mut edits = Edits::new(project);
    if paths.is_empty() {
        let templates = BUILTIN
            .iter()
            .map(|(path, _)| TemplateInfo {
                path: (*path).to_owned(),
                overridden: edits.exists(&format!("{OVERRIDES}/{path}")),
            })
            .collect();
        let mut report = Report::new("generate override");
        report.templates = Some(templates);
        report.next =
            vec!["ocre g override <path> (e.g. controller/html.rs, or controller for all its files)".to_owned()];
        return Ok(report);
    }
    for wanted in paths {
        let wanted = wanted.trim_end_matches('/');
        let matching: Vec<&(&str, &str)> = BUILTIN
            .iter()
            .filter(|(path, _)| *path == wanted || path.strip_prefix(wanted).is_some_and(|rest| rest.starts_with('/')))
            .collect();
        if matching.is_empty() {
            let all: Vec<&str> = BUILTIN.iter().map(|(path, _)| *path).collect();
            return Err(CliError::new(format!("no generator template `{wanted}`"))
                .hint(format!("templates: {}; or a generator, e.g. `controller`", all.join(", "))));
        }
        for (path, source) in matching {
            edits.create(&format!("{OVERRIDES}/{path}"), (*source).to_owned())?;
        }
    }
    let mut report = edits.apply("generate override")?;
    report.next = vec![format!(
        "edit the files in {OVERRIDES}/ (<%= value %>, <% for x in xs %>...<% endfor %>); generators use them until deleted"
    )];
    Ok(report)
}

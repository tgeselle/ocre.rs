//! The app's own generators: `ocre g <name> <Name> [args...]` runs
//! `.ocre/generators/<name>/` when `<name>` is not a built-in generator.
//!
//! Every file in that directory except `generator.toml` is a template
//! (same syntax as [`super::templates`]); its path, also a template, says
//! where the output goes (`src/services/<%= singular %>.rs`). The optional
//! `generator.toml` describes the generator and lists lines to insert after
//! markers of existing files:
//!
//! ```toml
//! description = "Service object in src/services/"
//!
//! [[insert]]
//! file = "src/services/mod.rs"
//! after = "// ocre:services"
//! line = "pub mod <%= singular %>;"
//! ```
//!
//! Templates see the names of the first argument (`model`, `singular`,
//! `plural`, `human_singular`, `human_plural`), `args` (the other
//! arguments), `fields` (when every other argument is `name:type`) and
//! `options` (`--key=value` and `--flag` arguments). `ocre g generator
//! <name>` creates a starting point.

use std::collections::BTreeMap;

use minijinja::Value;
use serde::{Deserialize, Serialize};

use super::{Edits, fields::parse_fields, insert_after_marker, templates::render_source};
use crate::{
    CliResult,
    names::{ModelNames, is_identifier},
    output::CliError,
    project::Project,
};

/// App generators, relative to the app root.
pub(crate) const DIR: &str = ".ocre/generators";

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Config {
    #[allow(dead_code)] // Documentation for readers of the directory.
    description: Option<String>,
    #[serde(default)]
    insert: Vec<Insert>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Insert {
    file: String,
    after: String,
    line: String,
}

#[derive(Serialize)]
struct Field {
    name: String,
    label: String,
    rust_type: String,
    optional: bool,
    unique: bool,
}

#[derive(Serialize)]
struct Context {
    model: String,
    singular: String,
    plural: String,
    human_singular: String,
    human_plural: String,
    args: Vec<String>,
    fields: Vec<Field>,
    options: BTreeMap<String, String>,
}

/// Runs `.ocre/generators/<generator>/` with `args` (`Name`, then the rest).
pub fn custom(project: &Project, generator: &str, args: &[String]) -> CliResult {
    let dir = project.root.join(DIR).join(generator);
    if !is_identifier(generator) || !dir.is_dir() {
        return Err(CliError::new(format!("unknown generator `{generator}`")).hint(format!(
            "run `ocre g --help` for the built-in generators; app generators live in {DIR}/<name>/ (create one with `ocre g generator {generator}`)"
        )));
    }
    let (options, positional): (Vec<&String>, Vec<&String>) = args.iter().partition(|arg| arg.starts_with("--"));
    let name = positional.first().ok_or_else(|| {
        CliError::new(format!("`ocre g {generator}` needs a name"))
            .hint(format!("run `ocre g {generator} <Name> [args...]`, e.g. `ocre g {generator} Invoice`"))
    })?;
    let names = ModelNames::parse(name)?;
    let rest: Vec<String> = positional[1..].iter().map(|arg| (*arg).clone()).collect();
    let fields = if !rest.is_empty() && rest.iter().all(|arg| arg.contains(':')) {
        parse_fields(&rest)?
            .into_iter()
            .map(|f| Field {
                label: f.label(),
                rust_type: f.column_type(),
                name: f.name,
                optional: f.optional,
                unique: f.unique,
            })
            .collect()
    } else {
        Vec::new()
    };
    let options = options
        .iter()
        .map(|option| {
            let option = option.trim_start_matches('-');
            match option.split_once('=') {
                Some((key, value)) => (key.replace('-', "_"), value.to_owned()),
                None => (option.replace('-', "_"), "true".to_owned()),
            }
        })
        .collect();
    let ModelNames { model, singular, plural, human_singular, human_plural } = names;
    let context = Value::from_serialize(Context {
        model,
        singular,
        plural,
        human_singular,
        human_plural,
        args: rest,
        fields,
        options,
    });
    let config_path = dir.join("generator.toml");
    let config: Config = match std::fs::read_to_string(&config_path) {
        Ok(text) => toml::from_str(&text).map_err(|err| {
            CliError::new(format!("{DIR}/{generator}/generator.toml is invalid: {err}"))
                .hint("keys: `description`, and [[insert]] tables with `file`, `after` and `line`")
        })?,
        Err(_) => Config::default(),
    };
    let mut edits = Edits::new(project);
    for file in crate::stats::files(&dir)? {
        if file == config_path {
            continue;
        }
        let relative = file.strip_prefix(&dir).expect("under the generator").to_string_lossy().into_owned();
        let origin = format!("{DIR}/{generator}/{relative}");
        let path = render_source(&origin, &relative, &context)?;
        let source = std::fs::read_to_string(&file)?;
        edits.create(&path, render_source(&origin, &source, &context)?)?;
    }
    for insert in &config.insert {
        let origin = format!("{DIR}/{generator}/generator.toml");
        let path = render_source(&origin, &insert.file, &context)?;
        let line = render_source(&origin, &insert.line, &context)?;
        let text = edits.read(&path)?.ok_or_else(|| {
            CliError::new(format!("{path} does not exist")).hint(format!(
                "create it with the `{}` marker line, or change the [[insert]] of {origin}",
                insert.after
            ))
        })?;
        let updated = insert_after_marker(&text, &insert.after, &line).ok_or_else(|| {
            CliError::new(format!("{path} is missing the `{}` marker", insert.after))
                .hint(format!("put `{}` on its own line where the generated lines go", insert.after))
        })?;
        edits.update(&path, updated);
    }
    edits.apply("generate custom")
}

const EXAMPLE_TOML: &str = r#"# `ocre g __GENERATOR__ <Name> [args...]`: every other file in this directory is
# a template, and so is its path. Templates see: model, singular, plural,
# human_singular, human_plural (from <Name>), args, fields (when the args are
# name:type), options (--key=value, --flag). Syntax: <%= value %>,
# <% for field in fields %>...<% endfor %>, <% if options.api %>...<% endif %>.
description = "Describe what `ocre g __GENERATOR__` generates"

# Lines to add after a marker line of an existing file (repeatable).
# [[insert]]
# file = "src/lib.rs"
# after = "// ocre:modules"
# line = "mod <%= plural %>;"
"#;

const EXAMPLE_FILE: &str = r#"//! <%= human_singular %>. Generated by `ocre g __GENERATOR__ <%= model %>`.

pub struct <%= model %> {
<% for field in fields %>
    pub <%= field.name %>: <%= field.rust_type %>,
<% endfor %>
}
"#;

/// `ocre g generator <name>`: a new app generator to edit.
pub fn generator(project: &Project, name: &str) -> CliResult {
    let names = ModelNames::parse(name)?;
    let singular = &names.singular;
    let dir = format!("{DIR}/{singular}");
    let mut edits = Edits::new(project);
    edits.create(&format!("{dir}/generator.toml"), EXAMPLE_TOML.replace("__GENERATOR__", singular))?;
    edits.create(
        &format!("{dir}/src/{}/<%= singular %>.rs", names.plural),
        EXAMPLE_FILE.replace("__GENERATOR__", singular),
    )?;
    let mut report = edits.apply("generate generator")?;
    report.next =
        vec![format!("edit the templates in {dir}/"), format!("ocre g {singular} Example name:string --pretend")];
    Ok(report)
}

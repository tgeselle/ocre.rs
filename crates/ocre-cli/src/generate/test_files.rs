//! Test files the generators write: a factory per model
//! (`tests/factories/<model>.rs`, FactoryBot-style builders) and request
//! tests for scaffolds and APIs (`tests/<plural>.rs`). Request tests are
//! `#[ignore]`d in plain `cargo test`; `ocre test --e2e` runs them against
//! the app in workerd.

use std::fmt::Write as _;

use super::{
    Edits,
    fields::{Field, FieldType},
    insert_after_marker,
};
use crate::{
    names::{ModelNames, humanize},
    output::CliError,
};

pub(super) const FACTORIES_MARKER: &str = "// ocre:factories";

/// The attribute every request test carries.
const IGNORE: &str = "#[ignore = \"request test: run with `ocre test --e2e`\"]";

const FACTORIES_MOD: &str = r#"//! Test data, one module per model (FactoryBot-style): `post()` returns valid
//! attributes, unique per call; `insert()` writes them to the test database,
//! `form()` and `json()` give them as a form or an API sends them. A test
//! file uses them with `mod factories;`. `ocre g model` adds a module below.

// Each test file uses only some factories.
#![allow(dead_code)]

use ocre::serde_json::Value;

// ocre:factories

/// A column value as form text: strings as they are, `true`/`false`,
/// numbers, and an empty field for `NULL`.
pub fn form_text(value: Value) -> String {
    match value {
        Value::String(text) => text,
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
"#;

/// Writes `tests/factories/<singular>.rs` and registers it in `tests/factories/mod.rs`.
pub(super) fn add_factory(
    edits: &mut Edits,
    names: &ModelNames,
    fields: &[Field],
    command: &str,
) -> Result<(), CliError> {
    ensure_test_dependency(edits)?;
    let registry = edits.read("tests/factories/mod.rs")?.unwrap_or_else(|| FACTORIES_MOD.to_owned());
    let registry = insert_after_marker(&registry, FACTORIES_MARKER, &format!("pub mod {};", names.singular))
        .ok_or_else(|| {
            CliError::new(format!("tests/factories/mod.rs is missing the `{FACTORIES_MARKER}` marker"))
                .hint(format!("put `{FACTORIES_MARKER}` on its own line where the `pub mod` lines go"))
        })?;
    edits.update("tests/factories/mod.rs", registry);
    let factory =
        factory_rs(names, fields, command, |target| edits.exists(&format!("tests/factories/{}.rs", target.singular)));
    edits.create(&format!("tests/factories/{}.rs", names.singular), factory)
}

/// Adds Ocre with its `testing` feature to `[dev-dependencies]` (apps created
/// before generators wrote tests lack it); unchanged when it is there, or
/// when the app has no Cargo.toml (cargo reports that itself).
fn ensure_test_dependency(edits: &mut Edits) -> Result<(), CliError> {
    let Some(cargo_toml) = edits.read("Cargo.toml")? else { return Ok(()) };
    if cargo_toml.lines().any(|line| line.starts_with("ocre = {") && line.contains("\"testing\"")) {
        return Ok(());
    }
    let ocre =
        cargo_toml.lines().find(|line| line.starts_with("ocre = {") && line.ends_with('}')).ok_or_else(|| {
            CliError::new("Cargo.toml has no one-line `ocre = { ... }` dependency")
                .hint("declare Ocre as `ocre = { ... }` on one line under [dependencies], then run the command again")
        })?;
    let line = format!("{}, features = [\"testing\"] }}", ocre.trim_end_matches('}').trim_end());
    let updated = match insert_after_marker(&cargo_toml, "[dev-dependencies]", &line) {
        Some(updated) => updated,
        None => format!("{}\n[dev-dependencies]\n{line}\n", cargo_toml.trim_end()),
    };
    edits.update("Cargo.toml", updated);
    Ok(())
}

/// A factory field: Rust type and default value (`n` is the sequence number).
struct FactoryField<'a> {
    field: &'a Field,
    ty: String,
    default: String,
    /// The default reads `n`.
    sequenced: bool,
}

impl<'a> FactoryField<'a> {
    /// `None` for attachments, which the factory leaves out.
    fn new(field: &'a Field) -> Option<Self> {
        let name = &field.name;
        let (ty, default) = match field.ty {
            FieldType::Attachment => return None,
            FieldType::String | FieldType::Text => ("String", format!("format!(\"{} {{n}}\")", humanize(name))),
            FieldType::Integer => ("i64", "n as i64".to_owned()),
            FieldType::Float => ("f64", "n as f64".to_owned()),
            FieldType::Decimal => ("String", "format!(\"{n}.99\")".to_owned()),
            FieldType::Boolean => ("bool", "false".to_owned()),
            FieldType::Date => ("String", "\"2026-01-01\".to_owned()".to_owned()),
            FieldType::Time => ("String", "\"12:00\".to_owned()".to_owned()),
            FieldType::DateTime => ("String", "\"2026-01-01 12:00:00\".to_owned()".to_owned()),
            FieldType::Uuid => ("String", "format!(\"00000000-0000-4000-8000-{n:012}\")".to_owned()),
            FieldType::Json => ("Value", "json!({})".to_owned()),
            FieldType::Enum => {
                let first = &field.enumeration.as_ref().expect("enum fields have values").values[0];
                ("String", format!("\"{first}\".to_owned()"))
            }
            FieldType::References => ("i64", "None".to_owned()),
        };
        let optional = field.optional || field.ty == FieldType::References;
        let sequenced = !field.optional && (default.contains("{n") || default.starts_with("n as"));
        Some(Self {
            field,
            ty: if optional { format!("Option<{ty}>") } else { ty.to_owned() },
            default: if field.optional { "None".to_owned() } else { default },
            sequenced,
        })
    }

    /// The value as the column stores it: JSON fields as their text.
    fn column_value(&self) -> String {
        let name = &self.field.name;
        match (self.field.ty, self.field.optional) {
            (FieldType::Json, false) => format!("Value::String(self.{name}.to_string())"),
            (FieldType::Json, true) => {
                format!("self.{name}.as_ref().map_or(Value::Null, |value| Value::String(value.to_string()))")
            }
            _ => format!("json!(self.{name})"),
        }
    }
}

/// `tests/factories/<singular>.rs`. `has_factory(target)` tells whether a
/// referenced model has a factory the parent can be created with.
fn factory_rs(
    names: &ModelNames,
    fields: &[Field],
    command: &str,
    has_factory: impl Fn(&ModelNames) -> bool,
) -> String {
    let ModelNames { model, singular, plural, human_plural, .. } = names;
    let factory_fields: Vec<FactoryField> = fields.iter().filter_map(FactoryField::new).collect();
    let (mut declarations, mut defaults, mut values, mut json_fields, mut parents) =
        (String::new(), String::new(), String::new(), String::new(), String::new());
    let mut creates_parents = false;
    for factory_field in &factory_fields {
        let field = factory_field.field;
        let name = &field.name;
        if field.ty == FieldType::References && !field.optional {
            writeln!(declarations, "    /// `None`: `with_parents` sets it.").expect("writing to a String");
            let target = field.target.as_ref().expect("references have a target");
            if has_factory(target) && target.singular != *singular {
                creates_parents = true;
                write!(
                    parents,
                    "        if self.{name}.is_none() {{\n            self.{name} = Some(super::{}::{}().insert());\n        }}\n",
                    target.singular, target.singular
                )
            } else {
                writeln!(
                    parents,
                    "        assert!(self.{name}.is_some(), \"set {name}: tests/factories has no {} factory to create one\");",
                    target.singular
                )
            }
            .expect("writing to a String");
        }
        writeln!(declarations, "    pub {name}: {},", factory_field.ty).expect("writing to a String");
        writeln!(defaults, "        {name}: {},", factory_field.default).expect("writing to a String");
        writeln!(values, "            (\"{name}\", {}),", factory_field.column_value()).expect("writing to a String");
        writeln!(json_fields, "            \"{name}\": self.{name},").expect("writing to a String");
    }
    let mut attachment_values = String::new();
    for field in fields.iter().filter(|field| field.is_attachment() && !field.optional) {
        let name = &field.name;
        write!(
            attachment_values,
            "        // A placeholder for the required file (no object in R2).\n        values.extend([\n            \
             (\"{name}_key\", json!(format!(\"factories/{plural}/{{}}\", ocre::testing::sequence()))),\n            \
             (\"{name}_filename\", json!(\"file.txt\")),\n            (\"{name}_content_type\", json!(\"text/plain\")),\n            \
             (\"{name}_size\", json!(0)),\n        ]);\n"
        )
        .expect("writing to a String");
    }
    let sequence =
        if factory_fields.iter().any(|f| f.sequenced) { "    let n = ocre::testing::sequence();\n" } else { "" };
    let (values_binding, values_end) = if attachment_values.is_empty() {
        ("", String::new())
    } else {
        ("let mut values = ", format!(";\n{attachment_values}        values"))
    };
    let self_binding = if creates_parents { "mut self" } else { "self" };
    format!(
        r#"//! Test data for {human_plural}. Generated by `{command}`.
//!
//! `{singular}()` has valid attributes, unique per call; change any of them
//! with `{model} {{ field: value, ..{singular}() }}`.

use ocre::serde_json::{{Value, json}};

/// A {lower}'s attributes for tests.
#[derive(Debug, Clone)]
pub struct {model} {{
{declarations}}}

/// Valid attributes; text fields are unique per call.
pub fn {singular}() -> {model} {{
{sequence}    {model} {{
{defaults}    }}
}}

impl {model} {{
    /// Creates the rows it references with their factories, when unset.
    pub fn with_parents({self_binding}) -> Self {{
{parents}        self
    }}

    /// Column values for `ocre::testing::insert`, parents created first.
    pub fn values(self) -> Vec<(&'static str, Value)> {{
        {values_binding}self.with_parents().column_values(){values_end}
    }}

    fn column_values(self) -> Vec<(&'static str, Value)> {{
        vec![
{values}        ]
    }}

    /// Inserts the row into the test database (one wrangler call, about a second); returns its id.
    pub fn insert(self) -> i64 {{
        ocre::testing::insert("{plural}", &self.values())
    }}

    /// The fields as the new and edit forms post them.
    pub fn form(self) -> Vec<(&'static str, String)> {{
        self.with_parents().column_values().into_iter().map(|(name, value)| (name, super::form_text(value))).collect()
    }}

    /// The attributes as a JSON API receives them.
    pub fn json(self) -> Value {{
        self.with_parents().json_value()
    }}

    fn json_value(self) -> Value {{
        json!({{
{json_fields}        }})
    }}
}}
"#,
        lower = names.human_singular.to_lowercase(),
    )
}

/// The first required plain-text field (`title`): shown on pages, and
/// "can't be blank" when empty, so the tests assert on it.
fn text_field(fields: &[Field]) -> Option<&str> {
    fields
        .iter()
        .find(|field| matches!(field.ty, FieldType::String | FieldType::Text) && !field.optional)
        .map(|field| field.name.as_str())
}

/// `tests/<plural>.rs`: request tests of the scaffold's pages. Forms with
/// files are multipart, so models with attachments get the read and delete tests only.
pub(super) fn scaffold_tests(
    edits: &mut Edits,
    names: &ModelNames,
    fields: &[Field],
    command: &str,
) -> Result<(), CliError> {
    let ModelNames { model, singular, plural, human_plural, human_singular } = names;
    let text = text_field(fields);
    let files = fields.iter().any(Field::is_attachment);
    let mut tests = format!(
        r#"//! Request tests for the {lower_plural} pages. Generated by `{command}`.
//! They run against the app in workerd with `ocre test --e2e`; plain
//! `cargo test` skips them (`#[ignore]`).

mod factories;

use factories::{singular}::{singular};
use ocre::testing::Client;

#[test]
{IGNORE}
fn lists_{plural}() {{
    Client::new().get("/{plural}").assert_status(200).assert_contains("{human_plural}");
}}

#[test]
{IGNORE}
fn shows_a_{singular}() {{
    let record = {singular}();
{keep_text}    let id = record.insert();
    Client::new().get(&format!("/{plural}/{{id}}")).assert_status(200){assert_text};
}}

#[test]
{IGNORE}
fn deletes_a_{singular}() {{
    let id = {singular}().insert();
    let mut client = Client::new();
    client.post(&format!("/{plural}/{{id}}/delete"), &()).assert_redirect_to("/{plural}");
    assert_eq!(client.flash("notice").as_deref(), Some("{human_singular} was successfully destroyed."));
    client.get(&format!("/{plural}/{{id}}")).assert_status(404);
}}
"#,
        lower_plural = human_plural.to_lowercase(),
        keep_text = text.map(|name| format!("    let text = record.{name}.clone();\n")).unwrap_or_default(),
        assert_text = if text.is_some() { ".assert_contains(&text)" } else { "" },
    );
    if !files {
        write!(
            tests,
            r#"
#[test]
{IGNORE}
fn creates_a_{singular}() {{
    let mut client = Client::new();
    let created = client.post("/{plural}", &{singular}().form());
    let location = created.assert_status(303).location().unwrap_or_default().to_owned();
    assert!(location.starts_with("/{plural}/"), "redirects to the new {lower}: {{location}}");
    assert_eq!(client.flash("notice").as_deref(), Some("{human_singular} was successfully created."));
    client.follow_redirect(&created).assert_status(200).assert_contains("{human_singular} was successfully created.");
}}

#[test]
{IGNORE}
fn updates_a_{singular}() {{
    let id = {singular}().insert();
    let changes = {singular}();
{keep_changed}    let mut client = Client::new();
    client.post(&format!("/{plural}/{{id}}"), &changes.form()).assert_redirect_to(&format!("/{plural}/{{id}}"));
    client.get(&format!("/{plural}/{{id}}")).assert_status(200){assert_changed};
}}
"#,
            lower = human_singular.to_lowercase(),
            keep_changed = text.map(|name| format!("    let text = changes.{name}.clone();\n")).unwrap_or_default(),
            assert_changed = if text.is_some() { ".assert_contains(&text)" } else { "" },
        )
        .expect("writing to a String");
        if let Some(name) = text {
            write!(
                tests,
                r#"
#[test]
{IGNORE}
fn rejects_an_invalid_{singular}() {{
    let invalid = factories::{singular}::{model} {{ {name}: String::new(), ..{singular}() }};
    Client::new().post("/{plural}", &invalid.form()).assert_status(422).assert_contains("can&#39;t be blank");
}}
"#
            )
            .expect("writing to a String");
        }
    }
    edits.create(&format!("tests/{plural}.rs"), tests)
}

/// `tests/api_<plural>.rs`: request tests of the JSON API (its attachments
/// are optional and uploaded separately, so JSON covers the rest).
pub(super) fn api_tests(
    edits: &mut Edits,
    names: &ModelNames,
    fields: &[Field],
    command: &str,
) -> Result<(), CliError> {
    let ModelNames { model, singular, plural, human_plural, .. } = names;
    let text = text_field(fields);
    let mut tests = format!(
        r#"//! Request tests for the {lower_plural} JSON API. Generated by `{command}`.
//! They run against the app in workerd with `ocre test --e2e`; plain
//! `cargo test` skips them (`#[ignore]`).

mod factories;

use factories::{singular}::{singular};
use ocre::{{serde_json::Value, testing::Client}};

#[test]
{IGNORE}
fn lists_{plural}() {{
    let list: Value = Client::new().get("/api/{plural}").assert_status(200).json();
    assert!(list.is_array(), "{{list}}");
}}

#[test]
{IGNORE}
fn shows_a_{singular}() {{
    let id = {singular}().insert();
    let shown: Value = Client::new().get(&format!("/api/{plural}/{{id}}")).assert_status(200).json();
    assert_eq!(shown["id"], id);
}}

#[test]
{IGNORE}
fn deletes_a_{singular}() {{
    let id = {singular}().insert();
    let mut client = Client::new();
    client.delete(&format!("/api/{plural}/{{id}}")).assert_status(204);
    client.get(&format!("/api/{plural}/{{id}}")).assert_status(404);
}}

#[test]
{IGNORE}
fn creates_a_{singular}() {{
    let mut client = Client::new();
    let created: Value = client.post_json("/api/{plural}", &{singular}().json()).assert_status(201).json();
    let id = created["id"].as_i64().expect("the new id");
    client.get(&format!("/api/{plural}/{{id}}")).assert_status(200);
}}

#[test]
{IGNORE}
fn updates_a_{singular}() {{
    let id = {singular}().insert();
    let changes = {singular}().json();
    let updated: Value =
        Client::new().patch_json(&format!("/api/{plural}/{{id}}"), &changes).assert_status(200).json();
    assert_eq!(updated["id"], id);
{assert_changed}}}
"#,
        lower_plural = human_plural.to_lowercase(),
        assert_changed =
            text.map(|name| format!("    assert_eq!(updated[\"{name}\"], changes[\"{name}\"]);\n")).unwrap_or_default(),
    );
    if let Some(name) = text {
        write!(
            tests,
            r#"
#[test]
{IGNORE}
fn rejects_an_invalid_{singular}() {{
    let invalid = factories::{singular}::{model} {{ {name}: String::new(), ..{singular}() }};
    Client::new().post_json("/api/{plural}", &invalid.json()).assert_status(422).assert_contains("{name}");
}}
"#
        )
        .expect("writing to a String");
    }
    edits.create(&format!("tests/api_{plural}.rs"), tests)
}

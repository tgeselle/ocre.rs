//! `ocre generate model`: the table, its migration and `src/models/<model>.rs`.

use std::fmt::Write as _;

use super::{
    Edits, MODULES_MARKER,
    fields::{Field, FieldType, parse_fields},
    insert_after_marker, next_migration_path,
};
use crate::{CliResult, names::ModelNames, output::CliError, project::Project};

pub(super) const MODELS_MARKER: &str = "// ocre:models";
pub(super) const ASSOCIATIONS_MARKER: &str = "// ocre:associations";

pub fn model(project: &Project, name: &str, specs: &[String]) -> CliResult {
    let names = ModelNames::parse(name)?;
    let fields = parse_fields(specs)?;
    let mut edits = Edits::new(project);
    add_model(&mut edits, &names, &fields, &format!("ocre g model {name} {}", specs.join(" ")))?;
    let mut report = edits.apply("generate model")?;
    report.next = vec!["ocre migrate".to_owned(), "cargo check --target wasm32-unknown-unknown".to_owned()];
    Ok(report)
}

/// Adds the model unless `src/models/<singular>.rs` exists (scaffold and api
/// reuse an existing model, e.g. `ocre g api Post` after `ocre g scaffold Post`).
pub(super) fn ensure_model(
    edits: &mut Edits,
    names: &ModelNames,
    fields: &[Field],
    command: &str,
) -> Result<(), CliError> {
    if edits.exists(&model_path(names)) { Ok(()) } else { add_model(edits, names, fields, command) }
}

fn model_path(names: &ModelNames) -> String {
    format!("src/models/{}.rs", names.singular)
}

fn add_model(edits: &mut Edits, names: &ModelNames, fields: &[Field], command: &str) -> Result<(), CliError> {
    for target in fields.iter().filter_map(|f| f.target.as_ref()) {
        let path = model_path(target);
        let source = edits.read(&path)?.ok_or_else(|| {
            CliError::new(format!("{path} does not exist"))
                .hint(format!("generate the referenced model first, e.g. `ocre g model {} name:string`", target.model))
        })?;
        let has_many = has_many_fn(names, target, fields);
        let updated = insert_after_marker(&source, ASSOCIATIONS_MARKER, &has_many).ok_or_else(|| {
            CliError::new(format!("{path} is missing the `{ASSOCIATIONS_MARKER}` marker"))
                .hint(format!("put `{ASSOCIATIONS_MARKER}` on its own line inside `impl {} {{ ... }}`", target.model))
        })?;
        edits.update(&path, updated);
    }
    let registry = match edits.read("src/models/mod.rs")? {
        Some(source) => source,
        None => {
            let lib = edits.read("src/lib.rs")?.unwrap_or_default();
            let lib = insert_after_marker(&lib, MODULES_MARKER, "mod models;").ok_or_else(|| {
                CliError::new("src/lib.rs is missing the `// ocre:modules` marker")
                    .hint("put `// ocre:modules` on its own line where `mod` declarations go")
            })?;
            edits.update("src/lib.rs", lib);
            format!(
                "//! Models: one module per table. `ocre g model` adds them below.\n\n// Each model exposes a complete API (find, find_many, count, associations);\n// an app rarely uses all of it.\n#![allow(dead_code)]\n\n{MODELS_MARKER}\n"
            )
        }
    };
    let registry = insert_after_marker(&registry, MODELS_MARKER, &format!("pub mod {};", names.singular))
        .ok_or_else(|| CliError::new(format!("src/models/mod.rs is missing the `{MODELS_MARKER}` marker")))?;
    edits.update("src/models/mod.rs", registry);
    if !edits.has_create_migration(&names.plural)? {
        let path = next_migration_path(edits, &format!("create_{}", names.plural))?;
        edits.create(&path, table_sql(&names.plural, fields))?;
    }
    edits.create(&model_path(names), model_rs(names, fields, command))
}

/// `CREATE TABLE` with the generated columns, plus its indexes.
pub(super) fn table_sql(table: &str, fields: &[Field]) -> String {
    let mut sql = format!("CREATE TABLE {table} (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n");
    for field in fields {
        writeln!(sql, "    {},", field.sql_column()).expect("writing to a String");
    }
    sql.push_str("    created_at TEXT NOT NULL DEFAULT (datetime('now')),\n");
    sql.push_str("    updated_at TEXT NOT NULL DEFAULT (datetime('now'))\n);\n");
    for field in fields {
        sql.push_str(&index_sql(table, field));
    }
    sql
}

/// `CREATE [UNIQUE] INDEX` for unique and reference columns.
pub(super) fn index_sql(table: &str, field: &Field) -> String {
    let name = &field.name;
    if field.unique {
        format!("CREATE UNIQUE INDEX index_{table}_on_{name} ON {table} ({name});\n")
    } else if field.target.is_some() {
        format!("CREATE INDEX index_{table}_on_{name} ON {table} ({name});\n")
    } else {
        String::new()
    }
}

/// Body of `validate()` for `NewX` (`changes == false`) or `XChanges`.
fn validate_body(fields: &[Field], changes: bool) -> String {
    let mut out = String::new();
    for field in fields {
        let name = &field.name;
        let text = field.ty.is_textual();
        let (open, value, close) = match (changes, field.optional) {
            (false, false) => (String::new(), if text { format!("&self.{name}") } else { format!("self.{name}") }, ""),
            (false, true) => {
                (format!("if let Some({name}) = &self.{name} {{\n            "), deref(text, name), "\n        }")
            }
            (true, false) => {
                (format!("if let Some({name}) = &self.{name} {{\n            "), deref(text, name), "\n        }")
            }
            (true, true) => {
                (format!("if let Some(Some({name})) = &self.{name} {{\n            "), deref(text, name), "\n        }")
            }
        };
        let checks = field.checks(&value);
        if !checks.is_empty() {
            writeln!(out, "        {open}{}{close}", checks.join("\n            ")).expect("writing to a String");
        }
    }
    out
}

fn deref(text: bool, name: &str) -> String {
    if text { name.to_owned() } else { format!("*{name}") }
}

/// Uniqueness and reference checks, which need the database.
fn database_checks(names: &ModelNames, fields: &[Field], changes: bool) -> String {
    let table = &names.plural;
    let source = if changes { "changes" } else { "new" };
    let mut out = String::new();
    for field in fields {
        let name = &field.name;
        let mut checks = Vec::new();
        let (value, exclude, exclude_param) = if field.ty.is_textual() {
            (name.clone(), if changes { " AND id != ?2" } else { "" }, if changes { ", id" } else { "" })
        } else {
            (format!("*{name}"), if changes { " AND id != ?2" } else { "" }, if changes { ", id" } else { "" })
        };
        if field.unique {
            checks.push(format!(
                "v.check(\"{name}\", db.exists(\"SELECT 1 FROM {table} WHERE {name} = ?1{exclude} LIMIT 1\", params![{value}{exclude_param}]).await?, \"has already been taken\");"
            ));
        }
        if let Some(target) = &field.target {
            checks.push(format!(
                "v.check(\"{name}\", !db.exists(\"SELECT 1 FROM {} WHERE id = ?1 LIMIT 1\", params![{value}]).await?, \"must exist\");",
                target.plural
            ));
        }
        if checks.is_empty() {
            continue;
        }
        let pattern = if changes && field.optional { format!("Some(Some({name}))") } else { format!("Some({name})") };
        let bind = if changes || field.optional {
            format!("{pattern} = &{source}.{name}")
        } else {
            format!("{name} = &{source}.{name}")
        };
        let keyword = if changes || field.optional { "if let" } else { "let" };
        if keyword == "let" {
            writeln!(out, "    {{\n        let {bind};\n        {}\n    }}", checks.join("\n        "))
                .expect("writing to a String");
        } else {
            writeln!(out, "    if let {bind} {{\n        {}\n    }}", checks.join("\n        "))
                .expect("writing to a String");
        }
    }
    out
}

fn model_rs(names: &ModelNames, fields: &[Field], command: &str) -> String {
    let ModelNames { model, plural, human_singular, human_plural, .. } = names;
    let lower = human_singular.to_lowercase();
    let lower_plural = human_plural.to_lowercase();
    let columns = fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ");
    let placeholders = (1..=fields.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let insert_params = fields.iter().map(|f| format!("new.{}", f.name)).collect::<Vec<_>>().join(", ");
    let sets = fields
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{0} = CASE WHEN ?{1} THEN ?{2} ELSE {0} END", f.name, 2 * i + 1, 2 * i + 2))
        .collect::<Vec<_>>()
        .join(", ");
    let update_id = 2 * fields.len() + 1;
    let update_params = fields
        .iter()
        .map(|f| {
            let name = &f.name;
            if f.optional {
                format!("changes.{name}.is_some(), changes.{name}.flatten()")
            } else {
                format!("changes.{name}.is_some(), changes.{name}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    let mut row_fields = String::new();
    let mut new_fields = String::new();
    let mut change_fields = String::new();
    for field in fields {
        let (name, ty, column) = (&field.name, field.rust_type(), field.column_type());
        if field.ty == FieldType::Boolean {
            row_fields.push_str("    #[serde(deserialize_with = \"ocre::bool_from_sql\")]\n");
            new_fields.push_str("    #[serde(default)]\n");
        }
        if field.optional {
            new_fields.push_str("    #[serde(default, deserialize_with = \"ocre::optional\")]\n");
            change_fields.push_str("    #[serde(default, deserialize_with = \"ocre::patch\")]\n");
            writeln!(change_fields, "    pub {name}: Option<Option<{ty}>>,").expect("writing to a String");
        } else {
            writeln!(change_fields, "    pub {name}: Option<{ty}>,").expect("writing to a String");
        }
        writeln!(row_fields, "    pub {name}: {column},").expect("writing to a String");
        writeln!(new_fields, "    pub {name}: {column},").expect("writing to a String");
    }
    let mut belongs_to = String::new();
    for field in fields {
        let Some(target) = &field.target else { continue };
        let (method, target_model, target_singular, name) =
            (field.name.trim_end_matches("_id"), &target.model, &target.singular, &field.name);
        let body = if field.optional {
            format!(
                "match self.{name} {{\n            Some(id) => crate::models::{target_singular}::find(ctx, id).await,\n            None => Ok(None),\n        }}"
            )
        } else {
            format!("crate::models::{target_singular}::find(ctx, self.{name}).await")
        };
        write!(
            belongs_to,
            "\n    /// The {} this {lower} belongs to.\n    pub async fn {method}(&self, ctx: &Ctx) -> Result<Option<crate::models::{target_singular}::{target_model}>> {{\n        {body}\n    }}\n",
            target.human_singular.to_lowercase(),
        )
        .expect("writing to a String");
    }
    let new_validation = validate_body(fields, false);
    let change_validation = validate_body(fields, true);
    let create_checks = database_checks(names, fields, false);
    let update_checks = database_checks(names, fields, true);
    let create_validation = if create_checks.is_empty() {
        "    new.validate().finish()?;\n".to_owned()
    } else {
        format!("    let mut v = new.validate();\n{create_checks}    v.finish()?;\n")
    };
    let update_validation = if update_checks.is_empty() {
        "    changes.validate().finish()?;\n".to_owned()
    } else {
        format!("    let mut v = changes.validate();\n{update_checks}    v.finish()?;\n")
    };

    format!(
        r#"//! {human_singular} model: the `{plural}` table. Generated by `{command}`.
//!
//! Every query and rule about {lower_plural} lives here. Controllers (HTML
//! pages, JSON API, GraphQL) call these functions instead of writing SQL.

use ocre::{{Ctx, Error, IntoParam, Page, Result, Validator, params}};
use serde::{{Deserialize, Serialize}};

/// A row of the `{plural}` table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct {model} {{
    pub id: i64,
{row_fields}    pub created_at: String,
    pub updated_at: String,
}}

/// Values for a new {lower}.
#[derive(Debug, Clone, Deserialize)]
pub struct New{model} {{
{new_fields}}}

/// Changes to a {lower}: absent fields keep their value; for optional fields
/// `Some(None)` clears it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct {model}Changes {{
{change_fields}}}

impl New{model} {{
    /// Checks that need no database; `create` adds uniqueness and references.
    pub fn validate(&self) -> Validator {{
        let mut v = Validator::new();
{new_validation}        v
    }}
}}

impl {model}Changes {{
    /// Checks the fields being changed; `update` adds the database checks.
    pub fn validate(&self) -> Validator {{
        let mut v = Validator::new();
{change_validation}        v
    }}
}}

impl {model} {{{belongs_to}
    {ASSOCIATIONS_MARKER}
}}

/// {human_plural}, newest first.
pub async fn all(ctx: &Ctx, page: Page) -> Result<Vec<{model}>> {{
    ctx.db()?.all("SELECT * FROM {plural} ORDER BY id DESC LIMIT ?1 OFFSET ?2", params![page.limit, page.offset]).await
}}

pub async fn count(ctx: &Ctx) -> Result<i64> {{
    #[derive(Deserialize)]
    struct Count {{
        count: i64,
    }}
    let row: Option<Count> = ctx.db()?.first("SELECT COUNT(*) AS count FROM {plural}", params![]).await?;
    Ok(row.map_or(0, |row| row.count))
}}

pub async fn find(ctx: &Ctx, id: i64) -> Result<Option<{model}>> {{
    ctx.db()?.first("SELECT * FROM {plural} WHERE id = ?1", params![id]).await
}}

/// Loads many {lower_plural} in few queries (100 ids per query, D1's limit on
/// parameters): use it instead of calling `find` in a loop.
pub async fn find_many(ctx: &Ctx, ids: &[i64]) -> Result<Vec<{model}>> {{
    let db = ctx.db()?;
    let mut rows = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(100) {{
        let placeholders = (1..=chunk.len()).map(|i| format!("?{{i}}")).collect::<Vec<_>>().join(", ");
        let sql = format!("SELECT * FROM {plural} WHERE id IN ({{placeholders}})");
        rows.extend(db.all::<{model}>(&sql, chunk.iter().map(|id| id.into_param()).collect()).await?);
    }}
    Ok(rows)
}}

pub async fn create(ctx: &Ctx, new: New{model}) -> Result<{model}> {{
    let db = ctx.db()?;
{create_validation}    db.first("INSERT INTO {plural} ({columns}) VALUES ({placeholders}) RETURNING *", params![{insert_params}])
        .await?
        .ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))
}}

/// `None` when there is no {lower} with this id.
pub async fn update(ctx: &Ctx, id: i64, changes: {model}Changes) -> Result<Option<{model}>> {{
    let db = ctx.db()?;
{update_validation}    db.first(
        "UPDATE {plural} SET {sets}, updated_at = datetime('now') WHERE id = ?{update_id} RETURNING *",
        params![{update_params}, id],
    )
    .await
}}

/// `false` when there is no {lower} with this id.
pub async fn delete(ctx: &Ctx, id: i64) -> Result<bool> {{
    Ok(ctx.db()?.execute("DELETE FROM {plural} WHERE id = ?1", params![id]).await? > 0)
}}
"#,
    )
}

/// `posts(ctx, page)` on the referenced model: the other side of `references`.
fn has_many_fn(names: &ModelNames, target: &ModelNames, fields: &[Field]) -> String {
    let column = fields.iter().find(|f| f.target.as_ref() == Some(target)).map(|f| f.name.as_str()).unwrap_or("id");
    let (model, singular, plural) = (&names.model, &names.singular, &names.plural);
    format!(
        "    /// {} of this {}, newest first.\n    pub async fn {plural}(&self, ctx: &Ctx, page: Page) -> Result<Vec<crate::models::{singular}::{model}>> {{\n        ctx.db()?\n            .all(\n                \"SELECT * FROM {plural} WHERE {column} = ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3\",\n                params![self.id, page.limit, page.offset],\n            )\n            .await\n    }}\n",
        names.human_plural,
        target.human_singular.to_lowercase(),
    )
}

//! `ocre generate model`: the table, its migration and `src/models/<model>.rs`.

use std::fmt::Write as _;

use super::{
    Edits, MODULES_MARKER,
    fields::{ATTACHMENT_TYPES, Field, FieldType, parse_fields},
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
    register_model(edits, &names.singular)?;
    if fields.iter().any(Field::is_attachment) {
        super::storage::ensure_bucket(edits)?;
    }
    if !edits.has_create_migration(&names.plural)? {
        let path = next_migration_path(edits, &format!("create_{}", names.plural))?;
        edits.create(&path, table_sql(&names.plural, fields))?;
    }
    edits.create(&model_path(names), model_rs(names, fields, command))
}

/// Adds `pub mod <module>;` to src/models/mod.rs, creating it (and `mod models;`
/// in src/lib.rs) for the first model.
pub(super) fn register_model(edits: &mut Edits, module: &str) -> Result<(), CliError> {
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
    let registry = insert_after_marker(&registry, MODELS_MARKER, &format!("pub mod {module};"))
        .ok_or_else(|| CliError::new(format!("src/models/mod.rs is missing the `{MODELS_MARKER}` marker")))?;
    edits.update("src/models/mod.rs", registry);
    Ok(())
}

/// `CREATE TABLE` with the generated columns, plus its indexes.
pub(super) fn table_sql(table: &str, fields: &[Field]) -> String {
    let mut sql = format!("CREATE TABLE {table} (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n");
    for field in fields {
        for column in field.sql_columns() {
            writeln!(sql, "    {column},").expect("writing to a String");
        }
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
        if field.is_attachment() {
            out.push_str(&attachment_checks(field, changes));
            continue;
        }
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

/// Presence (new, required) and `v.file` checks of an attachment's upload.
fn attachment_checks(field: &Field, changes: bool) -> String {
    let name = &field.name;
    let presence = if changes || field.optional {
        String::new()
    } else {
        format!("        v.check(\"{name}\", self.{name}.is_none(), \"can't be blank\");\n")
    };
    let pattern = if changes && field.optional { format!("Some(Some({name}))") } else { format!("Some({name})") };
    format!(
        "{presence}        if let {pattern} = &self.{name} {{\n            {}\n        }}\n",
        field.checks(name).join("")
    )
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
    let plain: Vec<&Field> = fields.iter().filter(|f| !f.is_attachment()).collect();
    let files: Vec<&Field> = fields.iter().filter(|f| f.is_attachment()).collect();
    // Attachment columns come after the plain ones, so their parameters can be
    // appended to `params![...]` with `storage::columns`.
    let column_names: Vec<String> = plain
        .iter()
        .map(|f| f.name.clone())
        .chain(files.iter().flat_map(|f| f.columns().into_iter().map(|(column, _)| column)))
        .collect();
    let columns = column_names.join(", ");
    let placeholders = (1..=column_names.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let insert_params = plain.iter().map(|f| format!("new.{}", f.name)).collect::<Vec<_>>().join(", ");
    let mut sets: Vec<String> = plain
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{0} = CASE WHEN ?{1} THEN ?{2} ELSE {0} END", f.name, 2 * i + 1, 2 * i + 2))
        .collect();
    // One flag per attachment decides its four columns.
    let mut next = 2 * plain.len() + 1;
    for file in &files {
        for (i, (column, _)) in file.columns().iter().enumerate() {
            sets.push(format!("{column} = CASE WHEN ?{next} THEN ?{} ELSE {column} END", next + 1 + i));
        }
        next += 5;
    }
    let sets = sets.join(", ");
    let update_id = next;
    let update_params = plain
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
        if field.is_attachment() {
            for (column, ty) in field.columns() {
                writeln!(row_fields, "    pub {column}: {ty},").expect("writing to a String");
            }
            let replaces = if field.optional {
                "`Some(Some(file))` replaces the stored file, `Some(None)` removes it"
            } else {
                "`Some(file)` replaces the stored file"
            };
            write!(
                new_fields,
                "    /// The file to store in R2 (checked against `{rules}`). JSON cannot carry it.\n    #[serde(skip)]\n    pub {name}: Option<Upload>,\n",
                rules = field.rules_const()
            )
            .expect("writing to a String");
            write!(
                change_fields,
                "    /// {replaces} (the old one is deleted from R2).\n    #[serde(skip)]\n    pub {name}: {},\n",
                if field.optional { "Option<Option<Upload>>" } else { "Option<Upload>" }
            )
            .expect("writing to a String");
            continue;
        }
        if field.ty == FieldType::Boolean {
            row_fields.push_str("    #[serde(deserialize_with = \"ocre::bool_from_sql\")]\n");
            new_fields.push_str("    #[serde(default)]\n");
        }
        let json = field.ty == FieldType::Json;
        if json {
            // D1 returns the column's JSON text; JSON bodies carry the value itself.
            let reader = if field.optional { "ocre::optional_json_from_sql" } else { "ocre::json_from_sql" };
            writeln!(row_fields, "    #[serde(deserialize_with = \"{reader}\")]").expect("writing to a String");
        }
        if field.optional {
            let (optional, patch) = if json {
                ("default", "ocre::patch_json")
            } else {
                ("default, deserialize_with = \"ocre::optional\"", "ocre::patch")
            };
            writeln!(new_fields, "    #[serde({optional})]").expect("writing to a String");
            writeln!(change_fields, "    #[serde(default, deserialize_with = \"{patch}\")]")
                .expect("writing to a String");
            writeln!(change_fields, "    pub {name}: Option<Option<{ty}>>,").expect("writing to a String");
        } else {
            writeln!(change_fields, "    pub {name}: Option<{ty}>,").expect("writing to a String");
        }
        writeln!(row_fields, "    pub {name}: {column},").expect("writing to a String");
        writeln!(new_fields, "    pub {name}: {column},").expect("writing to a String");
    }
    let mut methods = String::new();
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
            methods,
            "\n    /// The {} this {lower} belongs to.\n    pub async fn {method}(&self, ctx: &Ctx) -> Result<Option<crate::models::{target_singular}::{target_model}>> {{\n        {body}\n    }}\n",
            target.human_singular.to_lowercase(),
        )
        .expect("writing to a String");
    }
    for file in &files {
        methods.push_str(&attachment_fn(file, &lower));
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
    let insert_sql = format!("INSERT INTO {plural} ({columns}) VALUES ({placeholders}) RETURNING *");
    let update_sql =
        format!("UPDATE {plural} SET {sets}, updated_at = datetime('now') WHERE id = ?{update_id} RETURNING *");
    let (imports, rules, create_body, update_body, delete_body) = if files.is_empty() {
        (
            String::new(),
            String::new(),
            format!(
                "{create_validation}    db.first(\"{insert_sql}\", params![{insert_params}])\n        .await?\n        .ok_or_else(|| Error::internal(\"INSERT ... RETURNING returned no row\"))\n"
            ),
            format!(
                "{update_validation}    db.first(\n        \"{update_sql}\",\n        params![{update_params}, id],\n    )\n    .await\n"
            ),
            format!("    Ok(ctx.db()?.execute(\"DELETE FROM {plural} WHERE id = ?1\", params![id]).await? > 0)\n"),
        )
    } else {
        (
            ", storage::{self, Attachment, Rules, Upload}".to_owned(),
            files.iter().map(|f| rules_const(f)).collect(),
            create_with_files(&files, &create_validation, &insert_sql, &insert_params, &names.plural),
            update_with_files(&files, &update_validation, &update_sql, &update_params, &names.plural),
            format!(
                "    let deleted: Option<{model}> = ctx.db()?.first(\"DELETE FROM {plural} WHERE id = ?1 RETURNING *\", params![id]).await?;\n    let Some(record) = deleted else {{ return Ok(false) }};\n    storage::delete_attachments(ctx, &[{}]).await?;\n    Ok(true)\n",
                files.iter().map(|f| file_value(f, "record")).collect::<Vec<_>>().join(", ")
            ),
        )
    };

    format!(
        r#"//! {human_singular} model: the `{plural}` table. Generated by `{command}`.
//!
//! Every query and rule about {lower_plural} lives here. Controllers (HTML
//! pages, JSON API, GraphQL) call these functions instead of writing SQL.

use ocre::{{Ctx, Error, IntoParam, Page, Result, Validator, params{imports}}};
use serde::{{Deserialize, Serialize}};
{rules}
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

impl {model} {{{methods}
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
{create_body}}}

/// `None` when there is no {lower} with this id.
pub async fn update(ctx: &Ctx, id: i64, changes: {model}Changes) -> Result<Option<{model}>> {{
    let db = ctx.db()?;
{update_body}}}

/// `false` when there is no {lower} with this id.
pub async fn delete(ctx: &Ctx, id: i64) -> Result<bool> {{
{delete_body}}}
"#,
    )
}

/// `record.avatar()` wrapped in `Some`, or `record.doc()` (already an `Option`).
fn file_value(file: &Field, record: &str) -> String {
    if file.optional { format!("{record}.{}()", file.name) } else { format!("Some({record}.{}())", file.name) }
}

/// `pub const AVATAR: Rules = ...`: what the attachment accepts.
fn rules_const(file: &Field) -> String {
    let types = ATTACHMENT_TYPES.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(", ");
    format!(
        "\n/// Files `{name}` accepts; `validate()` checks each upload before anything is\n/// stored. The whole request is held in the Worker's memory (128 MB): keep\n/// `max_bytes` modest, and forms add it to their request limit.\npub const {rules}: Rules = Rules {{\n    max_bytes: 10 * 1024 * 1024,\n    content_types: &[{types}],\n}};\n",
        name = file.name,
        rules = file.rules_const(),
    )
}

/// `post.avatar()`: the stored file's `Attachment`, built from its columns.
fn attachment_fn(file: &Field, lower: &str) -> String {
    let name = &file.name;
    if file.optional {
        format!(
            "\n    /// This {lower}'s {name} file, if any; serve it with `ocre::storage::serve`.\n    pub fn {name}(&self) -> Option<Attachment> {{\n        Some(Attachment {{\n            key: self.{name}_key.clone()?,\n            filename: self.{name}_filename.clone()?,\n            content_type: self.{name}_content_type.clone()?,\n            size: self.{name}_size?,\n        }})\n    }}\n"
        )
    } else {
        format!(
            "\n    /// This {lower}'s {name} file; serve it with `ocre::storage::serve`.\n    pub fn {name}(&self) -> Attachment {{\n        Attachment {{\n            key: self.{name}_key.clone(),\n            filename: self.{name}_filename.clone(),\n            content_type: self.{name}_content_type.clone(),\n            size: self.{name}_size,\n        }}\n    }}\n"
        )
    }
}

/// Stores `source.<name>` (an `Option<Upload>`) in R2 as `Option<Attachment>`.
fn store_file(file: &Field, source: &str, table: &str) -> String {
    let name = &file.name;
    format!(
        "    let {name} = match {source}.{name} {{\n        Some(upload) => Some(storage::store(ctx, \"{table}/{name}\", upload).await?),\n        None => None,\n    }};\n"
    )
}

/// `create` for a model with attachments: files go to R2 once the values
/// are valid, and are deleted again when the INSERT fails.
fn create_with_files(files: &[&Field], validation: &str, sql: &str, params: &str, table: &str) -> String {
    let mut out = format!(
        "{validation}    // Files go to R2 once the values are valid; they are deleted again if the INSERT fails.\n"
    );
    for file in files {
        out.push_str(&store_file(file, "new", table));
    }
    writeln!(out, "    let mut params = params![{params}];").expect("writing to a String");
    for file in files {
        writeln!(out, "    params.extend(storage::columns({}.as_ref()));", file.name).expect("writing to a String");
    }
    let stored = files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ");
    write!(
        out,
        "    let created = db.first(\"{sql}\", params).await;\n    if !matches!(created, Ok(Some(_))) {{\n        storage::delete_attachments(ctx, &[{stored}]).await?;\n    }}\n    created?.ok_or_else(|| Error::internal(\"INSERT ... RETURNING returned no row\"))\n"
    )
    .expect("writing to a String");
    out
}

/// `update` for a model with attachments: new files go to R2 first; the
/// replaced ones are deleted after the UPDATE (the new ones if it fails).
fn update_with_files(files: &[&Field], validation: &str, sql: &str, params: &str, table: &str) -> String {
    let mut out = format!(
        "{validation}    // The current files: deleted from R2 once the row no longer points to them.\n    let Some(old) = find(ctx, id).await? else {{ return Ok(None) }};\n"
    );
    let mut replaced = Vec::new();
    let mut added = Vec::new();
    for file in files {
        let name = &file.name;
        if file.optional {
            write!(
                out,
                "    let {name} = match changes.{name} {{\n        Some(Some(upload)) => Some(Some(storage::store(ctx, \"{table}/{name}\", upload).await?)),\n        Some(None) => Some(None),\n        None => None,\n    }};\n"
            )
            .expect("writing to a String");
            replaced.push(format!("{name}.as_ref().and_then(|_| old.{name}())"));
            added.push(format!("{name}.flatten()"));
        } else {
            out.push_str(&store_file(file, "changes", table));
            replaced.push(format!("{name}.as_ref().map(|_| old.{name}())"));
            added.push(name.clone());
        }
    }
    writeln!(out, "    let mut params = params![{params}];").expect("writing to a String");
    for file in files {
        let change = if file.optional { "map(Option::as_ref)" } else { "map(Some)" };
        writeln!(out, "    params.extend(storage::column_changes({}.as_ref().{change}));", file.name)
            .expect("writing to a String");
    }
    write!(
        out,
        "    params.push(id.into_param());\n    let updated = db.first(\"{sql}\", params).await;\n    let unused = if matches!(updated, Ok(Some(_))) {{ [{}] }} else {{ [{}] }};\n    storage::delete_attachments(ctx, &unused).await?;\n    updated\n",
        replaced.join(", "),
        added.join(", ")
    )
    .expect("writing to a String");
    out
}

/// `posts(ctx, page)` on the referenced model: the other side of `references`.
fn has_many_fn(names: &ModelNames, target: &ModelNames, fields: &[Field]) -> String {
    let column = fields.iter().find(|f| f.target.as_ref() == Some(target)).map(|f| f.name.as_str()).unwrap_or("id");
    let (model, singular, plural) = (&names.model, &names.singular, &names.plural);
    format!(
        "    /// {} of this {}, newest first.\n    pub async fn {plural}(&self, ctx: &Ctx, page: ocre::Page) -> Result<Vec<crate::models::{singular}::{model}>> {{\n        ctx.db()?\n            .all(\n                \"SELECT * FROM {plural} WHERE {column} = ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3\",\n                params![self.id, page.limit, page.offset],\n            )\n            .await\n    }}\n",
        names.human_plural,
        target.human_singular.to_lowercase(),
    )
}

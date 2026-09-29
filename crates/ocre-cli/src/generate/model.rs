//! `ocre generate model`: the table, its migration and `src/models/<model>.rs`.

use std::fmt::Write as _;

use super::{
    Edits, MODULES_MARKER,
    fields::{ATTACHMENT_TYPES, Enumeration, Field, FieldType, parse_fields},
    insert_after_marker, next_migration_path,
};
use crate::{CliResult, names::ModelNames, output::CliError, project::Project};

pub(super) const MODELS_MARKER: &str = "// ocre:models";
pub(super) const ASSOCIATIONS_MARKER: &str = "// ocre:associations";

/// Names a model file already uses: an enum field cannot take them.
const TAKEN_TYPE_NAMES: &[&str] = &[
    "Attachment",
    "Ctx",
    "Deserialize",
    "Error",
    "Page",
    "Query",
    "Result",
    "Rules",
    "Serialize",
    "Upload",
    "Validator",
];

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
    for field in fields {
        let Some(enumeration) = &field.enumeration else { continue };
        let own = [names.model.clone(), format!("New{}", names.model), format!("{}Changes", names.model)];
        let type_name = &enumeration.type_name;
        if TAKEN_TYPE_NAMES.contains(&type_name.as_str()) || own.contains(type_name) {
            return Err(CliError::new(format!(
                "enum `{}` would be named `{type_name}`, a name the model already uses",
                field.name
            ))
            .hint(format!("rename the field, e.g. `{}_kind:enum:...`", field.name)));
        }
    }
    let references: Vec<&Field> = fields.iter().filter(|f| f.target.is_some()).collect();
    for field in &references {
        let target = field.target.as_ref().expect("references have a target");
        let path = model_path(target);
        let source = edits.read(&path)?.ok_or_else(|| {
            CliError::new(format!("{path} does not exist"))
                .hint(format!("generate the referenced model first, e.g. `ocre g model {} name:string`", target.model))
        })?;
        let mut code = if field.unique { has_one_fn(names, target, field) } else { has_many_fn(names, target, field) };
        // A join model (two references or more): each side reaches the others through it.
        for other in references.iter().filter(|other| other.name != field.name) {
            code.push_str(&has_many_through_fn(names, field, other));
        }
        let updated = insert_after_marker(&source, ASSOCIATIONS_MARKER, &code).ok_or_else(|| {
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
    if let Some(columns) = join_columns(fields) {
        let name = columns.join("_and_");
        writeln!(sql, "CREATE UNIQUE INDEX index_{table}_on_{name} ON {table} ({});", columns.join(", "))
            .expect("writing to a String");
    }
    sql
}

/// The reference columns of a join model: two `references` fields or more and
/// nothing else (`Tagging post:references tag:references`), linked at most once.
fn join_columns(fields: &[Field]) -> Option<Vec<&str>> {
    let join = fields.len() >= 2 && fields.iter().all(|f| f.target.is_some() && !f.unique);
    join.then(|| fields.iter().map(|f| f.name.as_str()).collect())
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
    let validator = |checks: String| {
        if checks.is_empty() {
            "        Validator::new()".to_owned()
        } else {
            format!("        let mut v = Validator::new();\n{checks}        v")
        }
    };
    let new_validation = validator(validate_body(fields, false));
    let change_validation = validator(validate_body(fields, true));
    let mut create_checks = database_checks(names, fields, false);
    if let Some(columns) = join_columns(fields) {
        let conditions: String = columns.iter().map(|c| format!(".eq(\"{c}\", new.{c})")).collect();
        let last = columns.last().expect("a join has two columns or more");
        writeln!(
            create_checks,
            "    v.check(\"{last}\", query(){conditions}.exists(&db).await?, \"has already been taken\");"
        )
        .expect("writing to a String");
    }
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
    let delete_query = format!(
        "    let deleted: Option<{model}> = ctx.db()?.first(\"DELETE FROM {plural} WHERE id = ?1 RETURNING *\", params![id]).await?;\n    let Some(record) = deleted else {{ return Ok(false) }};\n"
    );
    let (imports, rules, create_body, update_body, delete_body) = if files.is_empty() {
        (
            String::new(),
            String::new(),
            format!(
                "{create_validation}    let record: {model} = db\n        .first(\"{insert_sql}\", params![{insert_params}])\n        .await?\n        .ok_or_else(|| Error::internal(\"INSERT ... RETURNING returned no row\"))?;\n"
            ),
            format!(
                "{update_validation}    let updated: Option<{model}> = db\n        .first(\n            \"{update_sql}\",\n            params![{update_params}, id],\n        )\n        .await?;\n"
            ),
            delete_query,
        )
    } else {
        (
            ", storage::{self, Attachment, Rules, Upload}".to_owned(),
            files.iter().map(|f| rules_const(f)).collect(),
            create_with_files(&files, &create_validation, &insert_sql, &insert_params, names),
            update_with_files(&files, &update_validation, &update_sql, &update_params, names),
            format!(
                "{delete_query}    storage::delete_attachments(ctx, &[{}]).await?;\n",
                files.iter().map(|f| file_value(f, "record")).collect::<Vec<_>>().join(", ")
            ),
        )
    };
    let into_param = if files.is_empty() { "" } else { "IntoParam, " };
    let preloads: String = fields.iter().filter(|f| f.target.is_some()).map(|f| preload_fns(names, f)).collect();
    let singular = &names.singular;
    let enums: String = fields.iter().filter_map(|f| Some(enum_rs(&f.name, f.enumeration.as_ref()?))).collect();

    format!(
        r#"//! {human_singular} model: the `{plural}` table. Generated by `{command}`.
//!
//! Every query and rule about {lower_plural} lives here. Controllers (HTML
//! pages, JSON API, GraphQL) call these functions instead of writing SQL.

use ocre::{{Ctx, Error, {into_param}Page, Query, Result, Validator, params{imports}}};
use serde::{{Deserialize, Serialize}};
{enums}{rules}
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
{new_validation}
    }}
}}

impl {model}Changes {{
    /// Checks the fields being changed; `update` adds the database checks.
    pub fn validate(&self) -> Validator {{
{change_validation}
    }}
}}

impl {model} {{{methods}
    {ASSOCIATIONS_MARKER}
}}

/// Every query on {lower_plural} starts here, e.g.
/// `query().eq("id", id).first(&ctx.db()?)`. Scopes are functions taking and
/// returning a `Query<{model}>`; add them below.
pub fn query() -> Query<{model}> {{
    Query::table("{plural}")
}}

/// {human_plural}, newest first.
pub async fn all(ctx: &Ctx, page: Page) -> Result<Vec<{model}>> {{
    query().order_desc("id").page(page).all(&ctx.db()?).await
}}

pub async fn count(ctx: &Ctx) -> Result<i64> {{
    query().count(&ctx.db()?).await
}}

pub async fn find(ctx: &Ctx, id: i64) -> Result<Option<{model}>> {{
    query().eq("id", id).first(&ctx.db()?).await
}}

/// Loads many {lower_plural} in few queries (100 ids per query, D1's limit on
/// parameters): use it instead of calling `find` in a loop.
pub async fn find_many(ctx: &Ctx, ids: &[i64]) -> Result<Vec<{model}>> {{
    let db = ctx.db()?;
    let mut rows = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(100) {{
        rows.extend(query().is_in("id", chunk.iter().copied()).all(&db).await?);
    }}
    Ok(rows)
}}
{preloads}
pub async fn create(ctx: &Ctx, mut new: New{model}) -> Result<{model}> {{
    before_create(ctx, &mut new).await?;
    let db = ctx.db()?;
{create_body}    after_create(ctx, &record).await?;
    Ok(record)
}}

/// `None` when there is no {lower} with this id.
pub async fn update(ctx: &Ctx, id: i64, mut changes: {model}Changes) -> Result<Option<{model}>> {{
    before_update(ctx, id, &mut changes).await?;
    let db = ctx.db()?;
{update_body}    if let Some(record) = &updated {{
        after_update(ctx, record).await?;
    }}
    Ok(updated)
}}

/// `false` when there is no {lower} with this id.
pub async fn delete(ctx: &Ctx, id: i64) -> Result<bool> {{
    before_delete(ctx, id).await?;
{delete_body}    after_delete(ctx, &record).await?;
    Ok(true)
}}

// Callbacks: `create`, `update` and `delete` call these, so every controller
// gets them. Return an error to stop the operation. D1 keeps no transaction
// open between queries: an `after_*` error does not undo the write, so put
// writes that must succeed together in one `ctx.db()?.batch(...)`.

/// Before validation and the INSERT: normalize or fill in values.
async fn before_create(_ctx: &Ctx, _new: &mut New{model}) -> Result<()> {{
    Ok(())
}}

/// After the INSERT: send email, enqueue jobs, update counters.
async fn after_create(_ctx: &Ctx, _{singular}: &{model}) -> Result<()> {{
    Ok(())
}}

/// Before validation and the UPDATE of the {lower} `id`.
async fn before_update(_ctx: &Ctx, _id: i64, _changes: &mut {model}Changes) -> Result<()> {{
    Ok(())
}}

/// After a successful UPDATE.
async fn after_update(_ctx: &Ctx, _{singular}: &{model}) -> Result<()> {{
    Ok(())
}}

/// Before the DELETE of the {lower} `id`: return an error to keep it.
async fn before_delete(_ctx: &Ctx, _id: i64) -> Result<()> {{
    Ok(())
}}

/// After the DELETE, with the deleted row.
async fn after_delete(_ctx: &Ctx, _{singular}: &{model}) -> Result<()> {{
    Ok(())
}}
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
fn create_with_files(files: &[&Field], validation: &str, sql: &str, params: &str, names: &ModelNames) -> String {
    let (table, model) = (&names.plural, &names.model);
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
        "    let created: Result<Option<{model}>> = db.first(\"{sql}\", params).await;\n    if !matches!(created, Ok(Some(_))) {{\n        storage::delete_attachments(ctx, &[{stored}]).await?;\n    }}\n    let record = created?.ok_or_else(|| Error::internal(\"INSERT ... RETURNING returned no row\"))?;\n"
    )
    .expect("writing to a String");
    out
}

/// `update` for a model with attachments: new files go to R2 first; the
/// replaced ones are deleted after the UPDATE (the new ones if it fails).
fn update_with_files(files: &[&Field], validation: &str, sql: &str, params: &str, names: &ModelNames) -> String {
    let (table, model) = (&names.plural, &names.model);
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
        "    params.push(id.into_param());\n    let updated: Result<Option<{model}>> = db.first(\"{sql}\", params).await;\n    let unused = if matches!(updated, Ok(Some(_))) {{ [{}] }} else {{ [{}] }};\n    storage::delete_attachments(ctx, &unused).await?;\n    let updated = updated?;\n",
        replaced.join(", "),
        added.join(", ")
    )
    .expect("writing to a String");
    out
}

/// `comments(ctx, page)` on the referenced model: the has-many side of `references`.
fn has_many_fn(names: &ModelNames, target: &ModelNames, field: &Field) -> String {
    let (model, singular, plural, column) = (&names.model, &names.singular, &names.plural, &field.name);
    format!(
        "    /// {} of this {}, newest first.\n    pub async fn {plural}(&self, ctx: &Ctx, page: ocre::Page) -> Result<Vec<crate::models::{singular}::{model}>> {{\n        ctx.db()?\n            .all(\n                \"SELECT * FROM {plural} WHERE {column} = ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3\",\n                params![self.id, page.limit, page.offset],\n            )\n            .await\n    }}\n",
        names.human_plural,
        target.human_singular.to_lowercase(),
    )
}

/// `profile(ctx)` on the referenced model, for a unique reference (`user:references^`): has one.
fn has_one_fn(names: &ModelNames, target: &ModelNames, field: &Field) -> String {
    let (model, singular, column) = (&names.model, &names.singular, &field.name);
    format!(
        "    /// The {} of this {}, if any.\n    pub async fn {singular}(&self, ctx: &Ctx) -> Result<Option<crate::models::{singular}::{model}>> {{\n        crate::models::{singular}::query().eq(\"{column}\", self.id).first(&ctx.db()?).await\n    }}\n",
        names.human_singular.to_lowercase(),
        target.human_singular.to_lowercase(),
    )
}

/// `tags(ctx, page)` on `Post` for the join model `Tagging post:references
/// tag:references`: the `other` side of the join, through it.
fn has_many_through_fn(join: &ModelNames, field: &Field, other: &Field) -> String {
    let target = field.target.as_ref().expect("references have a target");
    let far = other.target.as_ref().expect("references have a target");
    let (join_table, column, other_column) = (&join.plural, &field.name, &other.name);
    let (far_table, far_singular, far_model) = (&far.plural, &far.singular, &far.model);
    format!(
        "    /// {} of this {}, through {join_table}, most recently linked first.\n    pub async fn {far_table}(&self, ctx: &Ctx, page: ocre::Page) -> Result<Vec<crate::models::{far_singular}::{far_model}>> {{\n        ctx.db()?\n            .all(\n                \"SELECT {far_table}.* FROM {far_table} JOIN {join_table} ON {join_table}.{other_column} = {far_table}.id WHERE {join_table}.{column} = ?1 ORDER BY {join_table}.id DESC LIMIT ?2 OFFSET ?3\",\n                params![self.id, page.limit, page.offset],\n            )\n            .await\n    }}\n",
        far.human_plural,
        target.human_singular.to_lowercase(),
    )
}

/// Eager loading for a `references` field, in the model that holds it:
/// `preload_<targets>` (belongs-to side) and `for_<targets>` (has-many side).
fn preload_fns(names: &ModelNames, field: &Field) -> String {
    let target = field.target.as_ref().expect("references have a target");
    let (model, lower_plural) = (&names.model, names.human_plural.to_lowercase());
    let (target_plural, target_singular, target_model) = (&target.plural, &target.singular, &target.model);
    let column = &field.name;
    let ids = if field.optional {
        format!("records.iter().filter_map(|record| record.{column}).collect()")
    } else {
        format!("records.iter().map(|record| record.{column}).collect()")
    };
    format!(
        r#"
/// The {target_lower_plural} of `records` by id, in one query per 100 ids: show a
/// list of {lower_plural} with their {target_lower} without one query per row.
pub async fn preload_{target_plural}(
    ctx: &Ctx,
    records: &[{model}],
) -> Result<std::collections::HashMap<i64, crate::models::{target_singular}::{target_model}>> {{
    let mut ids: Vec<i64> = {ids};
    ids.sort_unstable();
    ids.dedup();
    let rows = crate::models::{target_singular}::find_many(ctx, &ids).await?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}}

/// Every {lower} of the given {target_lower_plural}, newest first, in one query per
/// 100 ids: the has-many side preloaded for a list. Not paginated: keep the
/// list of ids short.
pub async fn for_{target_plural}(ctx: &Ctx, {column}s: &[i64]) -> Result<Vec<{model}>> {{
    let db = ctx.db()?;
    let mut rows = Vec::new();
    for chunk in {column}s.chunks(100) {{
        rows.extend(query().is_in("{column}", chunk.iter().copied()).order_desc("id").all(&db).await?);
    }}
    Ok(rows)
}}
"#,
        target_lower_plural = target.human_plural.to_lowercase(),
        target_lower = target.human_singular.to_lowercase(),
        lower = names.human_singular.to_lowercase(),
    )
}

/// The Rust enum of `status:enum:draft,published`: stored as its text, with
/// `ALL`, `as_str`, `Display`, `FromStr` and `IntoParam`.
fn enum_rs(field: &str, enumeration: &Enumeration) -> String {
    let name = &enumeration.type_name;
    let mut variants = String::new();
    let mut texts = String::new();
    for (i, value) in enumeration.values.iter().enumerate() {
        let variant = Enumeration::variant(value);
        let default = if i == 0 { "    #[default]\n" } else { "" };
        write!(variants, "{default}    #[serde(rename = \"{value}\")]\n    {variant},\n").expect("writing to a String");
        writeln!(texts, "            Self::{variant} => \"{value}\",").expect("writing to a String");
    }
    let all = enumeration.values.iter().map(|v| format!("Self::{}", Enumeration::variant(v))).collect::<Vec<_>>();
    format!(
        r#"
/// Values of `{field}`, stored as their text (a `CHECK` in the migration
/// refuses others). Add a value: a variant here and a migration rebuilding
/// the `CHECK` (`ocre g migration rebuild_<table>`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum {name} {{
{variants}}}

impl {name} {{
    /// Every value, in declaration order (select boxes, filters).
    pub const ALL: [Self; {count}] = [{all}];

    /// The stored text.
    pub fn as_str(self) -> &'static str {{
        match self {{
{texts}        }}
    }}
}}

impl std::fmt::Display for {name} {{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{
        f.write_str(self.as_str())
    }}
}}

impl std::str::FromStr for {name} {{
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, String> {{
        Self::ALL.into_iter().find(|value| value.as_str() == text).ok_or_else(|| format!("unknown value `{{text}}`"))
    }}
}}

impl ocre::IntoParam for {name} {{
    fn into_param(self) -> ocre::Param {{
        ocre::IntoParam::into_param(self.as_str())
    }}
}}
"#,
        count = enumeration.values.len(),
        all = all.join(", "),
    )
}

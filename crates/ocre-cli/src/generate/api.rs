//! `ocre generate api`: JSON REST resource, plus GraphQL with `--graphql`.

use std::{fmt::Write as _, path::PathBuf};

use super::{Field, FieldType, create_table_migration, parse_fields, register_module};
use crate::{
    CliResult,
    names::ModelNames,
    output::{CliError, Report},
    project::Project,
};

const QUERIES_MARKER: &str = "// ocre:graphql-queries";
const MUTATIONS_MARKER: &str = "// ocre:graphql-mutations";
const GRAPHQL_DEP: &str = r#"async-graphql = { version = "7.2.1", default-features = false, features = ["graphiql", "custom-error-conversion"] }"#;

pub fn api(project: &Project, name: &str, field_specs: &[String], graphql: bool) -> CliResult {
    let names = ModelNames::parse(name)?;
    let fields = parse_fields(field_specs)?;
    let module = format!("{}_api", names.plural);
    let module_path = project.root.join("src").join(format!("{module}.rs"));
    if module_path.exists() {
        return Err(CliError::new(format!("{} already exists", project.relative(&module_path))).hint(
            "api creates new resources only; edit the existing module, or add a migration with `ocre g migration`",
        ));
    }

    // Compute every change before writing, so a failure leaves nothing half-done.
    let lib_path = project.root.join("src/lib.rs");
    let mut lib = register_module(&std::fs::read_to_string(&lib_path)?, &module)?;
    let mut files: Vec<(PathBuf, String)> = create_table_migration(project, &names, &fields)?.into_iter().collect();
    let command = format!("ocre g api {name} {}{}", field_specs.join(" "), if graphql { " --graphql" } else { "" });
    files.push((module_path, module_rs(&names, &fields, &command, graphql)));
    let mut updated = vec![(lib_path.clone(), String::new())];
    if graphql {
        let cargo_path = project.root.join("Cargo.toml");
        updated.push((cargo_path.clone(), with_graphql(&std::fs::read_to_string(&cargo_path)?)?));
        let schema_path = project.root.join("src/graphql.rs");
        if schema_path.exists() {
            let schema = add_to_schema(&std::fs::read_to_string(&schema_path)?, &module, &names)?;
            updated.push((schema_path, schema));
        } else {
            lib = register_module(&lib, "graphql")?;
            files.push((schema_path, new_schema(&module, &names)));
        }
    }
    updated[0].1 = lib;

    let mut report = Report::new("generate api");
    for (path, contents) in files {
        std::fs::write(&path, contents)?;
        report.created.push(project.relative(&path));
    }
    for (path, contents) in updated {
        std::fs::write(&path, contents)?;
        report.updated.push(project.relative(&path));
    }
    report.next = vec![
        "ocre migrate".to_owned(),
        "ocre dev".to_owned(),
        format!("curl http://localhost:8787/api/{}", names.plural),
    ];
    if graphql {
        report.next.push("open http://localhost:8787/graphql".to_owned());
    }
    Ok(report)
}

/// Turns on Ocre's `graphql` feature and adds the async-graphql dependency.
fn with_graphql(cargo_toml: &str) -> Result<String, CliError> {
    let mut out = String::with_capacity(cargo_toml.len() + GRAPHQL_DEP.len() + 32);
    let mut found = false;
    let has_dep = cargo_toml.lines().any(|line| line.starts_with("async-graphql"));
    for line in cargo_toml.lines() {
        if line.starts_with("ocre = {") && line.ends_with('}') {
            found = true;
            if line.contains("\"graphql\"") {
                out.push_str(line);
            } else if let Some((before, after)) = line.split_once("features = [") {
                write!(out, "{before}features = [\"graphql\", {after}").expect("writing to a String");
            } else {
                write!(out, "{}, features = [\"graphql\"] }}", line.trim_end_matches('}').trim_end())
                    .expect("writing to a String");
            }
            out.push('\n');
            if !has_dep {
                out.push_str(GRAPHQL_DEP);
                out.push('\n');
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if found {
        Ok(out)
    } else {
        Err(CliError::new("Cargo.toml has no one-line `ocre = { ... }` dependency")
            .hint("declare Ocre as `ocre = { ... }` on one line under [dependencies], then run the command again"))
    }
}

fn new_schema(module: &str, names: &ModelNames) -> String {
    let model = &names.model;
    format!(
        r#"//! GraphQL schema: `POST /graphql`, and GraphiQL on `GET /graphql`.
//! `ocre g api <Model> ... --graphql` adds each resource below the markers.

use std::sync::LazyLock;

use async_graphql::{{EmptySubscription, MergedObject, Schema}};
use axum::Router;
use ocre::Ctx;

#[derive(MergedObject, Default)]
pub struct Query(
    {QUERIES_MARKER}
    crate::{module}::{model}Query,
);

#[derive(MergedObject, Default)]
pub struct Mutation(
    {MUTATIONS_MARKER}
    crate::{module}::{model}Mutation,
);

pub type AppSchema = Schema<Query, Mutation, EmptySubscription>;

/// Built once per Worker instance: building it per request would cost CPU.
static SCHEMA: LazyLock<AppSchema> =
    LazyLock::new(|| Schema::build(Query::default(), Mutation::default(), EmptySubscription).finish());

pub fn routes() -> Router<Ctx> {{
    ocre::graphql::routes(|| LazyLock::force(&SCHEMA))
}}
"#
    )
}

fn add_to_schema(schema: &str, module: &str, names: &ModelNames) -> Result<String, CliError> {
    let model = &names.model;
    let mut out = String::with_capacity(schema.len() + 128);
    let (mut queries, mut mutations) = (false, false);
    for line in schema.lines() {
        out.push_str(line);
        out.push('\n');
        let indent = &line[..line.len() - line.trim_start().len()];
        match line.trim() {
            QUERIES_MARKER => {
                queries = true;
                writeln!(out, "{indent}crate::{module}::{model}Query,").expect("writing to a String");
            }
            MUTATIONS_MARKER => {
                mutations = true;
                writeln!(out, "{indent}crate::{module}::{model}Mutation,").expect("writing to a String");
            }
            _ => {}
        }
    }
    if queries && mutations {
        Ok(out)
    } else {
        Err(CliError::new(
            "src/graphql.rs is missing the `// ocre:graphql-queries` or `// ocre:graphql-mutations` marker",
        )
        .hint("put each marker on its own line inside the `Query(...)` and `Mutation(...)` tuple structs"))
    }
}

/// Validation shared by the create and update inputs. `optional` fields are
/// `Option<T>` (PATCH): only present values are checked.
fn checks(fields: &[Field], optional: bool) -> String {
    let mut out = String::new();
    for field in fields {
        let (name, label) = (&field.name, field.label());
        let condition = match field.ty {
            FieldType::String | FieldType::Text => format!("{name}.trim().is_empty()"),
            FieldType::Integer => format!("{name}.unsigned_abs() > ocre::MAX_SAFE_INTEGER as u64"),
            FieldType::Float | FieldType::Boolean => continue,
        };
        let message = if field.ty.is_textual() { "is required" } else { "is out of range" };
        let (open, value) = if optional {
            (format!("if let Some({name}) = &self.{name} {{\n            if "), "")
        } else {
            ("if self.".to_owned(), "")
        };
        let close = if optional { "\n        }" } else { "" };
        let inner_indent = if optional { "                " } else { "            " };
        let end_indent = if optional { "            " } else { "        " };
        writeln!(
            out,
            "        {open}{value}{condition} {{\n{inner_indent}return Err(Error::bad_request(\"{label} {message}.\"));\n{end_indent}}}{close}"
        )
        .expect("writing to a String");
    }
    out
}

fn module_rs(names: &ModelNames, fields: &[Field], command: &str, graphql: bool) -> String {
    let ModelNames { model, singular, plural, human_plural, .. } = names;
    let columns = fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ");
    let placeholders = (1..=fields.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let coalesce = fields
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{0} = COALESCE(?{1}, {0})", f.name, i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let id_placeholder = fields.len() + 1;
    let input_params = fields.iter().map(|f| format!("input.{}", f.name)).collect::<Vec<_>>().join(", ");
    let change_params = fields.iter().map(|f| format!("changes.{}", f.name)).collect::<Vec<_>>().join(", ");

    let (gql_object, gql_input) = if graphql { (", SimpleObject", ", InputObject") } else { ("", "") };
    let mut model_fields = String::new();
    let mut new_fields = String::new();
    let mut change_fields = String::new();
    for field in fields {
        let (name, ty) = (&field.name, field.ty.rust_type());
        if field.ty == FieldType::Boolean {
            model_fields.push_str("    #[serde(deserialize_with = \"ocre::bool_from_sql\")]\n");
            new_fields.push_str("    #[serde(default)]\n");
            if graphql {
                new_fields.push_str("    #[graphql(default)]\n");
            }
        }
        writeln!(model_fields, "    pub {name}: {ty},").expect("writing to a String");
        writeln!(new_fields, "    pub {name}: {ty},").expect("writing to a String");
        writeln!(change_fields, "    pub {name}: Option<{ty}>,").expect("writing to a String");
    }
    let new_checks = checks(fields, false);
    let change_checks = checks(fields, true);
    let gql_imports = if graphql { "use async_graphql::{Context, InputObject, Object, SimpleObject};\n" } else { "" };
    let gql_resolvers = if graphql { resolvers(names) } else { String::new() };

    format!(
        r#"//! {human_plural} JSON API{also}. Generated by `{command}`.
//!
//! GET /api/{plural}?limit=&offset=   list, newest first
//! GET /api/{plural}/{{id}}             one
//! POST /api/{plural}                  create (every field), 201
//! PATCH /api/{plural}/{{id}}           update (only the fields sent)
//! DELETE /api/{plural}/{{id}}          delete, 204

{gql_imports}use axum::{{
    Router,
    extract::{{Path, State}},
    http::StatusCode,
    routing::get,
}};
use ocre::{{ApiResult, Created, Ctx, Error, Json, OptionExt, Page, Result, params}};
use serde::{{Deserialize, Serialize}};

pub fn routes() -> Router<Ctx> {{
    Router::new()
        .route("/api/{plural}", get(index_handler).post(create_handler))
        .route("/api/{plural}/{{id}}", get(show_handler).patch(update_handler).delete(delete_handler))
}}

/// Row of the `{plural}` table.
#[derive(Deserialize, Serialize{gql_object})]
pub struct {model} {{
    pub id: i64,
{model_fields}    pub created_at: String,
    pub updated_at: String,
}}

/// Body of `POST /api/{plural}`.
#[derive(Deserialize{gql_input})]
pub struct New{model} {{
{new_fields}}}

/// Body of `PATCH /api/{plural}/{{id}}`: absent fields keep their value.
#[derive(Deserialize{gql_input})]
pub struct {model}Changes {{
{change_fields}}}

impl New{model} {{
    fn validate(&self) -> Result<()> {{
{new_checks}        Ok(())
    }}
}}

impl {model}Changes {{
    fn validate(&self) -> Result<()> {{
{change_checks}        Ok(())
    }}
}}

pub async fn list(ctx: &Ctx, page: Page) -> Result<Vec<{model}>> {{
    ctx.db()?.all("SELECT * FROM {plural} ORDER BY id DESC LIMIT ?1 OFFSET ?2", params![page.limit, page.offset]).await
}}

pub async fn find(ctx: &Ctx, id: i64) -> Result<Option<{model}>> {{
    ctx.db()?.first("SELECT * FROM {plural} WHERE id = ?1", params![id]).await
}}

pub async fn create(ctx: &Ctx, input: New{model}) -> Result<{model}> {{
    input.validate()?;
    ctx.db()?
        .first("INSERT INTO {plural} ({columns}) VALUES ({placeholders}) RETURNING *", params![{input_params}])
        .await?
        .ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))
}}

/// `None` when there is no {singular} with this id.
pub async fn update(ctx: &Ctx, id: i64, changes: {model}Changes) -> Result<Option<{model}>> {{
    changes.validate()?;
    ctx.db()?
        .first(
            "UPDATE {plural} SET {coalesce}, updated_at = datetime('now') WHERE id = ?{id_placeholder} RETURNING *",
            params![{change_params}, id],
        )
        .await
}}

/// `false` when there is no {singular} with this id.
pub async fn delete(ctx: &Ctx, id: i64) -> Result<bool> {{
    Ok(ctx.db()?.execute("DELETE FROM {plural} WHERE id = ?1", params![id]).await? > 0)
}}

async fn index_handler(State(ctx): State<Ctx>, page: Page) -> ApiResult<Json<Vec<{model}>>> {{
    Ok(Json(list(&ctx, page).await?))
}}

async fn show_handler(State(ctx): State<Ctx>, Path(id): Path<i64>) -> ApiResult<Json<{model}>> {{
    Ok(Json(find(&ctx, id).await?.or_404()?))
}}

async fn create_handler(State(ctx): State<Ctx>, Json(input): Json<New{model}>) -> ApiResult<Created<{model}>> {{
    Ok(Created(create(&ctx, input).await?))
}}

async fn update_handler(
    State(ctx): State<Ctx>,
    Path(id): Path<i64>,
    Json(changes): Json<{model}Changes>,
) -> ApiResult<Json<{model}>> {{
    Ok(Json(update(&ctx, id, changes).await?.or_404()?))
}}

async fn delete_handler(State(ctx): State<Ctx>, Path(id): Path<i64>) -> ApiResult<StatusCode> {{
    if delete(&ctx, id).await? {{ Ok(StatusCode::NO_CONTENT) }} else {{ Err(Error::NotFound.into()) }}
}}
{gql_resolvers}"#,
        also = if graphql { " and GraphQL" } else { "" },
    )
}

fn resolvers(names: &ModelNames) -> String {
    let ModelNames { model, singular, plural, human_plural, .. } = names;
    let lower_plural = human_plural.to_lowercase();
    format!(
        r#"
#[derive(Default)]
pub struct {model}Query;

#[Object]
impl {model}Query {{
    /// {human_plural}, newest first.
    async fn {plural}(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 50)] limit: i64,
        #[graphql(default = 0)] offset: i64,
    ) -> async_graphql::Result<Vec<{model}>> {{
        Ok(list(ctx.data::<Ctx>()?, Page::new(limit, offset)?).await?)
    }}

    async fn {singular}(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<{model}>> {{
        Ok(find(ctx.data::<Ctx>()?, id).await?)
    }}
}}

#[derive(Default)]
pub struct {model}Mutation;

#[Object]
impl {model}Mutation {{
    async fn create_{singular}(&self, ctx: &Context<'_>, input: New{model}) -> async_graphql::Result<{model}> {{
        Ok(create(ctx.data::<Ctx>()?, input).await?)
    }}

    /// Only the fields given change. Errors with status 404 for unknown {lower_plural}.
    async fn update_{singular}(
        &self,
        ctx: &Context<'_>,
        id: i64,
        changes: {model}Changes,
    ) -> async_graphql::Result<{model}> {{
        Ok(update(ctx.data::<Ctx>()?, id, changes).await?.or_404()?)
    }}

    async fn delete_{singular}(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<bool> {{
        if delete(ctx.data::<Ctx>()?, id).await? {{ Ok(true) }} else {{ Err(Error::NotFound.into()) }}
    }}
}}
"#
    )
}

#[cfg(test)]
mod tests;

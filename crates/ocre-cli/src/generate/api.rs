//! `ocre generate api`: model + JSON REST resource, plus GraphQL with `--graphql`.

use std::fmt::Write as _;

use super::{
    Edits,
    fields::{Field, FieldType, parse_fields},
    model::ensure_model,
    register_routes, with_ocre_feature,
};
use crate::{CliResult, names::ModelNames, output::CliError, project::Project};

const QUERIES_MARKER: &str = "// ocre:graphql-queries";
const MUTATIONS_MARKER: &str = "// ocre:graphql-mutations";
const GRAPHQL_DEP: &str = r#"async-graphql = { version = "7.2.1", default-features = false, features = ["graphiql", "custom-error-conversion"] }"#;

pub fn api(project: &Project, name: &str, specs: &[String], graphql: bool) -> CliResult {
    let names = ModelNames::parse(name)?;
    let fields = parse_fields(specs)?;
    if let Some(file) = fields.iter().find(|f| f.is_attachment() && !f.optional) {
        return Err(CliError::new(format!("attachment `{}` must be optional in a JSON API", file.name)).hint(format!(
            "JSON cannot carry a file, so create cannot require one: use `{0}:attachment?`, then upload with `curl -X PUT -F {0}=@file http://localhost:8787/api/<plural>/1/{0}`",
            file.name
        )));
    }
    let command = format!("ocre g api {name} {}{}", specs.join(" "), if graphql { " --graphql" } else { "" });
    let module = format!("{}_api", names.plural);
    let mut edits = Edits::new(project);
    ensure_model(&mut edits, &names, &fields, &command)?;
    edits.create(&format!("src/{module}.rs"), module_rs(&names, &fields, &command, graphql))?;
    register_routes(&mut edits, &module)?;
    if graphql {
        let cargo = edits.read("Cargo.toml")?.unwrap_or_default();
        edits.update("Cargo.toml", with_graphql(&cargo)?);
        match edits.read("src/graphql.rs")? {
            Some(schema) => edits.update("src/graphql.rs", add_to_schema(&schema, &module, &names)?),
            None => {
                edits.create("src/graphql.rs", new_schema(&module, &names))?;
                let lib = edits.read("src/lib.rs")?.unwrap_or_default();
                let lib = super::insert_after_marker(&lib, super::MODULES_MARKER, "mod graphql;")
                    .expect("register_routes checked the marker");
                let lib = super::insert_after_marker(&lib, super::ROUTES_MARKER, ".merge(graphql::routes())")
                    .expect("register_routes checked the marker");
                edits.update("src/lib.rs", lib);
            }
        }
    }
    let mut report = edits.apply("generate api")?;
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
    let cargo = with_ocre_feature(cargo_toml, "graphql")?;
    if cargo.lines().any(|line| line.starts_with("async-graphql")) {
        return Ok(cargo);
    }
    let mut out = String::with_capacity(cargo.len() + GRAPHQL_DEP.len() + 1);
    for line in cargo.lines() {
        out.push_str(line);
        out.push('\n');
        if line.starts_with("ocre = {") {
            out.push_str(GRAPHQL_DEP);
            out.push('\n');
        }
    }
    Ok(out)
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

fn module_rs(names: &ModelNames, fields: &[Field], command: &str, graphql: bool) -> String {
    let ModelNames { model, singular, plural, human_plural, .. } = names;
    let gql = if graphql { graphql_rs(names, fields) } else { String::new() };
    let also = if graphql { " and GraphQL" } else { "" };
    let files: Vec<&Field> = fields.iter().filter(|f| f.is_attachment()).collect();
    let (mut file_docs, mut file_routes, mut file_items) = (String::new(), String::new(), String::new());
    for file in &files {
        let (name, rules) = (&file.name, file.rules_const());
        let limit = format!("{}_LIMIT", rules);
        write!(
            file_docs,
            "\n//! GET /api/{plural}/{{id}}/{name}        the file (404 when none)\n//! PUT /api/{plural}/{{id}}/{name}        multipart body with the file as `{name}` (`curl -X PUT -F {name}=@file`); replaces it\n//! DELETE /api/{plural}/{{id}}/{name}     removes the file"
        )
        .expect("writing to a String");
        write!(file_routes, "\n        .route(\"/api/{plural}/{{id}}/{name}\", get({name}_file).put(attach_{name}).delete(remove_{name}))")
            .expect("writing to a String");
        write!(
            file_items,
            r#"
/// Largest `PUT .../{name}` body: the file at its limit, plus room for the multipart framing.
const {limit}: usize = {singular}::{rules}.max_bytes + 64 * 1024;

async fn {name}_file(State(ctx): State<Ctx>, Path(id): Path<i64>, headers: HeaderMap) -> ApiResult<Response> {{
    let record = {singular}::find(&ctx, id).await?.or_404()?;
    Ok(storage::serve(&ctx, &record.{name}().or_404()?, &headers, Disposition::Inline).await?)
}}

/// Stores the file and deletes the one it replaces; 422 when the file is missing or not allowed.
async fn attach_{name}(
    State(ctx): State<Ctx>,
    Path(id): Path<i64>,
    Multipart(mut form): Multipart<{limit}>,
) -> ApiResult<Json<{model}>> {{
    let upload = form.file("{name}").ok_or_else(|| Error::Invalid(vec![FieldError::new("{name}", "can't be blank")]))?;
    let changes = {model}Changes {{ {name}: Some(Some(upload)), ..Default::default() }};
    Ok(Json({singular}::update(&ctx, id, changes).await?.or_404()?))
}}

async fn remove_{name}(State(ctx): State<Ctx>, Path(id): Path<i64>) -> ApiResult<Json<{model}>> {{
    let changes = {model}Changes {{ {name}: Some(None), ..Default::default() }};
    Ok(Json({singular}::update(&ctx, id, changes).await?.or_404()?))
}}
"#
        )
        .expect("writing to a String");
    }
    let (http_import, response_import, ocre_import) = if files.is_empty() {
        ("http::StatusCode", "", "")
    } else {
        (
            "http::{HeaderMap, StatusCode}",
            "\n    response::Response,",
            ", FieldError, storage::{self, Disposition, Multipart}",
        )
    };
    format!(
        r#"//! {human_plural} JSON API{also}. Generated by `{command}`.
//! Queries and rules live in the model, `crate::models::{singular}`.
//!
//! GET /api/{plural}?limit=&offset=   list, newest first
//! GET /api/{plural}/{{id}}             one
//! POST /api/{plural}                  create (every required field), 201
//! PATCH /api/{plural}/{{id}}           update (only the fields sent; null clears an optional field)
//! DELETE /api/{plural}/{{id}}          delete, 204{file_docs}
//! Failed validations answer 422 with {{"error": {{"fields": {{"title": ["can't be blank"]}}}}}}.

use axum::{{
    Router,
    extract::{{Path, State}},
    {http_import},{response_import}
    routing::get,
}};
use ocre::{{ApiResult, Created, Ctx, Error, Json, OptionExt, Page{ocre_import}}};

use crate::models::{singular}::{{self, New{model}, {model}, {model}Changes}};

pub fn routes() -> Router<Ctx> {{
    Router::new()
        .route("/api/{plural}", get(index).post(create))
        .route("/api/{plural}/{{id}}", get(show).patch(update).delete(delete)){file_routes}
}}

async fn index(State(ctx): State<Ctx>, page: Page) -> ApiResult<Json<Vec<{model}>>> {{
    Ok(Json({singular}::all(&ctx, page).await?))
}}

async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> ApiResult<Json<{model}>> {{
    Ok(Json({singular}::find(&ctx, id).await?.or_404()?))
}}

async fn create(State(ctx): State<Ctx>, Json(new): Json<New{model}>) -> ApiResult<Created<{model}>> {{
    Ok(Created({singular}::create(&ctx, new).await?))
}}

async fn update(
    State(ctx): State<Ctx>,
    Path(id): Path<i64>,
    Json(changes): Json<{model}Changes>,
) -> ApiResult<Json<{model}>> {{
    Ok(Json({singular}::update(&ctx, id, changes).await?.or_404()?))
}}

async fn delete(State(ctx): State<Ctx>, Path(id): Path<i64>) -> ApiResult<StatusCode> {{
    if {singular}::delete(&ctx, id).await? {{ Ok(StatusCode::NO_CONTENT) }} else {{ Err(Error::NotFound.into()) }}
}}
{file_items}{gql}"#
    )
}

/// GraphQL types mirror the model: `PostNode` (output), `NewPostInput`,
/// `PostPatch` (`undefined` keeps a value, `null` clears an optional one).
fn graphql_rs(names: &ModelNames, fields: &[Field]) -> String {
    let ModelNames { model, singular, plural, human_plural, .. } = names;
    let lower_plural = human_plural.to_lowercase();
    let mut node_fields = String::new();
    let mut node_values = String::new();
    let mut input_fields = String::new();
    let mut input_values = String::new();
    let mut patch_fields = String::new();
    let mut patch_values = String::new();
    for field in fields {
        let (name, ty, column) = (&field.name, field.rust_type(), field.column_type());
        if field.is_attachment() {
            // The file's columns are readable; files are uploaded over REST.
            for (column, ty) in field.columns() {
                writeln!(node_fields, "    pub {column}: {ty},").expect("writing to a String");
                writeln!(node_values, "            {column}: record.{column},").expect("writing to a String");
            }
            writeln!(input_values, "            {name}: None,").expect("writing to a String");
            writeln!(patch_values, "            {name}: None,").expect("writing to a String");
            continue;
        }
        writeln!(node_fields, "    pub {name}: {column},").expect("writing to a String");
        writeln!(node_values, "            {name}: record.{name},").expect("writing to a String");
        if field.ty == FieldType::Boolean {
            input_fields.push_str("    #[graphql(default)]\n");
        }
        writeln!(input_fields, "    pub {name}: {column},").expect("writing to a String");
        writeln!(input_values, "            {name}: input.{name},").expect("writing to a String");
        if field.optional {
            writeln!(patch_fields, "    pub {name}: MaybeUndefined<{ty}>,").expect("writing to a String");
            writeln!(
                patch_values,
                "            {name}: match patch.{name} {{\n                MaybeUndefined::Undefined => None,\n                MaybeUndefined::Null => Some(None),\n                MaybeUndefined::Value(value) => Some(Some(value)),\n            }},"
            )
            .expect("writing to a String");
        } else {
            writeln!(patch_fields, "    pub {name}: Option<{ty}>,").expect("writing to a String");
            writeln!(patch_values, "            {name}: patch.{name},").expect("writing to a String");
        }
    }
    let uses_maybe = if fields.iter().any(|f| f.optional && !f.is_attachment()) { ", MaybeUndefined" } else { "" };
    format!(
        r#"
// ---- GraphQL ----

use async_graphql::{{Context, InputObject, Object, SimpleObject{uses_maybe}}};

#[derive(SimpleObject)]
#[graphql(name = "{model}")]
pub struct {model}Node {{
    pub id: i64,
{node_fields}    pub created_at: String,
    pub updated_at: String,
}}

impl From<{model}> for {model}Node {{
    fn from(record: {model}) -> Self {{
        Self {{
            id: record.id,
{node_values}            created_at: record.created_at,
            updated_at: record.updated_at,
        }}
    }}
}}

#[derive(InputObject)]
pub struct New{model}Input {{
{input_fields}}}

impl From<New{model}Input> for New{model} {{
    fn from(input: New{model}Input) -> Self {{
        Self {{
{input_values}        }}
    }}
}}

#[derive(InputObject)]
pub struct {model}Patch {{
{patch_fields}}}

impl From<{model}Patch> for {model}Changes {{
    fn from(patch: {model}Patch) -> Self {{
        Self {{
{patch_values}        }}
    }}
}}

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
    ) -> async_graphql::Result<Vec<{model}Node>> {{
        let records = {singular}::all(ctx.data::<Ctx>()?, Page::new(limit, offset)?).await?;
        Ok(records.into_iter().map(Into::into).collect())
    }}

    async fn {singular}(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<Option<{model}Node>> {{
        Ok({singular}::find(ctx.data::<Ctx>()?, id).await?.map(Into::into))
    }}
}}

#[derive(Default)]
pub struct {model}Mutation;

#[Object]
impl {model}Mutation {{
    async fn create_{singular}(&self, ctx: &Context<'_>, input: New{model}Input) -> async_graphql::Result<{model}Node> {{
        Ok({singular}::create(ctx.data::<Ctx>()?, input.into()).await?.into())
    }}

    /// Errors with status 404 for unknown {lower_plural}.
    async fn update_{singular}(&self, ctx: &Context<'_>, id: i64, patch: {model}Patch) -> async_graphql::Result<{model}Node> {{
        Ok({singular}::update(ctx.data::<Ctx>()?, id, patch.into()).await?.or_404()?.into())
    }}

    async fn delete_{singular}(&self, ctx: &Context<'_>, id: i64) -> async_graphql::Result<bool> {{
        if {singular}::delete(ctx.data::<Ctx>()?, id).await? {{ Ok(true) }} else {{ Err(Error::NotFound.into()) }}
    }}
}}
"#
    )
}

#[cfg(test)]
#[path = "../../tests/generate/api.rs"]
mod tests;

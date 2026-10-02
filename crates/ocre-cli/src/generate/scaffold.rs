//! `ocre generate scaffold`: model + HTML pages (askama) for full CRUD; with
//! `--realtime`, index pages that show other visitors' changes live.

use std::fmt::Write as _;

use minijinja::Value;
use serde::Serialize;

use super::{
    Edits, ROUTES_MARKER,
    fields::{ATTACHMENT_TYPES, Field, FieldType, parse_model_fields},
    insert_after_marker,
    model::ensure_model,
    realtime::add_channel,
    register_routes,
    templates::render,
};
use crate::{
    CliResult,
    names::{ModelNames, humanize},
    output::CliError,
    project::Project,
};

/// The route serving the editor's upload script.
const DIRECT_UPLOAD_ROUTES: &str = ".merge(ocre::storage::direct_upload_script())";

pub fn scaffold(project: &Project, name: &str, specs: &[String], realtime: bool) -> CliResult {
    let names = ModelNames::parse(name)?;
    let (fields, many) = parse_model_fields(specs)?;
    let command = format!("ocre g scaffold {name} {}{}", specs.join(" "), if realtime { " --realtime" } else { "" });
    let mut edits = Edits::new(project);
    ensure_model(&mut edits, &names, &fields, &many, &command)?;
    // `public_id:token`: URLs carry it instead of the integer id; no form or page shows it.
    let public_id = fields.iter().any(|f| f.ty == FieldType::PublicId);
    let fields: Vec<Field> = fields.into_iter().filter(|f| f.ty != FieldType::PublicId).collect();
    let plural = &names.plural;
    let mut controller = controller_rs(&names, &fields, &many, &command, realtime);
    if public_id {
        controller = super::public_id::html(&controller, &names, &many);
    }
    edits.create(&format!("src/{plural}.rs"), controller)?;
    for (file, contents) in views(&edits, &names, &fields, &many, realtime, public_id)? {
        edits.create(&format!("templates/{plural}/{file}"), contents)?;
    }
    register_routes(&mut edits, plural)?;
    if fields.iter().any(|f| f.ty == FieldType::RichText) {
        // Serves /ocre/direct-upload.js, which uploads the images dropped into the editor; once per app.
        let lib = edits.read("src/lib.rs")?.unwrap_or_default();
        if !lib.contains(DIRECT_UPLOAD_ROUTES) {
            let lib = insert_after_marker(&lib, ROUTES_MARKER, DIRECT_UPLOAD_ROUTES)
                .expect("register_routes found the marker");
            edits.update("src/lib.rs", lib);
        }
    }
    super::test_files::scaffold_tests(&mut edits, &names, &fields, &command, public_id)?;
    if realtime {
        add_channel(&mut edits, plural, &command)?;
    }
    let mut report = edits.apply("generate scaffold")?;
    report.next =
        vec!["ocre migrate".to_owned(), "ocre dev".to_owned(), format!("open http://localhost:8787/{plural}")];
    if realtime {
        report.next.push(format!(
            "open http://localhost:8787/{plural} in a second window, then create a {}",
            names.human_singular.to_lowercase()
        ));
    }
    Ok(report)
}

/// `PostForm` field type: text for anything typed in an input, `bool` for checkboxes.
fn form_type(field: &Field) -> &'static str {
    if field.ty == FieldType::Boolean { "bool" } else { "String" }
}

/// Expression turning form text into the model value (`v` is a Validator).
fn from_form(field: &Field) -> String {
    let name = &field.name;
    match (field.ty, field.optional) {
        (FieldType::Boolean, _) => format!("self.{name}"),
        (FieldType::Json, false) => format!("v.json(\"{name}\", &self.{name}).unwrap_or_default()"),
        (FieldType::Json, true) => format!("v.optional_json(\"{name}\", &self.{name})"),
        (FieldType::Enum, false) => format!("v.one_of(\"{name}\", &self.{name}).unwrap_or_default()"),
        (FieldType::Enum, true) => format!("v.optional_one_of(\"{name}\", &self.{name})"),
        (ty, false) if ty.is_numeric() => format!("v.number(\"{name}\", &self.{name}).unwrap_or_default()"),
        (ty, true) if ty.is_numeric() => format!("v.optional_number(\"{name}\", &self.{name})"),
        (_, false) => format!("self.{name}.clone()"),
        (_, true) => format!("(!self.{name}.trim().is_empty()).then(|| self.{name}.clone())"),
    }
}

/// Expression turning the model value into form text (compact JSON for `json`).
fn to_form(field: &Field, record: &str) -> String {
    let name = &field.name;
    match (field.ty, field.optional) {
        (FieldType::Boolean, _) => format!("{record}.{name}"),
        (FieldType::Json, false) => format!("{record}.{name}.to_string()"),
        (FieldType::Json, true) => {
            format!("{record}.{name}.as_ref().map(|value| value.to_string()).unwrap_or_default()")
        }
        (FieldType::Enum, false) => format!("{record}.{name}.to_string()"),
        (FieldType::Enum, true) => format!("{record}.{name}.map(|value| value.to_string()).unwrap_or_default()"),
        (ty, false) if ty.is_numeric() => format!("{record}.{name}.to_string()"),
        (ty, true) if ty.is_numeric() => format!("{record}.{name}.map(|value| value.to_string()).unwrap_or_default()"),
        (_, false) => format!("{record}.{name}.clone()"),
        (_, true) => format!("{record}.{name}.clone().unwrap_or_default()"),
    }
}

fn controller_rs(names: &ModelNames, fields: &[Field], many: &[String], command: &str, realtime: bool) -> String {
    let ModelNames { model, singular, plural, human_singular, human_plural } = names;
    let mut form_fields = String::new();
    let mut new_values = String::new();
    let mut change_values = String::new();
    let mut record_values = String::new();
    let files: Vec<&Field> = fields.iter().filter(|f| f.is_attachment()).collect();
    for field in fields {
        let name = &field.name;
        if field.is_attachment() {
            write!(
                form_fields,
                "    /// The chosen file. File inputs cannot be refilled: after an error the form asks again.\n    #[serde(skip)]\n    pub {name}: Option<Upload>,\n"
            )
            .expect("writing to a String");
            writeln!(new_values, "            {name}: self.{name}.clone(),").expect("writing to a String");
            if field.optional {
                write!(form_fields, "    /// \"Remove\" checkbox on the edit page.\n    pub remove_{name}: bool,\n")
                    .expect("writing to a String");
                writeln!(
                    change_values,
                    "            {name}: if self.remove_{name} {{ Some(None) }} else {{ self.{name}.clone().map(Some) }},"
                )
                .expect("writing to a String");
            } else {
                writeln!(change_values, "            {name}: self.{name}.clone(),").expect("writing to a String");
            }
            continue;
        }
        if field.ty == FieldType::Boolean {
            form_fields.push_str("    /// Unchecked checkboxes are not submitted.\n");
        }
        if field.ty == FieldType::LockVersion {
            form_fields.push_str("    /// Hidden field: the version the edit form was opened on.\n");
        }
        writeln!(form_fields, "    pub {name}: {},", form_type(field)).expect("writing to a String");
        let value = from_form(field);
        if field.ty != FieldType::LockVersion {
            writeln!(new_values, "            {name}: {value},").expect("writing to a String");
        }
        writeln!(change_values, "            {name}: Some({value}),").expect("writing to a String");
        writeln!(record_values, "            {name}: {},", to_form(field, "record")).expect("writing to a String");
    }
    // `rich_text` fields render with `ocre::filters` (`|rich_text`, `|plain_text`).
    let filters = if fields.iter().any(|f| f.ty == FieldType::RichText) { "filters, " } else { "" };
    if !files.is_empty() {
        record_values.push_str("            ..Self::default()\n");
    }
    let Files {
        form_import,
        mut http_import,
        mut storage_import,
        extractor,
        binding,
        mut routes,
        mut items,
        mut handlers,
    } = Files::new(names, &files);
    let Many { show_fields, show_loads, show_values, paths: many_paths } =
        Many::add(names, many, &mut routes, &mut items, &mut handlers);
    let rich_text = fields.iter().any(|f| f.ty == FieldType::RichText);
    if rich_text {
        add_embeds(names, &mut routes, &mut items, &mut handlers);
    }
    if files.is_empty() && (!many.is_empty() || rich_text) {
        http_import = "http::{HeaderMap, StatusCode}";
        storage_import = ", storage::{self, Disposition, Multipart}";
    }
    let live = if realtime { Live::new(names) } else { Live::default() };
    let Live { import, views, on_create, on_update, on_delete } = live;
    let file_paths: String = files
        .iter()
        .map(|file| {
            let name = &file.name;
            format!(
                "\n    /// The {name} file.\n    pub fn {name}(id: impl Display) -> String {{\n        format!(\"/{plural}/{{id}}/{name}\")\n    }}\n"
            )
        })
        .collect();
    let lower_plural = human_plural.to_lowercase();

    format!(
        r#"//! {human_plural} pages (HTML). Generated by `{command}`.
//! Queries and rules live in the model, `crate::models::{singular}`.

use askama::Template;
use axum::{{
    {form_import}Router,
    extract::{{Path, State}},
    {http_import},
    response::{{Html, IntoResponse, Redirect, Response}},
    routing::{{get, post}},
}};
use ocre::{{Ctx, Error, FieldError, {filters}Flash, OptionExt, Page, Result, Session, Validator, {import}render{storage_import}}};
use serde::Deserialize;

use crate::models::{singular}::{{self, New{model}, {model}, {model}Changes}};

pub fn routes() -> Router<Ctx> {{
    Router::new()
        .route("/{plural}", get(index).post(create))
        .route("/{plural}/new", get(new))
        .route("/{plural}/{{id}}", get(show).post(update))
        .route("/{plural}/{{id}}/edit", get(edit))
        .route("/{plural}/{{id}}/delete", post(delete)){routes}
}}

/// Paths of the {lower_plural} pages (Rails' `{plural}_path`, `{singular}_path(record)`...).
/// Handlers redirect to them and templates link with them:
/// `<a href="{{{{ paths::show({singular}.id) }}}}">`; other modules call
/// `crate::{plural}::paths::show(id)`.
pub mod paths {{
    use std::fmt::Display;

    pub fn index() -> &'static str {{
        "/{plural}"
    }}

    pub fn new() -> &'static str {{
        "/{plural}/new"
    }}

    pub fn show(id: impl Display) -> String {{
        format!("/{plural}/{{id}}")
    }}

    pub fn edit(id: impl Display) -> String {{
        format!("/{plural}/{{id}}/edit")
    }}

    pub fn delete(id: impl Display) -> String {{
        format!("/{plural}/{{id}}/delete")
    }}
{file_paths}{many_paths}}}
{items}
/// What the new and edit forms submit, as typed: numbers stay text until
/// validated, so a typo shows a field error instead of a failed request.
/// A missing field is empty (and unchecked for checkboxes).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct {model}Form {{
{form_fields}}}

impl {model}Form {{
    fn from_record(record: &{model}) -> Self {{
        Self {{
{record_values}        }}
    }}

    /// Parses the text and runs the model's checks, so the form shows every
    /// error at once (database checks run in `create`).
    fn to_new(&self) -> Result<New{model}> {{
        let mut v = Validator::new();
        let new = New{model} {{
{new_values}        }};
        v.merge(new.validate()).finish()?;
        Ok(new)
    }}

    fn to_changes(&self) -> Result<{model}Changes> {{
        let mut v = Validator::new();
        let changes = {model}Changes {{
{change_values}        }};
        v.merge(changes.validate()).finish()?;
        Ok(changes)
    }}
}}

#[derive(Template)]
#[template(path = "{plural}/index.html")]
struct IndexView {{
    flash: Flash,
    page: Page,
    {plural}: Vec<{model}>,
}}

#[derive(Template)]
#[template(path = "{plural}/show.html")]
struct ShowView {{
    flash: Flash,
    {singular}: {model},{show_fields}
}}

#[derive(Template)]
#[template(path = "{plural}/new.html")]
struct NewView {{
    form: {model}Form,
    errors: Vec<FieldError>,
}}

#[derive(Template)]
#[template(path = "{plural}/edit.html")]
struct EditView {{
    id: i64,
    form: {model}Form,
    errors: Vec<FieldError>,
}}{views}

async fn index(State(ctx): State<Ctx>, flash: Flash, page: Page) -> Result<Html<String>> {{
    render(&IndexView {{ flash, page, {plural}: {singular}::all(&ctx, page).await? }})
}}

async fn show(State(ctx): State<Ctx>, flash: Flash, Path(id): Path<i64>) -> Result<Html<String>> {{
    let record = {singular}::find(&ctx, id).await?.or_404()?;{show_loads}
    render(&ShowView {{ flash, {singular}: record{show_values} }})
}}

async fn new() -> Result<Html<String>> {{
    render(&NewView {{ form: {model}Form::default(), errors: vec![] }})
}}

async fn create(State(ctx): State<Ctx>, session: Session, {extractor}) -> Result<Response> {{
{binding}    let created = match form.to_new() {{
        Ok(new) => {singular}::create(&ctx, new).await,
        Err(err) => Err(err),
    }};
    match created {{
        Ok(record) => {{
{on_create}            session.flash("notice", "{human_singular} was successfully created.")?;
            Ok(Redirect::to(&paths::show(record.id)).into_response())
        }}
        Err(Error::Invalid(errors)) => {{
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&NewView {{ form, errors }})?).into_response())
        }}
        Err(err) => Err(err),
    }}
}}

async fn edit(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Html<String>> {{
    let record = {singular}::find(&ctx, id).await?.or_404()?;
    render(&EditView {{ id, form: {model}Form::from_record(&record), errors: vec![] }})
}}

async fn update(
    State(ctx): State<Ctx>,
    session: Session,
    Path(id): Path<i64>,
    {extractor},
) -> Result<Response> {{
{binding}    let updated = match form.to_changes() {{
        Ok(changes) => {singular}::update(&ctx, id, changes).await,
        Err(err) => Err(err),
    }};
    match updated {{
        Ok(record) => {{
            let record = record.or_404()?;
{on_update}            session.flash("notice", "{human_singular} was successfully updated.")?;
            Ok(Redirect::to(&paths::show(record.id)).into_response())
        }}
        Err(Error::Invalid(errors)) => {{
            Ok((StatusCode::UNPROCESSABLE_ENTITY, render(&EditView {{ id, form, errors }})?).into_response())
        }}
        Err(err) => Err(err),
    }}
}}

async fn delete(State(ctx): State<Ctx>, session: Session, Path(id): Path<i64>) -> Result<Redirect> {{
    if !{singular}::delete(&ctx, id).await? {{
        return Err(Error::NotFound);
    }}
{on_delete}    session.flash("notice", "{human_singular} was successfully destroyed.")?;
    Ok(Redirect::to(paths::index()))
}}
{handlers}"#
    )
}

/// The controller code attachment fields add: multipart forms, and a route
/// serving each file. Plain `Form` handling without attachments.
struct Files {
    form_import: &'static str,
    http_import: &'static str,
    storage_import: &'static str,
    extractor: String,
    binding: String,
    routes: String,
    items: String,
    handlers: String,
}

impl Files {
    fn new(names: &ModelNames, files: &[&Field]) -> Self {
        let ModelNames { model, singular, plural, .. } = names;
        if files.is_empty() {
            return Self {
                form_import: "Form, ",
                http_import: "http::StatusCode",
                storage_import: "",
                extractor: format!("Form(form): Form<{model}Form>"),
                binding: String::new(),
                routes: String::new(),
                items: String::new(),
                handlers: String::new(),
            };
        }
        let limit: String =
            files.iter().map(|f| format!("{singular}::{}.max_bytes as usize + ", f.rules_const())).collect();
        let takes: String = files.iter().map(|f| format!("{0}: multipart.file(\"{0}\"), ", f.name)).collect();
        let items = format!(
            r#"
/// Largest request the new and edit forms accept: every file at its limit,
/// plus 1 MB for the text fields. Larger requests get a 413 page.
const FORM_LIMIT: usize = {limit}1024 * 1024;

impl {model}Form {{
    /// The text fields, plus the chosen files.
    fn from_multipart(mut multipart: MultipartForm) -> Result<Self> {{
        Ok(Self {{ {takes}..multipart.form()? }})
    }}
}}
"#
        );
        let mut routes = String::new();
        let mut handlers = String::new();
        for file in files {
            let name = &file.name;
            write!(routes, "\n        .route(\"/{plural}/{{id}}/{name}\", get({name}_file))")
                .expect("writing to a String");
            let (attachment, missing) = if file.optional {
                (format!("record.{name}().or_404()?"), " (404 when there is none)")
            } else {
                (format!("record.{name}()"), "")
            };
            write!(
                handlers,
                r#"
/// The {name} file{missing}: shown in the browser when its type is safe to
/// display, downloaded otherwise.
async fn {name}_file(State(ctx): State<Ctx>, Path(id): Path<i64>, headers: HeaderMap) -> Result<Response> {{
    let record = {singular}::find(&ctx, id).await?.or_404()?;
    storage::serve(&ctx, &{attachment}, &headers, Disposition::Inline).await
}}
"#
            )
            .expect("writing to a String");
        }
        Self {
            form_import: "",
            http_import: "http::{HeaderMap, StatusCode}",
            storage_import: ", storage::{self, Disposition, Multipart, MultipartForm, Upload}",
            extractor: "Multipart(multipart): Multipart<FORM_LIMIT>".to_owned(),
            binding: format!("    let form = {model}Form::from_multipart(multipart)?;\n"),
            routes,
            items,
            handlers,
        }
    }
}

/// The show page's lists of `photos:attachments` files, with the routes to
/// add files, open one and delete one. Empty strings without them.
#[derive(Default)]
struct Many {
    show_fields: String,
    show_loads: String,
    show_values: String,
    paths: String,
}

impl Many {
    fn add(
        names: &ModelNames,
        many: &[String],
        routes: &mut String,
        items: &mut String,
        handlers: &mut String,
    ) -> Self {
        let ModelNames { singular, plural, .. } = names;
        let mut out = Self::default();
        for name in many {
            let one = crate::names::singularize(name).expect("checked when parsed");
            let child = ModelNames::parse(&format!("{singular}_{one}")).expect("a valid model name");
            let (module, model, limit) = (&child.singular, &child.model, format!("{}_LIMIT", name.to_uppercase()));
            let human = humanize(name);
            write!(out.show_fields, "\n    {name}: Vec<crate::models::{module}::{model}>,")
                .expect("writing to a String");
            write!(
                out.show_loads,
                "\n    let {name} = crate::models::{module}::query().eq(\"{singular}_id\", id).order_asc(\"id\").all(&ctx.db()?).await?;"
            )
            .expect("writing to a String");
            write!(out.show_values, ", {name}").expect("writing to a String");
            write!(
                out.paths,
                "\n    /// Where the show page adds {name}.\n    pub fn {name}(id: impl Display) -> String {{\n        format!(\"/{plural}/{{id}}/{name}\")\n    }}\n\n    /// One of the {name}.\n    pub fn {one}(id: impl Display, file_id: impl Display) -> String {{\n        format!(\"/{plural}/{{id}}/{name}/{{file_id}}\")\n    }}\n\n    /// Deletes one of the {name}.\n    pub fn delete_{one}(id: impl Display, file_id: impl Display) -> String {{\n        format!(\"/{plural}/{{id}}/{name}/{{file_id}}/delete\")\n    }}\n"
            )
            .expect("writing to a String");
            write!(
                routes,
                "\n        .route(\"/{plural}/{{id}}/{name}\", post(attach_{name}))\n        .route(\"/{plural}/{{id}}/{name}/{{file_id}}\", get({one}_file))\n        .route(\"/{plural}/{{id}}/{name}/{{file_id}}/delete\", post(delete_{one}))"
            )
            .expect("writing to a String");
            write!(
                items,
                "\n/// Largest request adding {name}: ten files at their limit, plus room for the multipart framing.\nconst {limit}: usize = 10 * crate::models::{module}::FILE.max_bytes as usize + 64 * 1024;\n"
            )
            .expect("writing to a String");
            write!(
                handlers,
                r#"
/// Stores the chosen files (the show page's form); an error names the file in an alert.
async fn attach_{name}(
    State(ctx): State<Ctx>,
    session: Session,
    Path(id): Path<i64>,
    Multipart(mut form): Multipart<{limit}>,
) -> Result<Redirect> {{
    let record = {singular}::find(&ctx, id).await?.or_404()?;
    let uploads = form.files("{name}");
    if uploads.is_empty() {{
        session.flash("alert", "Choose at least one file.")?;
        return Ok(Redirect::to(&paths::show(id)));
    }}
    match record.attach_{name}(&ctx, uploads).await {{
        Ok(_) => session.flash("notice", "{human} were added.")?,
        Err(Error::Invalid(errors)) => {{
            let messages: Vec<String> = errors.iter().map(FieldError::full_message).collect();
            session.flash("alert", messages.join(" "))?;
        }}
        Err(err) => return Err(err),
    }}
    Ok(Redirect::to(&paths::show(id)))
}}

/// The row of file `file_id` of record `id`.
async fn find_{one}(ctx: &Ctx, id: i64, file_id: i64) -> Result<crate::models::{module}::{model}> {{
    let query = crate::models::{module}::query().eq("id", file_id).eq("{singular}_id", id);
    query.first(&ctx.db()?).await?.or_404()
}}

/// One of the {name}: shown in the browser when its type is safe to display, downloaded otherwise.
async fn {one}_file(
    State(ctx): State<Ctx>,
    Path((id, file_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> Result<Response> {{
    let row = find_{one}(&ctx, id, file_id).await?;
    storage::serve(&ctx, &row.file(), &headers, Disposition::Inline).await
}}

async fn delete_{one}(
    State(ctx): State<Ctx>,
    session: Session,
    Path((id, file_id)): Path<(i64, i64)>,
) -> Result<Redirect> {{
    let row = find_{one}(&ctx, id, file_id).await?;
    crate::models::{module}::delete(&ctx, row.id).await?;
    session.flash("notice", format!("{{}} was deleted.", row.file_filename))?;
    Ok(Redirect::to(&paths::show(id)))
}}
"#
            )
            .expect("writing to a String");
        }
        out
    }
}

/// Routes of the files dropped into a `rich_text` field's editor (Action
/// Text attachments): stored in R2 under `<plural>/embeds/`, then shown in
/// the text from their route.
fn add_embeds(names: &ModelNames, routes: &mut String, items: &mut String, handlers: &mut String) {
    let plural = &names.plural;
    write!(
        routes,
        "\n        .route(\"/{plural}/embeds\", post(upload_embed))\n        .route(\"/{plural}/embeds/{{name}}\", get(embed))"
    )
    .expect("writing to a String");
    items.push_str(
        r#"
/// Files the rich text editor accepts: images, up to 10 MB each.
const EMBED: ocre::storage::Rules = ocre::storage::Rules {
    max_bytes: 10 * 1024 * 1024,
    content_types: &["image/png", "image/jpeg", "image/gif", "image/webp"],
};

/// Largest embed request: the image at its limit, plus room for the multipart framing.
const EMBED_LIMIT: usize = EMBED.max_bytes as usize + 64 * 1024;
"#,
    );
    write!(
        handlers,
        r#"
/// Stores an image dropped into the editor (Action Text attachments); the
/// editor then shows it from the `url` answered.
async fn upload_embed(
    State(ctx): State<Ctx>,
    Multipart(mut form): Multipart<EMBED_LIMIT>,
) -> Result<axum::Json<ocre::serde_json::Value>> {{
    let upload = form.file("file").ok_or_else(|| Error::bad_request("no `file` in the form"))?;
    let mut v = Validator::new();
    v.file("file", &upload, &EMBED);
    v.finish()?;
    let stored = storage::store(&ctx, "{plural}/embeds", upload).await?;
    let name = stored.key.rsplit('/').next().unwrap_or_default();
    Ok(axum::Json(ocre::serde_json::json!({{ "url": format!("/{plural}/embeds/{{name}}") }})))
}}

/// An image of the rich text.
async fn embed(State(ctx): State<Ctx>, Path(name): Path<String>, headers: HeaderMap) -> Result<Response> {{
    let object = storage::head(&ctx, &format!("{plural}/embeds/{{name}}")).await?.or_404()?;
    storage::serve(&ctx, &object.attachment(&name), &headers, Disposition::Inline).await
}}
"#
    )
    .expect("writing to a String");
}

/// The controller code `--realtime` adds: a row partial, and a broadcast
/// after each change. Empty strings without `--realtime`.
#[derive(Default)]
struct Live {
    import: &'static str,
    views: String,
    on_create: String,
    on_update: &'static str,
    on_delete: String,
}

impl Live {
    fn new(names: &ModelNames) -> Self {
        let ModelNames { model, singular, plural, .. } = names;
        Self {
            import: "realtime, ",
            views: format!(
                r#"

/// One table row; the index page and live updates share it.
#[derive(Template)]
#[template(path = "{plural}/_row.html")]
struct RowView<'a> {{
    {singular}: &'a {model},
}}

fn row(record: &{model}) -> Result<String> {{
    Ok(render(&RowView {{ {singular}: record }})?.0)
}}

/// Sends `html` to every open index page (channel `{plural}`, see
/// src/realtime.rs). Best effort: a failed broadcast is logged by Ocre and
/// never fails the request.
async fn broadcast(ctx: &Ctx, html: &str) {{
    realtime::broadcast(ctx, "{plural}", html).await.ok();
}}"#
            ),
            on_create: format!(
                "            broadcast(&ctx, &realtime::prepend(\"{plural}\", &row(&record)?)).await;\n"
            ),
            on_update: "            broadcast(&ctx, &row(&record)?).await;\n",
            on_delete: format!("    broadcast(&ctx, &realtime::remove(&format!(\"{singular}_{{id}}\"))).await;\n"),
        }
    }
}

/// The scaffold's view templates, in the order they are written.
const VIEWS: [&str; 5] = ["index.html", "show.html", "new.html", "edit.html", "_form.html"];

#[derive(Serialize)]
struct ViewField {
    name: String,
    label: String,
    attachment: bool,
    optional: bool,
    /// askama expression showing the value in a table cell.
    display: String,
    /// The value on the show page: the display, or a link for a file.
    show: String,
    /// The form widget bound to `form.<name>`.
    input: String,
    /// Not shown in lists or on the show page, and a hidden input in forms (`lock_version`).
    hidden: bool,
}

#[derive(Serialize)]
struct ViewContext<'a> {
    model: &'a str,
    singular: &'a str,
    plural: &'a str,
    human_singular: &'a str,
    human_plural: &'a str,
    /// `blog post`, in sentences.
    lower: String,
    /// The field links and titles use: `id`, or `public_id` with `public_id:token`.
    key: &'a str,
    realtime: bool,
    /// Some field is a file: forms are `multipart/form-data`.
    multipart: bool,
    fields: Vec<ViewField>,
    /// Some field is `rich_text`: forms load the Trix editor.
    rich_text: bool,
    /// `photos:attachments` lists on the show page.
    many: Vec<ManyView>,
}

#[derive(Serialize)]
struct ManyView {
    /// `photos`
    name: String,
    /// `photo`
    one: String,
    /// `Photos`
    label: String,
    /// `photos`, in sentences.
    lower: String,
}

/// Renders `templates/<plural>/*.html` from the `scaffold/` templates (the
/// app's overrides first); `_row.html` too with `--realtime`.
fn views(
    edits: &Edits,
    names: &ModelNames,
    fields: &[Field],
    many: &[String],
    realtime: bool,
    public_id: bool,
) -> Result<Vec<(String, String)>, CliError> {
    let key = if public_id { "public_id" } else { "id" };
    let ModelNames { model, singular, plural, human_singular, human_plural } = names;
    let context = ViewContext {
        model,
        singular,
        plural,
        human_singular,
        human_plural,
        lower: human_singular.to_lowercase(),
        key,
        realtime,
        multipart: fields.iter().any(Field::is_attachment),
        fields: fields
            .iter()
            .map(|field| ViewField {
                name: field.name.clone(),
                label: field.label(),
                attachment: field.is_attachment(),
                optional: field.optional,
                display: field.display(singular),
                show: if field.is_attachment() {
                    file_link(field, singular, key)
                } else {
                    field.display_full(singular)
                },
                input: input(field, singular, plural),
                hidden: field.ty == FieldType::LockVersion,
            })
            .collect(),
        rich_text: fields.iter().any(|f| f.ty == FieldType::RichText),
        many: many
            .iter()
            .map(|name| ManyView {
                name: name.clone(),
                one: crate::names::singularize(name).expect("checked when parsed"),
                label: humanize(name),
                lower: humanize(name).to_lowercase(),
            })
            .collect(),
    };
    let values = Value::from_serialize(&context);
    let mut out = Vec::new();
    for view in VIEWS.iter().chain(realtime.then_some(&"_row.html")) {
        out.push(((*view).to_owned(), render(edits, &format!("scaffold/{view}"), &values)?));
    }
    Ok(out)
}

/// `<a href="{{ paths::avatar(post.id) }}">me.png</a> (12 KB)` on the show page;
/// not boosted, so the browser opens or downloads the file itself.
fn file_link(field: &Field, singular: &str, key: &str) -> String {
    let name = &field.name;
    let link = format!(
        "<a href=\"{{{{ paths::{name}({singular}.{key}) }}}}\" hx-boost=\"false\">{{{{ file.filename }}}}</a> ({{{{ file.human_size() }}}})"
    );
    if field.optional {
        format!("{{% if let Some(file) = {singular}.{name}() %}}{link}{{% endif %}}")
    } else {
        format!("{{% let file = {singular}.{name}() %}}{link}")
    }
}

/// The form widget of a field, bound to `form.<name>`.
fn input(field: &Field, singular: &str, plural: &str) -> String {
    let name = &field.name;
    let required = if field.optional { "" } else { " required" };
    match field.ty {
        FieldType::String | FieldType::Uuid => {
            format!(r#"<input name="{name}" value="{{{{ form.{name} }}}}"{required}>"#)
        }
        FieldType::Text => format!(r#"<textarea name="{name}" rows="5"{required}>{{{{ form.{name} }}}}</textarea>"#),
        // Trix edits a hidden input (`required` would not reach the editor):
        // the model checks presence on the text.
        FieldType::RichText => format!(
            r#"<input type="hidden" id="{singular}_{name}" name="{name}" value="{{{{ form.{name} }}}}"><trix-editor input="{singular}_{name}" data-embeds-url="/{plural}/embeds"></trix-editor>"#
        ),
        // Public ids never reach a form (the scaffold drops them); hidden if one did.
        FieldType::LockVersion | FieldType::PublicId => {
            format!(r#"<input type="hidden" name="{name}" value="{{{{ form.{name} }}}}">"#)
        }
        FieldType::Json => format!(
            r#"<textarea name="{name}" rows="5" spellcheck="false" placeholder="{{}}"{required}>{{{{ form.{name} }}}}</textarea>"#
        ),
        FieldType::Integer | FieldType::References => {
            format!(r#"<input type="number" step="1" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#)
        }
        FieldType::Float => {
            format!(r#"<input type="number" step="any" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#)
        }
        FieldType::Decimal => {
            format!(r#"<input inputmode="decimal" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#)
        }
        FieldType::Date => format!(r#"<input type="date" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#),
        FieldType::Time => format!(r#"<input type="time" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#),
        FieldType::DateTime => {
            format!(r#"<input type="datetime-local" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#)
        }
        FieldType::Boolean => {
            format!(r#"<input type="checkbox" name="{name}" value="true"{{% if form.{name} %}} checked{{% endif %}}>"#)
        }
        FieldType::Enum => {
            let mut options = if field.optional { "<option value=\"\"></option>".to_owned() } else { String::new() };
            for value in &field.enumeration.as_ref().expect("enum fields have values").values {
                write!(
                    options,
                    r#"<option value="{value}"{{% if form.{name} == "{value}" %}} selected{{% endif %}}>{}</option>"#,
                    humanize(value)
                )
                .expect("writing to a String");
            }
            format!(r#"<select name="{name}"{required}>{options}</select>"#)
        }
        // Not `required`: the edit form keeps the stored file when none is chosen.
        FieldType::Attachment => {
            format!(r#"<input type="file" name="{name}" accept="{}">"#, ATTACHMENT_TYPES.join(","))
        }
    }
}

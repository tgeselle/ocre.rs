//! `ocre generate scaffold`: model + HTML pages (askama) for full CRUD; with
//! `--realtime`, index pages that show other visitors' changes live.

use std::fmt::Write as _;

use minijinja::Value;
use serde::Serialize;

use super::{
    Edits,
    fields::{ATTACHMENT_TYPES, Field, FieldType, parse_fields},
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

pub fn scaffold(project: &Project, name: &str, specs: &[String], realtime: bool) -> CliResult {
    let names = ModelNames::parse(name)?;
    let fields = parse_fields(specs)?;
    let command = format!("ocre g scaffold {name} {}{}", specs.join(" "), if realtime { " --realtime" } else { "" });
    let mut edits = Edits::new(project);
    ensure_model(&mut edits, &names, &fields, &command)?;
    let plural = &names.plural;
    edits.create(&format!("src/{plural}.rs"), controller_rs(&names, &fields, &command, realtime))?;
    for (file, contents) in views(&edits, &names, &fields, realtime)? {
        edits.create(&format!("templates/{plural}/{file}"), contents)?;
    }
    register_routes(&mut edits, plural)?;
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

fn controller_rs(names: &ModelNames, fields: &[Field], command: &str, realtime: bool) -> String {
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
        writeln!(form_fields, "    pub {name}: {},", form_type(field)).expect("writing to a String");
        let value = from_form(field);
        writeln!(new_values, "            {name}: {value},").expect("writing to a String");
        writeln!(change_values, "            {name}: Some({value}),").expect("writing to a String");
        writeln!(record_values, "            {name}: {},", to_form(field, "record")).expect("writing to a String");
    }
    if !files.is_empty() {
        record_values.push_str("            ..Self::default()\n");
    }
    let Files { form_import, http_import, storage_import, extractor, binding, routes, items, handlers } =
        Files::new(names, &files);
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
use ocre::{{Ctx, Error, FieldError, Flash, OptionExt, Page, Result, Session, Validator, {import}render{storage_import}}};
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
{file_paths}}}
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
    {singular}: {model},
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
    render(&ShowView {{ flash, {singular}: {singular}::find(&ctx, id).await?.or_404()? }})
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
        let limit: String = files.iter().map(|f| format!("{singular}::{}.max_bytes + ", f.rules_const())).collect();
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
    realtime: bool,
    /// Some field is a file: forms are `multipart/form-data`.
    multipart: bool,
    fields: Vec<ViewField>,
}

/// Renders `templates/<plural>/*.html` from the `scaffold/` templates (the
/// app's overrides first); `_row.html` too with `--realtime`.
fn views(
    edits: &Edits,
    names: &ModelNames,
    fields: &[Field],
    realtime: bool,
) -> Result<Vec<(String, String)>, CliError> {
    let ModelNames { model, singular, plural, human_singular, human_plural } = names;
    let context = ViewContext {
        model,
        singular,
        plural,
        human_singular,
        human_plural,
        lower: human_singular.to_lowercase(),
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
                show: if field.is_attachment() { file_link(field, singular) } else { field.display(singular) },
                input: input(field),
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
fn file_link(field: &Field, singular: &str) -> String {
    let name = &field.name;
    let link = format!(
        "<a href=\"{{{{ paths::{name}({singular}.id) }}}}\" hx-boost=\"false\">{{{{ file.filename }}}}</a> ({{{{ file.human_size() }}}})"
    );
    if field.optional {
        format!("{{% if let Some(file) = {singular}.{name}() %}}{link}{{% endif %}}")
    } else {
        format!("{{% let file = {singular}.{name}() %}}{link}")
    }
}

/// The form widget of a field, bound to `form.<name>`.
fn input(field: &Field) -> String {
    let name = &field.name;
    let required = if field.optional { "" } else { " required" };
    match field.ty {
        FieldType::String | FieldType::Uuid => {
            format!(r#"<input name="{name}" value="{{{{ form.{name} }}}}"{required}>"#)
        }
        FieldType::Text => format!(r#"<textarea name="{name}" rows="5"{required}>{{{{ form.{name} }}}}</textarea>"#),
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

//! `ocre generate scaffold`: model + HTML pages (askama) for full CRUD; with
//! `--realtime`, index pages that show other visitors' changes live.

use std::fmt::Write as _;

use super::{
    Edits,
    fields::{ATTACHMENT_TYPES, Field, FieldType, parse_fields},
    model::ensure_model,
    realtime::add_channel,
    register_routes,
};
use crate::{CliResult, names::ModelNames, project::Project};

pub fn scaffold(project: &Project, name: &str, specs: &[String], realtime: bool) -> CliResult {
    let names = ModelNames::parse(name)?;
    let fields = parse_fields(specs)?;
    let command = format!("ocre g scaffold {name} {}{}", specs.join(" "), if realtime { " --realtime" } else { "" });
    let mut edits = Edits::new(project);
    ensure_model(&mut edits, &names, &fields, &command)?;
    let plural = &names.plural;
    edits.create(&format!("src/{plural}.rs"), controller_rs(&names, &fields, &command, realtime))?;
    let mut templates = vec![
        ("index.html", index_html(&names, &fields, realtime)),
        ("show.html", show_html(&names, &fields)),
        ("new.html", page_html(&names, &fields, false)),
        ("edit.html", page_html(&names, &fields, true)),
        ("_form.html", form_fields_html(&fields)),
    ];
    if realtime {
        templates.push(("_row.html", row_html(&names, &fields)));
    }
    for (file, contents) in templates {
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
        (ty, false) if ty.is_numeric() => format!("v.number(\"{name}\", &self.{name}).unwrap_or_default()"),
        (ty, true) if ty.is_numeric() => format!("v.optional_number(\"{name}\", &self.{name})"),
        (_, false) => format!("self.{name}.clone()"),
        (_, true) => format!("(!self.{name}.trim().is_empty()).then(|| self.{name}.clone())"),
    }
}

/// Expression turning the model value into form text.
fn to_form(field: &Field, record: &str) -> String {
    let name = &field.name;
    match (field.ty, field.optional) {
        (FieldType::Boolean, _) => format!("{record}.{name}"),
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
    render(&IndexView {{ flash, {plural}: {singular}::all(&ctx, page).await? }})
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
            Ok(Redirect::to(&format!("/{plural}/{{}}", record.id)).into_response())
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
            Ok(Redirect::to(&format!("/{plural}/{{}}", record.id)).into_response())
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
    Ok(Redirect::to("/{plural}"))
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

/// Flash messages set by the previous request (create, update, delete).
const FLASH: &str = r#"{% if let Some(notice) = flash.notice() %}<p class="notice">{{ notice }}</p>{% endif %}
{% if let Some(alert) = flash.alert() %}<p class="alert">{{ alert }}</p>{% endif %}"#;

/// `{{ post.title }}`, or an `if let` for optional values. Attachments show
/// their file name.
fn display(field: &Field, record: &str) -> String {
    let name = &field.name;
    if field.is_attachment() {
        return if field.optional {
            format!("{{% if let Some(file) = {record}.{name}() %}}{{{{ file.filename }}}}{{% endif %}}")
        } else {
            format!("{{{{ {record}.{name}_filename }}}}")
        };
    }
    if field.optional {
        format!("{{% if let Some(value) = {record}.{name} %}}{{{{ value }}}}{{% endif %}}")
    } else {
        format!("{{{{ {record}.{name} }}}}")
    }
}

/// With `realtime`, the table sits in a WebSocket connection to the
/// `<plural>` channel, and rows come from `_row.html` with an `id` so
/// broadcasts can replace or remove them.
fn index_html(names: &ModelNames, fields: &[Field], realtime: bool) -> String {
    let ModelNames { singular, plural, human_singular, human_plural, .. } = names;
    let lower = human_singular.to_lowercase();
    let headers: String = fields.iter().map(|f| format!("<th>{}</th>", f.label())).collect();
    let (open, tbody, row, close) = if realtime {
        (
            format!(
                "{{# Live updates: htmx's WebSocket extension swaps in the rows other visitors create, edit and delete (src/realtime.rs). #}}\n\
                 <script src=\"https://unpkg.com/htmx-ext-ws@2.0.4/dist/ws.js\" crossorigin=\"anonymous\"></script>\n\
                 <div hx-ext=\"ws\" ws-connect=\"/realtime/{plural}\">\n"
            ),
            format!("<tbody id=\"{plural}\">"),
            format!("{{% include \"{plural}/_row.html\" %}}"),
            "\n</div>",
        )
    } else {
        (String::new(), "<tbody>".to_owned(), row_tr(names, fields, ""), "")
    };
    format!(
        r#"{{% extends "layout.html" %}}

{{% block title %}}{human_plural}{{% endblock %}}

{{% block content %}}
<h1>{human_plural}</h1>
{FLASH}
<p><a href="/{plural}/new">New {lower}</a></p>

{open}<table>
  <thead>
    <tr>{headers}<th></th></tr>
  </thead>
  {tbody}
    {{% for {singular} in {plural} %}}
    {row}
    {{% endfor %}}
  </tbody>
</table>{close}
{{% endblock %}}
"#
    )
}

/// `templates/<plural>/_row.html` (`--realtime`): one row, with the id
/// broadcasts target (`post_12`).
fn row_html(names: &ModelNames, fields: &[Field]) -> String {
    let singular = &names.singular;
    format!("{}\n", row_tr(names, fields, &format!(" id=\"{singular}_{{{{ {singular}.id }}}}\"")))
}

fn row_tr(names: &ModelNames, fields: &[Field], attributes: &str) -> String {
    let ModelNames { singular, plural, .. } = names;
    let cells: String = fields.iter().map(|f| format!("<td>{}</td>", display(f, singular))).collect();
    format!(
        "<tr{attributes}>{cells}<td><a href=\"/{plural}/{{{{ {singular}.id }}}}\">Show</a> <a href=\"/{plural}/{{{{ {singular}.id }}}}/edit\">Edit</a></td></tr>"
    )
}

fn show_html(names: &ModelNames, fields: &[Field]) -> String {
    let ModelNames { singular, plural, human_singular, .. } = names;
    let lower = human_singular.to_lowercase();
    let mut rows = String::new();
    for field in fields {
        let value = if field.is_attachment() { file_link(field, singular, plural) } else { display(field, singular) };
        writeln!(rows, "  <dt>{}</dt><dd>{value}</dd>", field.label()).expect("writing to a String");
    }
    format!(
        r#"{{% extends "layout.html" %}}

{{% block title %}}{human_singular} {{{{ {singular}.id }}}}{{% endblock %}}

{{% block content %}}
<h1>{human_singular} {{{{ {singular}.id }}}}</h1>
{FLASH}

<dl>
{rows}  <dt>Created at</dt><dd>{{{{ {singular}.created_at }}}}</dd>
  <dt>Updated at</dt><dd>{{{{ {singular}.updated_at }}}}</dd>
</dl>

<p><a href="/{plural}/{{{{ {singular}.id }}}}/edit">Edit</a> · <a href="/{plural}">Back</a></p>

<form action="/{plural}/{{{{ {singular}.id }}}}/delete" method="post" onsubmit="return confirm('Delete this {lower}?')">
  <button type="submit">Delete {lower}</button>
</form>
{{% endblock %}}
"#
    )
}

/// `<a href="/posts/{{ post.id }}/avatar">me.png</a> (12 KB)` on the show page.
fn file_link(field: &Field, singular: &str, plural: &str) -> String {
    let name = &field.name;
    let link = format!(
        "<a href=\"/{plural}/{{{{ {singular}.id }}}}/{name}\">{{{{ file.filename }}}}</a> ({{{{ file.human_size() }}}})"
    );
    if field.optional {
        format!("{{% if let Some(file) = {singular}.{name}() %}}{link}{{% endif %}}")
    } else {
        format!("{{% let file = {singular}.{name}() %}}{link}")
    }
}

/// new.html and edit.html: the form tag around `_form.html`. Forms with
/// files are multipart; edit pages add a "Remove" box per optional file.
fn page_html(names: &ModelNames, fields: &[Field], edit: bool) -> String {
    let ModelNames { plural, human_singular, .. } = names;
    let lower = human_singular.to_lowercase();
    let (title, action, submit, back) = if edit {
        (
            format!("Edit {lower}"),
            format!("/{plural}/{{{{ id }}}}"),
            format!("Update {lower}"),
            format!("/{plural}/{{{{ id }}}}"),
        )
    } else {
        (format!("New {lower}"), format!("/{plural}"), format!("Create {lower}"), format!("/{plural}"))
    };
    let enctype = if fields.iter().any(Field::is_attachment) { r#" enctype="multipart/form-data""# } else { "" };
    let mut removes = String::new();
    for field in fields.iter().filter(|f| edit && f.is_attachment() && f.optional) {
        writeln!(
            removes,
            r#"  <label><input type="checkbox" name="remove_{}" value="true"> Remove {}</label>"#,
            field.name,
            field.label().to_lowercase()
        )
        .expect("writing to a String");
    }
    format!(
        r#"{{% extends "layout.html" %}}

{{% block title %}}{title}{{% endblock %}}

{{% block content %}}
<h1>{title}</h1>

<form action="{action}" method="post"{enctype}>
{{% include "{plural}/_form.html" %}}
{removes}  <button type="submit">{submit}</button>
</form>

<p><a href="{back}">Back</a></p>
{{% endblock %}}
"#
    )
}

/// Error list and one labelled input per field; `form` and `errors` come from
/// the page that includes it.
fn form_fields_html(fields: &[Field]) -> String {
    let mut out = String::from(
        "{% if !errors.is_empty() %}\n  <ul class=\"errors\">\n    {% for error in errors %}<li>{{ error.full_message() }}</li>{% endfor %}\n  </ul>\n{% endif %}\n",
    );
    for field in fields {
        let (name, label) = (&field.name, field.label());
        let required = if field.optional { "" } else { " required" };
        let input = match field.ty {
            FieldType::String => format!(r#"<input name="{name}" value="{{{{ form.{name} }}}}"{required}>"#),
            FieldType::Text => {
                format!(r#"<textarea name="{name}" rows="5"{required}>{{{{ form.{name} }}}}</textarea>"#)
            }
            FieldType::Integer | FieldType::References => {
                format!(r#"<input type="number" step="1" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#)
            }
            FieldType::Float => {
                format!(r#"<input type="number" step="any" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#)
            }
            FieldType::Date => format!(r#"<input type="date" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#),
            FieldType::DateTime => {
                format!(r#"<input type="datetime-local" name="{name}" value="{{{{ form.{name} }}}}"{required}>"#)
            }
            FieldType::Boolean => {
                format!(
                    r#"<input type="checkbox" name="{name}" value="true"{{% if form.{name} %}} checked{{% endif %}}>"#
                )
            }
            // Not `required`: the edit form keeps the stored file when none is chosen.
            FieldType::Attachment => {
                format!(r#"<input type="file" name="{name}" accept="{}">"#, ATTACHMENT_TYPES.join(","))
            }
        };
        writeln!(out, "  <label>{label} {input}</label>").expect("writing to a String");
    }
    out
}

//! `ocre generate scaffold|api|migration`.

mod api;

pub use api::api;

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use crate::{
    CliResult,
    names::{ModelNames, humanize, is_identifier},
    output::{CliError, Report},
    project::Project,
};

const MODULES_MARKER: &str = "// ocre:modules";
const ROUTES_MARKER: &str = "// ocre:routes";

/// Names that would clash with generated columns, Rust keywords or SQL keywords.
const RESERVED_FIELDS: &[&str] = &[
    "id",
    "created_at",
    "updated_at", // generated columns
    "as",
    "async",
    "await",
    "box",
    "break",
    "const",
    "continue",
    "crate",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "fn",
    "for",
    "gen",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "match",
    "mod",
    "move",
    "mut",
    "pub",
    "ref",
    "return",
    "self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "try",
    "type",
    "unsafe",
    "use",
    "where",
    "while",
    "yield",
    "and",
    "asc",
    "by",
    "case",
    "check",
    "default",
    "desc",
    "from",
    "group",
    "index",
    "join",
    "key",
    "limit",
    "not",
    "null",
    "offset",
    "or",
    "order",
    "primary",
    "references",
    "select",
    "table",
    "unique",
    "values",
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum FieldType {
    String,
    Text,
    Integer,
    Float,
    Boolean,
}

impl FieldType {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "string" => Self::String,
            "text" => Self::Text,
            "integer" => Self::Integer,
            "float" => Self::Float,
            "boolean" => Self::Boolean,
            _ => return None,
        })
    }

    fn rust_type(self) -> &'static str {
        match self {
            Self::String | Self::Text => "String",
            Self::Integer => "i64",
            Self::Float => "f64",
            Self::Boolean => "bool",
        }
    }

    fn sql_column(self) -> &'static str {
        match self {
            Self::String | Self::Text => "TEXT NOT NULL",
            Self::Integer => "INTEGER NOT NULL",
            Self::Float => "REAL NOT NULL",
            Self::Boolean => "INTEGER NOT NULL DEFAULT 0",
        }
    }

    fn is_textual(self) -> bool {
        matches!(self, Self::String | Self::Text)
    }
}

#[derive(Debug, PartialEq)]
struct Field {
    name: String,
    ty: FieldType,
}

impl Field {
    fn parse(spec: &str) -> Result<Self, CliError> {
        let (name, ty) = spec.split_once(':').ok_or_else(|| {
            CliError::new(format!("field `{spec}` has no type"))
                .hint("write fields as `name:type`, e.g. `title:string`")
        })?;
        if !is_identifier(name) {
            return Err(CliError::new(format!("invalid field name `{name}`"))
                .hint("use snake_case starting with a letter, e.g. `published_at`"));
        }
        if RESERVED_FIELDS.contains(&name) {
            return Err(CliError::new(format!("field name `{name}` is reserved"))
                .hint("`id`, `created_at` and `updated_at` are generated; Rust and SQL keywords are not allowed. Pick another name, e.g. `kind` for `type`"));
        }
        let ty = FieldType::parse(ty).ok_or_else(|| {
            CliError::new(format!("unknown field type `{ty}` for `{name}`"))
                .hint("types: string, text, integer, float, boolean")
        })?;
        Ok(Self { name: name.to_owned(), ty })
    }

    fn label(&self) -> String {
        humanize(&self.name)
    }
}

fn parse_fields(specs: &[String]) -> Result<Vec<Field>, CliError> {
    let fields = specs.iter().map(|spec| Field::parse(spec)).collect::<Result<Vec<_>, _>>()?;
    for (i, field) in fields.iter().enumerate() {
        if fields[..i].iter().any(|f| f.name == field.name) {
            return Err(CliError::new(format!("field `{}` is listed twice", field.name)));
        }
    }
    Ok(fields)
}

pub fn scaffold(project: &Project, name: &str, field_specs: &[String]) -> CliResult {
    let names = ModelNames::parse(name)?;
    let fields = parse_fields(field_specs)?;

    let module_path = project.root.join("src").join(format!("{}.rs", names.plural));
    let template_dir = project.root.join("templates").join(&names.plural);
    for existing in [&module_path, &template_dir] {
        if existing.exists() {
            return Err(CliError::new(format!("{} already exists", project.relative(existing)))
                .hint("scaffold creates new resources only; edit the existing files, or add a migration with `ocre g migration`"));
        }
    }
    // Validate lib.rs before writing anything, so a failure leaves no partial scaffold.
    let lib_path = project.root.join("src/lib.rs");
    let lib = register_module(&std::fs::read_to_string(&lib_path)?, &names.plural)?;
    let migration = create_table_migration(project, &names, &fields)?;

    let mut files: Vec<(PathBuf, String)> = migration.into_iter().collect();
    files.extend([
        (module_path, module_rs(&names, &fields, name, field_specs)),
        (template_dir.join("index.html"), index_html(&names, &fields)),
        (template_dir.join("show.html"), show_html(&names, &fields)),
        (template_dir.join("new.html"), form_html(&names, &fields, false)),
        (template_dir.join("edit.html"), form_html(&names, &fields, true)),
    ]);
    let mut report = Report::new("generate scaffold");
    for (path, contents) in files {
        std::fs::create_dir_all(path.parent().expect("generated files have a parent"))?;
        std::fs::write(&path, contents)?;
        report.created.push(project.relative(&path));
    }
    std::fs::write(&lib_path, lib)?;
    report.updated.push(project.relative(&lib_path));
    report.next =
        vec!["ocre migrate".to_owned(), "ocre dev".to_owned(), format!("open http://localhost:8787/{}", names.plural)];
    Ok(report)
}

pub fn migration(project: &Project, name: &str) -> CliResult {
    if !is_identifier(name) {
        return Err(
            CliError::new(format!("invalid migration name `{name}`")).hint("use snake_case, e.g. `add_slug_to_posts`")
        );
    }
    let path = next_migration_path(&project.root, name)?;
    std::fs::create_dir_all(path.parent().expect("migration has a parent"))?;
    let header =
        format!("-- Migration: {name}\n-- Applied once, in file-name order. Never edit after it has been applied.\n");
    std::fs::write(&path, header)?;
    let mut report = Report::new("generate migration");
    report.created.push(project.relative(&path));
    report.next = vec!["ocre migrate".to_owned()];
    Ok(report)
}

/// The `CREATE TABLE` migration for a resource, unless one already exists
/// (for example `ocre g api` after `ocre g scaffold` for the same model).
fn create_table_migration(
    project: &Project,
    names: &ModelNames,
    fields: &[Field],
) -> Result<Option<(PathBuf, String)>, CliError> {
    let name = format!("create_{}", names.plural);
    let dir = project.root.join("migrations");
    let suffix = format!("_{name}.sql");
    if dir.is_dir() {
        for entry in std::fs::read_dir(&dir)? {
            if entry?.file_name().to_string_lossy().ends_with(&suffix) {
                return Ok(None);
            }
        }
    }
    Ok(Some((next_migration_path(&project.root, &name)?, migration_sql(names, fields))))
}
/// `migrations/NNNN_<name>.sql`, numbered after the highest existing migration.
fn next_migration_path(root: &Path, name: &str) -> Result<PathBuf, CliError> {
    let dir = root.join("migrations");
    let mut highest = 0u32;
    if dir.is_dir() {
        for entry in std::fs::read_dir(&dir)? {
            let file_name = entry?.file_name();
            let digits: String = file_name.to_string_lossy().chars().take_while(char::is_ascii_digit).collect();
            if let Ok(number) = digits.parse::<u32>() {
                highest = highest.max(number);
            }
        }
    }
    Ok(dir.join(format!("{:04}_{name}.sql", highest + 1)))
}

/// Adds `mod <module>;` and `.merge(<module>::routes())` after the markers.
fn register_module(lib: &str, module: &str) -> Result<String, CliError> {
    let mut out = String::with_capacity(lib.len() + 64);
    let (mut found_modules, mut found_routes) = (false, false);
    for line in lib.lines() {
        out.push_str(line);
        out.push('\n');
        let trimmed = line.trim();
        if trimmed == MODULES_MARKER {
            found_modules = true;
            writeln!(out, "mod {module};").expect("writing to a String");
        } else if trimmed == ROUTES_MARKER {
            found_routes = true;
            let indent = &line[..line.len() - line.trim_start().len()];
            writeln!(out, "{indent}.merge({module}::routes())").expect("writing to a String");
        }
    }
    if found_modules && found_routes {
        Ok(out)
    } else {
        Err(CliError::new("src/lib.rs is missing the `// ocre:modules` or `// ocre:routes` marker").hint(
            "put `// ocre:modules` on its own line where `mod` declarations go, and `// ocre:routes` inside the `Router::new()` chain",
        ))
    }
}

fn migration_sql(names: &ModelNames, fields: &[Field]) -> String {
    let mut sql = format!("CREATE TABLE {} (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n", names.plural);
    for field in fields {
        writeln!(sql, "    {} {},", field.name, field.ty.sql_column()).expect("writing to a String");
    }
    sql.push_str("    created_at TEXT NOT NULL DEFAULT (datetime('now')),\n");
    sql.push_str("    updated_at TEXT NOT NULL DEFAULT (datetime('now'))\n);\n");
    sql
}

fn module_rs(names: &ModelNames, fields: &[Field], name: &str, field_specs: &[String]) -> String {
    let ModelNames { model, singular, plural, .. } = names;
    let columns = fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ");
    let placeholders = (1..=fields.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
    let assignments =
        fields.iter().enumerate().map(|(i, f)| format!("{} = ?{}", f.name, i + 1)).collect::<Vec<_>>().join(", ");
    let id_placeholder = fields.len() + 1;
    let form_params = fields.iter().map(|f| format!("form.{}", f.name)).collect::<Vec<_>>().join(", ");

    let mut model_fields = String::new();
    let mut form_fields = String::new();
    let mut checks = String::new();
    for field in fields {
        let (name, ty) = (&field.name, field.ty.rust_type());
        if field.ty == FieldType::Boolean {
            model_fields.push_str("    #[serde(deserialize_with = \"ocre::bool_from_sql\")]\n");
            form_fields.push_str("    /// Unchecked checkboxes are not submitted.\n    #[serde(default)]\n");
        }
        writeln!(model_fields, "    pub {name}: {ty},").expect("writing to a String");
        writeln!(form_fields, "    pub {name}: {ty},").expect("writing to a String");
        if field.ty.is_textual() {
            writeln!(
                checks,
                "        if self.{name}.trim().is_empty() {{\n            return Err(Error::bad_request(\"{} is required.\"));\n        }}",
                field.label()
            )
            .expect("writing to a String");
        }
        if field.ty == FieldType::Integer {
            writeln!(
                checks,
                "        if self.{name}.unsigned_abs() > ocre::MAX_SAFE_INTEGER as u64 {{\n            return Err(Error::bad_request(\"{} is out of range.\"));\n        }}",
                field.label()
            )
            .expect("writing to a String");
        }
    }
    let command = format!("ocre g scaffold {name} {}", field_specs.join(" "));

    format!(
        r#"//! {human_plural}. Generated by `{command}`.

use askama::Template;
use axum::{{
    Form, Router,
    extract::{{Path, State}},
    response::{{Html, Redirect}},
    routing::{{get, post}},
}};
use ocre::{{Ctx, Error, OptionExt, Result, params, render}};
use serde::Deserialize;

pub fn routes() -> Router<Ctx> {{
    Router::new()
        .route("/{plural}", get(index).post(create))
        .route("/{plural}/new", get(new))
        .route("/{plural}/{{id}}", get(show).post(update))
        .route("/{plural}/{{id}}/edit", get(edit))
        .route("/{plural}/{{id}}/delete", post(delete))
}}

/// Row of the `{plural}` table.
#[derive(Deserialize)]
pub struct {model} {{
    pub id: i64,
{model_fields}    pub created_at: String,
    pub updated_at: String,
}}

/// Submitted by the new and edit forms.
#[derive(Deserialize)]
pub struct {model}Form {{
{form_fields}}}

impl {model}Form {{
    fn validate(&self) -> Result<()> {{
{checks}        Ok(())
    }}
}}

#[derive(Template)]
#[template(path = "{plural}/index.html")]
struct IndexView {{
    {plural}: Vec<{model}>,
}}

#[derive(Template)]
#[template(path = "{plural}/show.html")]
struct ShowView {{
    {singular}: {model},
}}

#[derive(Template)]
#[template(path = "{plural}/new.html")]
struct NewView;

#[derive(Template)]
#[template(path = "{plural}/edit.html")]
struct EditView {{
    {singular}: {model},
}}

async fn find(ctx: &Ctx, id: i64) -> Result<{model}> {{
    ctx.db()?.first("SELECT * FROM {plural} WHERE id = ?1", params![id]).await?.or_404()
}}

async fn index(State(ctx): State<Ctx>) -> Result<Html<String>> {{
    let {plural} = ctx.db()?.all("SELECT * FROM {plural} ORDER BY id DESC", params![]).await?;
    render(&IndexView {{ {plural} }})
}}

async fn show(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Html<String>> {{
    render(&ShowView {{ {singular}: find(&ctx, id).await? }})
}}

async fn new() -> Result<Html<String>> {{
    render(&NewView)
}}

async fn create(State(ctx): State<Ctx>, Form(form): Form<{model}Form>) -> Result<Redirect> {{
    form.validate()?;
    let {singular}: {model} = ctx
        .db()?
        .first(
            "INSERT INTO {plural} ({columns}) VALUES ({placeholders}) RETURNING *",
            params![{form_params}],
        )
        .await?
        .ok_or_else(|| Error::internal("INSERT ... RETURNING returned no row"))?;
    Ok(Redirect::to(&format!("/{plural}/{{}}", {singular}.id)))
}}

async fn edit(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Html<String>> {{
    render(&EditView {{ {singular}: find(&ctx, id).await? }})
}}

async fn update(State(ctx): State<Ctx>, Path(id): Path<i64>, Form(form): Form<{model}Form>) -> Result<Redirect> {{
    form.validate()?;
    let changed = ctx
        .db()?
        .execute(
            "UPDATE {plural} SET {assignments}, updated_at = datetime('now') WHERE id = ?{id_placeholder}",
            params![{form_params}, id],
        )
        .await?;
    if changed == 0 {{
        return Err(Error::NotFound);
    }}
    Ok(Redirect::to(&format!("/{plural}/{{id}}")))
}}

async fn delete(State(ctx): State<Ctx>, Path(id): Path<i64>) -> Result<Redirect> {{
    let changed = ctx.db()?.execute("DELETE FROM {plural} WHERE id = ?1", params![id]).await?;
    if changed == 0 {{
        return Err(Error::NotFound);
    }}
    Ok(Redirect::to("/{plural}"))
}}
"#,
        human_plural = names.human_plural,
    )
}

fn index_html(names: &ModelNames, fields: &[Field]) -> String {
    let ModelNames { singular, plural, human_singular, human_plural, .. } = names;
    let lower = human_singular.to_lowercase();
    let headers: String = fields.iter().map(|f| format!("<th>{}</th>", f.label())).collect();
    let cells: String = fields.iter().map(|f| format!("<td>{{{{ {singular}.{} }}}}</td>", f.name)).collect();
    format!(
        r#"{{% extends "layout.html" %}}

{{% block title %}}{human_plural}{{% endblock %}}

{{% block content %}}
<h1>{human_plural}</h1>
<p><a href="/{plural}/new">New {lower}</a></p>

<table>
  <thead>
    <tr>{headers}<th></th></tr>
  </thead>
  <tbody>
    {{% for {singular} in {plural} %}}
    <tr>{cells}<td><a href="/{plural}/{{{{ {singular}.id }}}}">Show</a> <a href="/{plural}/{{{{ {singular}.id }}}}/edit">Edit</a></td></tr>
    {{% endfor %}}
  </tbody>
</table>
{{% endblock %}}
"#
    )
}

fn show_html(names: &ModelNames, fields: &[Field]) -> String {
    let ModelNames { singular, plural, human_singular, .. } = names;
    let lower = human_singular.to_lowercase();
    let mut rows = String::new();
    for field in fields {
        writeln!(rows, "  <dt>{}</dt><dd>{{{{ {singular}.{} }}}}</dd>", field.label(), field.name)
            .expect("writing to a String");
    }
    format!(
        r#"{{% extends "layout.html" %}}

{{% block title %}}{human_singular} {{{{ {singular}.id }}}}{{% endblock %}}

{{% block content %}}
<h1>{human_singular} {{{{ {singular}.id }}}}</h1>

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

fn form_html(names: &ModelNames, fields: &[Field], edit: bool) -> String {
    let ModelNames { singular, plural, human_singular, .. } = names;
    let lower = human_singular.to_lowercase();
    let value = |field: &Field| {
        if edit { format!("{{{{ {singular}.{} }}}}", field.name) } else { String::new() }
    };
    let mut inputs = String::new();
    for field in fields {
        let (name, label, value) = (&field.name, field.label(), value(field));
        let value_attr = if edit { format!(r#" value="{value}""#) } else { String::new() };
        let input = match field.ty {
            FieldType::String => format!(r#"<input name="{name}"{value_attr} required>"#),
            FieldType::Text => format!(r#"<textarea name="{name}" rows="5" required>{value}</textarea>"#),
            FieldType::Integer => format!(r#"<input type="number" step="1" name="{name}"{value_attr} required>"#),
            FieldType::Float => format!(r#"<input type="number" step="any" name="{name}"{value_attr} required>"#),
            FieldType::Boolean => {
                let checked =
                    if edit { format!("{{% if {singular}.{name} %}} checked{{% endif %}}") } else { String::new() };
                format!(r#"<input type="checkbox" name="{name}" value="true"{checked}>"#)
            }
        };
        writeln!(inputs, "  <label>{label} {input}</label>").expect("writing to a String");
    }
    let (title, action, submit, back) = if edit {
        (
            format!("Edit {lower}"),
            format!("/{plural}/{{{{ {singular}.id }}}}"),
            format!("Update {lower}"),
            format!("/{plural}/{{{{ {singular}.id }}}}"),
        )
    } else {
        (format!("New {lower}"), format!("/{plural}"), format!("Create {lower}"), format!("/{plural}"))
    };
    format!(
        r#"{{% extends "layout.html" %}}

{{% block title %}}{title}{{% endblock %}}

{{% block content %}}
<h1>{title}</h1>

<form action="{action}" method="post">
{inputs}  <button type="submit">{submit}</button>
</form>

<p><a href="{back}">Back</a></p>
{{% endblock %}}
"#
    )
}

#[cfg(test)]
mod tests;

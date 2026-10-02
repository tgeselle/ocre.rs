//! `ocre generate controller`: a module of GET actions, HTML pages (askama)
//! in a full-stack app, JSON under /api in an API-only app or with `--api`.
//! `ocre generate resource`: a model plus index and show actions over it.
//!
//! Both render the templates of [`super::templates`], which apps can override.

use minijinja::Value;
use serde::Serialize;

use super::{
    Edits,
    fields::{FieldType, parse_model_fields},
    model::ensure_model,
    register_routes,
    templates::render,
};
use crate::{
    CliResult,
    names::{ModelNames, RUST_KEYWORDS, humanize, is_identifier, split_words},
    output::CliError,
    project::Project,
};

#[derive(Serialize)]
struct Action {
    /// `about_us`: handler, view and template name.
    name: String,
    /// `AboutUs`: `AboutUsView` / `AboutUsResponse`.
    pascal: String,
    /// `About us`: page title.
    human: String,
    /// `/pages/about_us`
    path: String,
}

#[derive(Serialize)]
struct ControllerContext<'a> {
    command: &'a str,
    /// `pages`: module, URL segment and template directory.
    module: &'a str,
    /// `pages` or `pages_api`: src/<file>.rs.
    file: &'a str,
    /// `Pages`
    human: String,
    actions: Vec<Action>,
    /// `--auth`: handlers take the signed-in user.
    auth: bool,
}

pub fn controller(project: &Project, name: &str, actions: &[String], api: bool, auth: bool) -> CliResult {
    let module = module_name(name)?;
    let actions = if actions.is_empty() { vec!["index".to_owned()] } else { action_names(actions)? };
    let json = api || project.api_only;
    let file = if json { format!("{module}_api") } else { module.clone() };
    let prefix = if json { format!("/api/{module}") } else { format!("/{module}") };
    let auth_module = if json { "src/auth_api.rs" } else { "src/auth.rs" };
    if auth && !project.root.join(auth_module).is_file() {
        return Err(CliError::new(format!("--auth needs {auth_module}, which `ocre g auth` creates"))
            .hint("run `ocre g auth` and `ocre migrate` first, or generate the controller without --auth"));
    }
    let flags = format!("{}{}", if api { " --api" } else { "" }, if auth { " --auth" } else { "" });
    let command = format!("ocre g controller {name} {}{flags}", actions.join(" "));
    let context = ControllerContext {
        command: &command,
        module: &module,
        file: &file,
        human: humanize(&module),
        actions: actions
            .iter()
            .map(|action| Action {
                pascal: super::mailer::pascal(action),
                human: humanize(action),
                path: if action == "index" { prefix.clone() } else { format!("{prefix}/{action}") },
                name: action.clone(),
            })
            .collect(),
        auth,
    };
    let mut edits = Edits::new(project);
    let values = Value::from_serialize(&context);
    let source = render(&edits, if json { "controller/api.rs" } else { "controller/html.rs" }, &values)?;
    edits.create(&format!("src/{file}.rs"), source)?;
    if !json {
        for action in &context.actions {
            let view = render(&edits, "controller/view.html", &minijinja::context! { module, action })?;
            edits.create(&format!("templates/{module}/{}.html", action.name), view)?;
        }
    }
    register_routes(&mut edits, &file)?;
    let mut report = edits.apply("generate controller")?;
    let first = &context.actions[0].path;
    report.next = vec![
        "ocre dev".to_owned(),
        if json { format!("curl http://localhost:8787{first}") } else { format!("open http://localhost:8787{first}") },
    ];
    Ok(report)
}

/// `PagesController`, `pages_controller` or `Pages` -> `pages`.
fn module_name(name: &str) -> Result<String, CliError> {
    let mut words = split_words(name);
    if words.len() > 1 && words.last().is_some_and(|w| w == "controller") {
        words.pop();
    }
    let module = words.join("_");
    if !is_identifier(&module) || RUST_KEYWORDS.contains(&module.as_str()) || module.ends_with("_api") {
        return Err(CliError::new(format!("invalid controller name `{name}`")).hint(
            "use a name starting with a letter that is not a Rust keyword and does not end in `api`, e.g. `Pages` or `Dashboard`",
        ));
    }
    Ok(module)
}

/// Action names as snake_case function names, each once.
fn action_names(actions: &[String]) -> Result<Vec<String>, CliError> {
    let mut names: Vec<String> = Vec::with_capacity(actions.len());
    for action in actions {
        let name = split_words(action).join("_");
        if !is_identifier(&name)
            || RUST_KEYWORDS.contains(&name.as_str())
            || matches!(name.as_str(), "routes" | "paths")
        {
            return Err(CliError::new(format!("invalid action name `{action}`")).hint(
                "use snake_case starting with a letter, not a Rust keyword nor `routes`/`paths`, e.g. `about` or `contact_us`",
            ));
        }
        if names.contains(&name) {
            return Err(CliError::new(format!("action `{name}` is listed twice")).hint("list each action once"));
        }
        names.push(name);
    }
    Ok(names)
}

#[derive(Serialize)]
struct ResourceField {
    label: String,
    /// askama expression showing the value (`{{ post.title }}`).
    display: String,
}

#[derive(Serialize)]
struct ResourceContext<'a> {
    command: &'a str,
    model: &'a str,
    singular: &'a str,
    plural: &'a str,
    human_singular: &'a str,
    human_plural: &'a str,
    /// The field links use: `id`, or `public_id` with `public_id:token`.
    key: &'a str,
    fields: Vec<ResourceField>,
}

pub fn resource(project: &Project, name: &str, specs: &[String], api: bool) -> CliResult {
    let names = ModelNames::parse(name)?;
    let (fields, many) = parse_model_fields(specs)?;
    let json = api || project.api_only;
    let command = format!("ocre g resource {name} {}{}", specs.join(" "), if api { " --api" } else { "" });
    let mut edits = Edits::new(project);
    ensure_model(&mut edits, &names, &fields, &many, &command)?;
    let public_id = fields.iter().any(|f| f.ty == FieldType::PublicId);
    let fields: Vec<_> = fields.into_iter().filter(|f| f.ty != FieldType::PublicId).collect();
    let ModelNames { model, singular, plural, human_singular, human_plural } = &names;
    let context = ResourceContext {
        key: if public_id { "public_id" } else { "id" },
        command: &command,
        model,
        singular,
        plural,
        human_singular,
        human_plural,
        fields: fields
            .iter()
            .map(|field| ResourceField { label: field.label(), display: field.display(singular) })
            .collect(),
    };
    let module = if json { format!("{plural}_api") } else { plural.clone() };
    let values = Value::from_serialize(&context);
    let mut source = render(&edits, if json { "resource/api.rs" } else { "resource/html.rs" }, &values)?;
    if public_id {
        source = super::public_id::resource(&source, &names, json);
    }
    edits.create(&format!("src/{module}.rs"), source)?;
    if !json {
        for view in ["index", "show"] {
            let source = render(&edits, &format!("resource/{view}.html"), &values)?;
            edits.create(&format!("templates/{plural}/{view}.html"), source)?;
        }
    }
    register_routes(&mut edits, &module)?;
    let mut report = edits.apply("generate resource")?;
    let url = if json {
        format!("curl http://localhost:8787/api/{plural}")
    } else {
        format!("open http://localhost:8787/{plural}")
    };
    report.next = vec!["ocre migrate".to_owned(), "ocre dev".to_owned(), url];
    Ok(report)
}

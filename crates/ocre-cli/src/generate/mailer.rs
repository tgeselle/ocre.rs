//! `ocre generate mailer`: `src/mailers/<name>.rs`, one function per action
//! building an `ocre::mail::Email`, with askama text and HTML templates
//! (extending shared layouts) in full-stack apps. The first mailer creates
//! `src/mailers/mod.rs` with the app-wide `defaults` and the `PREVIEWS`
//! served at `/ocre/dev/mailers` by `ocre dev`.

use std::fmt::Write as _;

use super::{Edits, MODULES_MARKER, ROUTES_MARKER, fields::RESERVED, insert_after_marker};
use crate::{
    CliResult,
    names::{humanize, is_identifier, split_words},
    output::CliError,
    project::Project,
};

pub(super) const MAILERS_MARKER: &str = "// ocre:mailers";
const PREVIEWS_MARKER: &str = "// ocre:mailer-previews";
/// The development pages without mailers, as `ocre g mailbox` merges them.
pub(super) const DEV_ROUTES_EMPTY: &str = ".merge(ocre::mail::dev_routes(&[]))";
const DEV_ROUTES: &str = ".merge(ocre::mail::dev_routes(mailers::PREVIEWS))";

const LAYOUT_HTML: &str = r#"{# Layout of every HTML email: `{% extends "mailers/layout.html" %}` in a mailer template. -#}
<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
</head>
<body style="margin: 0; padding: 24px; font-family: -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif; font-size: 16px; line-height: 1.5; color: #222;">
{% block content %}{% endblock %}
</body>
</html>
"#;

const LAYOUT_TXT: &str = r#"{# Layout of every text email: `{% extends "mailers/layout.txt" %}` in a mailer template. #}
{%- block content %}{% endblock %}
"#;

pub fn mailer(project: &Project, name: &str, actions: &[String]) -> CliResult {
    let module = module_name(name)?;
    let actions = action_names(actions)?;
    let command = format!("ocre g mailer {name} {}", actions.join(" "));
    let mut edits = Edits::new(project);
    // Apps whose registry predates `defaults` keep compiling.
    let defaults = edits.read("src/mailers/mod.rs")?.is_none_or(|registry| registry.contains("pub fn defaults"));
    let path = format!("src/mailers/{module}.rs");
    if project.api_only {
        edits.create(&path, api_mailer_rs(&module, &actions, &command, defaults))?;
    } else {
        edits.create(&path, mailer_rs(&module, &actions, &command, defaults))?;
        for action in &actions {
            edits.create(&format!("templates/mailers/{module}/{action}.txt"), text_template(&module, action))?;
            edits.create(&format!("templates/mailers/{module}/{action}.html"), html_template(&module, action))?;
        }
        for (path, layout) in
            [("templates/mailers/layout.html", LAYOUT_HTML), ("templates/mailers/layout.txt", LAYOUT_TXT)]
        {
            if !edits.exists(path) {
                edits.create(path, layout.to_owned())?;
            }
        }
    }
    register(&mut edits, &module, &actions)?;
    let mut report = edits.apply("generate mailer")?;
    report.next = vec![
        format!("send it from a handler: ocre::mail::send(&ctx, mailers::{module}::{}(&address)?).await?", actions[0]),
        "ocre dev, then open http://localhost:8787/ocre/dev/mailers to preview it (MAIL_ADAPTER=log in .dev.vars \
         prints each email sent instead of sending it)"
            .to_owned(),
    ];
    Ok(report)
}

/// `UserMailer`, `user_mailer` or `User` -> `user`.
fn module_name(name: &str) -> Result<String, CliError> {
    let mut words = split_words(name);
    if words.len() > 1 && words.last().is_some_and(|w| w == "mailer") {
        words.pop();
    }
    let module = words.join("_");
    if !is_identifier(&module) || RESERVED.contains(&module.as_str()) {
        return Err(CliError::new(format!("invalid mailer name `{name}`"))
            .hint("use a name starting with a letter that is not a Rust keyword, e.g. `User` or `Billing`"));
    }
    Ok(module)
}

/// Action names as snake_case function names, each once.
fn action_names(actions: &[String]) -> Result<Vec<String>, CliError> {
    let mut names: Vec<String> = Vec::with_capacity(actions.len());
    for action in actions {
        let name = split_words(action).join("_");
        if !is_identifier(&name) || RESERVED.contains(&name.as_str()) {
            return Err(CliError::new(format!("invalid action name `{action}`")).hint(
                "use snake_case starting with a letter, not a Rust or SQL keyword, e.g. `welcome` or `password_reset`",
            ));
        }
        if names.contains(&name) {
            return Err(CliError::new(format!("action `{name}` is listed twice")).hint("list each action once"));
        }
        names.push(name);
    }
    Ok(names)
}

/// `password_reset` -> `PasswordReset`.
pub(super) fn pascal(snake: &str) -> String {
    snake
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| first.to_ascii_uppercase().to_string() + chars.as_str())
        })
        .collect()
}

fn header(module: &str, command: &str, api_only: bool) -> String {
    let source = if api_only {
        "Each function builds an `Email` (text only; add `.html(...)` for HTML)".to_owned()
    } else {
        format!(
            "Each function builds an `Email` from templates/mailers/{module}/<action>.txt and .html, which extend \
             templates/mailers/layout.txt and .html"
        )
    };
    format!(
        "//! {} emails. Generated by `{command}`.\n//!\n//! {source}.\n//! Send one from a handler (`MAIL_ADAPTER` picks log, Resend or Cloudflare):\n//! `ocre::mail::send(&ctx, crate::mailers::{module}::<action>(&address)?).await?`\n\n",
        humanize(module),
    )
}

/// `Email::new(...)`, passed through the app's `defaults` when the registry has it.
fn finish(email: &str, defaults: bool) -> String {
    if defaults { format!("Ok(super::defaults({email}))") } else { format!("Ok({email})") }
}

fn mailer_rs(module: &str, actions: &[String], command: &str, defaults: bool) -> String {
    let mut out = header(module, command, false);
    out.push_str("use askama::Template;\nuse ocre::{Result, mail::Email};\n");
    for action in actions {
        let (pascal, subject) = (pascal(action), humanize(action));
        for (kind, extension) in [("Text", "txt"), ("Html", "html")] {
            write!(
                out,
                "\n#[derive(Template)]\n#[template(path = \"mailers/{module}/{action}.{extension}\")]\nstruct {pascal}{kind}<'a> {{\n    to: &'a str,\n}}\n"
            )
            .expect("writing to a String");
        }
        let email = finish(&format!("Email::new(to, \"{subject}\", text).html(html)"), defaults);
        write!(
            out,
            "\n/// \"{subject}\" email to `to`.\npub fn {action}(to: &str) -> Result<Email> {{\n    let text = {pascal}Text {{ to }}.render()?;\n    let html = {pascal}Html {{ to }}.render()?;\n    {email}\n}}\n"
        )
        .expect("writing to a String");
    }
    out
}

fn api_mailer_rs(module: &str, actions: &[String], command: &str, defaults: bool) -> String {
    let mut out = header(module, command, true);
    out.push_str("use ocre::{Result, mail::Email};\n");
    for action in actions {
        let subject = humanize(action);
        let email = finish(&format!("Email::new(to, \"{subject}\", text)"), defaults);
        write!(
            out,
            "\n/// \"{subject}\" email to `to`.\npub fn {action}(to: &str) -> Result<Email> {{\n    let text = format!(\"Hello {{to}},\\n\\nThis is the {} email. Edit it in src/mailers/{module}.rs.\\n\");\n    {email}\n}}\n",
            subject.to_lowercase(),
        )
        .expect("writing to a String");
    }
    out
}

fn text_template(module: &str, action: &str) -> String {
    format!(
        "{{% extends \"mailers/layout.txt\" %}}\n{{% block content -%}}\nHello {{{{ to }}}},\n\nThis is the {} email. Edit templates/mailers/{module}/{action}.txt.\n{{%- endblock %}}\n",
        humanize(action).to_lowercase()
    )
}

fn html_template(module: &str, action: &str) -> String {
    format!(
        "{{% extends \"mailers/layout.html\" %}}\n{{% block content %}}\n<p>Hello {{{{ to }}}},</p>\n<p>This is the {} email. Edit templates/mailers/{module}/{action}.html.</p>\n{{% endblock %}}\n",
        humanize(action).to_lowercase()
    )
}

fn registry_rs() -> String {
    format!(
        r#"//! Mailers: functions that build emails. `ocre g mailer` adds them below.
//!
//! What every mailer shares, like Rails' ApplicationMailer: `defaults` runs
//! on each email they build, HTML and text templates extend
//! templates/mailers/layout.html and layout.txt, and `PREVIEWS` are listed
//! at http://localhost:8787/ocre/dev/mailers while `ocre dev` runs.

// A mailer is often generated before a handler sends it.
#![allow(dead_code)]

use ocre::mail::{{Email, Preview}};

{MAILERS_MARKER}

/// Applied to every email of the mailers, like Rails' `default from:` and
/// `after_action`: e.g. `email.from("Shop <hello@yourdomain.com>")`,
/// `.bcc("archive@yourdomain.com")` or `.header("List-Unsubscribe", ...)`.
/// Without `from`, emails come from the MAIL_FROM variable.
pub fn defaults(email: Email) -> Email {{
    email
}}

/// Emails shown at /ocre/dev/mailers in `ocre dev` (debug builds only), built
/// with sample data: change the arguments to realistic values.
pub static PREVIEWS: &[Preview] = &[
    {PREVIEWS_MARKER}
];
"#
    )
}

/// Adds `pub mod <module>;` and the previews to src/mailers/mod.rs, creating
/// it (and `mod mailers;` plus the development pages in src/lib.rs) on first use.
fn register(edits: &mut Edits, module: &str, actions: &[String]) -> Result<(), CliError> {
    let missing_marker = |marker: &str| {
        CliError::new(format!("src/lib.rs is missing the `{marker}` marker"))
            .hint(format!("put `{marker}` on its own line where the generator should add code"))
    };
    let lib = edits.read("src/lib.rs")?.unwrap_or_default();
    let registry = match edits.read("src/mailers/mod.rs")? {
        Some(source) => source,
        None => {
            let lib = insert_after_marker(&lib, MODULES_MARKER, "mod mailers;").ok_or_else(|| {
                CliError::new("src/lib.rs is missing the `// ocre:modules` marker")
                    .hint("put `// ocre:modules` on its own line where `mod` declarations go")
            })?;
            edits.update("src/lib.rs", lib);
            registry_rs()
        }
    };
    let registry = insert_after_marker(&registry, MAILERS_MARKER, &format!("pub mod {module};"))
        .ok_or_else(|| CliError::new(format!("src/mailers/mod.rs is missing the `{MAILERS_MARKER}` marker")))?;
    // Registries that predate previews get none.
    let registry = if registry.contains(PREVIEWS_MARKER) {
        let previews: Vec<String> = actions
            .iter()
            .map(|action| format!("Preview::new(\"{module}/{action}\", || {module}::{action}(\"ada@example.com\")),"))
            .collect();
        let lib = edits.read("src/lib.rs")?.unwrap_or_default();
        if !lib.contains(DEV_ROUTES) {
            let lib = if lib.contains(DEV_ROUTES_EMPTY) {
                lib.replace(DEV_ROUTES_EMPTY, DEV_ROUTES)
            } else {
                insert_after_marker(&lib, ROUTES_MARKER, DEV_ROUTES).ok_or_else(|| missing_marker(ROUTES_MARKER))?
            };
            edits.update("src/lib.rs", lib);
        }
        insert_after_marker(&registry, PREVIEWS_MARKER, &previews.join("\n")).expect("marker checked above")
    } else {
        registry
    };
    edits.update("src/mailers/mod.rs", registry);
    Ok(())
}

//! `ocre generate auth`: users, sessions, password reset, magic links, JWT
//! and API keys, generated into the app (like Rails 8's authentication
//! generator) so every rule is visible and editable there.
//!
//! The files are static: they live in `templates/auth/` and are compiled by
//! the e2e test. API-only apps get the JSON part only.

use super::{Edits, MODULES_MARKER, insert_after_marker, model::register_model, next_migration_path, register_routes};
use crate::{CliResult, output::CliError, project::Project};

/// (module, source) of the models in both kinds of apps, under src/models/.
const MODELS: [(&str, &str); 2] = [
    ("user", include_str!("../../templates/auth/user.rs")),
    ("api_key", include_str!("../../templates/auth/api_key.rs")),
];
const AUTH_TOKEN_MODEL: &str = include_str!("../../templates/auth/auth_token.rs");

/// HTML route modules of full-stack apps.
const PAGES: [(&str, &str); 3] = [
    ("registrations", include_str!("../../templates/auth/registrations.rs")),
    ("sessions", include_str!("../../templates/auth/sessions.rs")),
    ("passwords", include_str!("../../templates/auth/passwords.rs")),
];

const TEMPLATES: [(&str, &str); 7] = [
    ("signup.html", include_str!("../../templates/auth/templates/signup.html")),
    ("login.html", include_str!("../../templates/auth/templates/login.html")),
    ("account.html", include_str!("../../templates/auth/templates/account.html")),
    ("magic_link_new.html", include_str!("../../templates/auth/templates/magic_link_new.html")),
    ("magic_link_show.html", include_str!("../../templates/auth/templates/magic_link_show.html")),
    ("password_new.html", include_str!("../../templates/auth/templates/password_new.html")),
    ("password_edit.html", include_str!("../../templates/auth/templates/password_edit.html")),
];

pub fn auth(project: &Project) -> CliResult {
    let mut edits = Edits::new(project);
    if edits.exists("src/models/user.rs") || edits.has_create_migration("users")? {
        return Err(CliError::new("this app already has a User model or a users table").hint(
            "`ocre g auth` creates both and runs once per app; to start over, remove src/models/user.rs and the create_users migration",
        ));
    }
    let full_stack = !project.api_only;

    let mut migrations = vec![("users", include_str!("../../templates/auth/create_users.sql"))];
    if full_stack {
        migrations.push(("auth_tokens", include_str!("../../templates/auth/create_auth_tokens.sql")));
    }
    migrations.push(("api_keys", include_str!("../../templates/auth/create_api_keys.sql")));
    for (table, sql) in migrations {
        let path = next_migration_path(&edits, &format!("create_{table}"))?;
        edits.create(&path, sql.to_owned())?;
    }

    let mut models = MODELS.to_vec();
    if full_stack {
        models.push(("auth_token", AUTH_TOKEN_MODEL));
    }
    for (module, source) in models {
        register_model(&mut edits, module)?;
        edits.create(&format!("src/models/{module}.rs"), source.to_owned())?;
    }

    edits.create("src/auth_api.rs", include_str!("../../templates/auth/auth_api.rs").to_owned())?;
    register_routes(&mut edits, "auth_api")?;
    if full_stack {
        edits.create("src/auth.rs", include_str!("../../templates/auth/auth.rs").to_owned())?;
        let lib = edits.read("src/lib.rs")?.unwrap_or_default();
        let lib = insert_after_marker(&lib, MODULES_MARKER, "mod auth;").expect("register_routes checked the marker");
        edits.update("src/lib.rs", lib);
        for (module, source) in PAGES {
            edits.create(&format!("src/{module}.rs"), source.to_owned())?;
            register_routes(&mut edits, module)?;
        }
        for (file, contents) in TEMPLATES {
            edits.create(&format!("templates/auth/{file}"), contents.to_owned())?;
        }
    }

    let mut report = edits.apply("generate auth")?;
    report.next = vec!["ocre migrate".to_owned(), "ocre dev".to_owned()];
    report.next.push(if full_stack {
        "open http://localhost:8787/signup".to_owned()
    } else {
        r#"curl -X POST http://localhost:8787/api/auth/signup -H 'content-type: application/json' -d '{"email":"ada@example.com","password":"correct horse"}'"#.to_owned()
    });
    Ok(report)
}

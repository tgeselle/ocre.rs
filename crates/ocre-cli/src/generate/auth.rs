//! `ocre generate auth`: users, sessions, password reset, magic links, email
//! confirmation, account deletion, rate limits, JWT and API keys, generated
//! into the app (like Rails 8's authentication generator) so every rule is
//! visible and editable there. `--db-sessions` tracks sessions in D1 (list
//! and revoke them); `--oauth github,google` adds "Continue with ..." sign-in.
//!
//! The files are static: they live in `templates/auth/` and are compiled by
//! the e2e test. API-only apps get the JSON part only.

use super::{
    Edits, MODULES_MARKER, insert_after_marker, model::register_model, next_migration_path, read_config,
    register_routes,
};
use crate::{
    CliResult,
    config::{self, ENV_MARKER},
    output::CliError,
    project::Project,
};

/// What `ocre g auth` generates beyond the default.
#[derive(Debug, Clone, Default)]
pub struct AuthOptions {
    /// `--db-sessions`: a `user_sessions` table and /account/sessions.
    pub db_sessions: bool,
    /// `--oauth github,google`: OAuth providers to offer on the login page.
    pub oauth: Vec<String>,
}

/// Providers `--oauth` accepts (`ocre::oauth::PROVIDERS`), with their secrets.
const OAUTH_PROVIDERS: [(&str, &str, &str); 2] =
    [("github", "GITHUB_CLIENT_ID", "GITHUB_CLIENT_SECRET"), ("google", "GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET")];

/// (module, source) of the models in both kinds of apps, under src/models/.
const MODELS: [(&str, &str); 2] = [
    ("user", include_str!("../../templates/auth/user.rs")),
    ("api_key", include_str!("../../templates/auth/api_key.rs")),
];
const AUTH_TOKEN_MODEL: &str = include_str!("../../templates/auth/auth_token.rs");
const USER_SESSION_MODEL: &str = include_str!("../../templates/auth/user_session.rs");
const IDENTITY_MODEL: &str = include_str!("../../templates/auth/identity.rs");

/// HTML route modules of full-stack apps.
const PAGES: [(&str, &str); 4] = [
    ("registrations", include_str!("../../templates/auth/registrations.rs")),
    ("sessions", include_str!("../../templates/auth/sessions.rs")),
    ("passwords", include_str!("../../templates/auth/passwords.rs")),
    ("confirmations", include_str!("../../templates/auth/confirmations.rs")),
];

const TEMPLATES: [(&str, &str); 8] = [
    ("signup.html", include_str!("../../templates/auth/templates/signup.html")),
    ("login.html", include_str!("../../templates/auth/templates/login.html")),
    ("account.html", include_str!("../../templates/auth/templates/account.html")),
    ("magic_link_new.html", include_str!("../../templates/auth/templates/magic_link_new.html")),
    ("magic_link_show.html", include_str!("../../templates/auth/templates/magic_link_show.html")),
    ("password_new.html", include_str!("../../templates/auth/templates/password_new.html")),
    ("password_edit.html", include_str!("../../templates/auth/templates/password_edit.html")),
    ("confirmation_show.html", include_str!("../../templates/auth/templates/confirmation_show.html")),
];

/// Placeholder line of account.html where `--db-sessions` links its page.
const ACCOUNT_LINKS: &str = "{# ocre:account-links #}";
/// The declaration of src/auth.rs that `--oauth` fills.
const NO_OAUTH_PROVIDERS: &str = "pub const OAUTH_PROVIDERS: &[&str] = &[];";

/// Name of the Workers Rate Limiting binding the generated code uses.
const RATE_LIMITER: &str = "AUTH_RATE_LIMITER";

pub fn auth(project: &Project, options: &AuthOptions) -> CliResult {
    let mut edits = Edits::new(project);
    if edits.exists("src/models/user.rs") || edits.has_create_migration("users")? {
        return Err(CliError::new("this app already has a User model or a users table").hint(
            "`ocre g auth` creates both and runs once per app; to start over, remove src/models/user.rs and the create_users migration",
        ));
    }
    let full_stack = !project.api_only;
    if !full_stack && (options.db_sessions || !options.oauth.is_empty()) {
        return Err(CliError::new("--db-sessions and --oauth need HTML pages, and this app is API-only").hint(
            "run `ocre g auth` without them: JSON clients use JWTs and API keys, which `DELETE /api/auth/keys/{id}` revokes",
        ));
    }
    let providers = oauth_providers(&options.oauth)?;

    let mut migrations = vec![("users", include_str!("../../templates/auth/create_users.sql"))];
    if full_stack {
        migrations.push(("auth_tokens", include_str!("../../templates/auth/create_auth_tokens.sql")));
    }
    migrations.push(("api_keys", include_str!("../../templates/auth/create_api_keys.sql")));
    if options.db_sessions {
        migrations.push(("user_sessions", include_str!("../../templates/auth/create_user_sessions.sql")));
    }
    if !providers.is_empty() {
        migrations.push(("identities", include_str!("../../templates/auth/create_identities.sql")));
    }
    for (table, sql) in migrations {
        let path = next_migration_path(&edits, &format!("create_{table}"))?;
        edits.create(&path, sql.to_owned())?;
    }

    let mut models = MODELS.to_vec();
    if full_stack {
        models.push(("auth_token", AUTH_TOKEN_MODEL));
    }
    if options.db_sessions {
        models.push(("user_session", USER_SESSION_MODEL));
    }
    if !providers.is_empty() {
        models.push(("identity", IDENTITY_MODEL));
    }
    for (module, source) in models {
        register_model(&mut edits, module)?;
        edits.create(&format!("src/models/{module}.rs"), source.to_owned())?;
    }

    edits.create("src/auth_api.rs", include_str!("../../templates/auth/auth_api.rs").to_owned())?;
    register_routes(&mut edits, "auth_api")?;
    if full_stack {
        let auth = if options.db_sessions {
            include_str!("../../templates/auth/auth_db_sessions.rs")
        } else {
            include_str!("../../templates/auth/auth.rs")
        };
        let names: Vec<String> = providers.iter().map(|(name, _, _)| format!("\"{name}\"")).collect();
        let auth =
            auth.replace(NO_OAUTH_PROVIDERS, &format!("pub const OAUTH_PROVIDERS: &[&str] = &[{}];", names.join(", ")));
        edits.create("src/auth.rs", auth)?;
        let lib = edits.read("src/lib.rs")?.unwrap_or_default();
        let lib = insert_after_marker(&lib, MODULES_MARKER, "mod auth;").expect("register_routes checked the marker");
        edits.update("src/lib.rs", lib);
        let mut pages = PAGES.to_vec();
        if options.db_sessions {
            pages.push(("user_sessions", include_str!("../../templates/auth/user_sessions.rs")));
        }
        if !providers.is_empty() {
            pages.push(("oauth", include_str!("../../templates/auth/oauth.rs")));
        }
        for (module, source) in pages {
            edits.create(&format!("src/{module}.rs"), source.to_owned())?;
            register_routes(&mut edits, module)?;
        }
        let mut templates = TEMPLATES.to_vec();
        if options.db_sessions {
            templates.push(("user_sessions.html", include_str!("../../templates/auth/templates/user_sessions.html")));
        }
        for (file, contents) in templates {
            let link =
                if options.db_sessions { "<p><a href=\"/account/sessions\">Signed-in devices</a></p>" } else { "" };
            let contents = contents.replace(
                &format!("{ACCOUNT_LINKS}\n"),
                &if link.is_empty() { String::new() } else { format!("{link}\n") },
            );
            edits.create(&format!("templates/auth/{file}"), contents)?;
        }
    }
    add_rate_limiter(&mut edits, &project.database_name)?;
    if !providers.is_empty() && edits.exists(".dev.vars") {
        let mut vars = edits.read(".dev.vars")?.unwrap_or_default();
        for (_, id, secret) in &providers {
            if !vars.contains(id) {
                vars.push_str(&format!("# From the OAuth app you registered (callback http://localhost:8787/auth/.../callback):\n# {id}=...\n# {secret}=...\n"));
            }
        }
        edits.update(".dev.vars", vars);
    }

    let mut report = edits.apply("generate auth")?;
    report.next = vec!["ocre migrate".to_owned(), "ocre dev".to_owned()];
    report.next.push(if full_stack {
        "open http://localhost:8787/signup".to_owned()
    } else {
        r#"curl -X POST http://localhost:8787/api/auth/signup -H 'content-type: application/json' -d '{"email":"ada@example.com","password":"correct horse"}'"#.to_owned()
    });
    for (name, id, secret) in &providers {
        report.next.push(format!(
            "register an OAuth app with {name} (callback https://<your host>/auth/{name}/callback), put {id} and {secret} in .dev.vars and their production values in .prod.vars, then `ocre secrets push {id} {secret} --file .prod.vars`"
        ));
    }
    Ok(report)
}

/// `["github", "google,github"]` -> the known providers, once each, in the order given.
fn oauth_providers(requested: &[String]) -> Result<Vec<(&'static str, &'static str, &'static str)>, CliError> {
    let mut providers = Vec::new();
    for name in requested.iter().flat_map(|names| names.split(',')).map(str::trim).filter(|name| !name.is_empty()) {
        let name = name.to_ascii_lowercase();
        let provider = OAUTH_PROVIDERS.into_iter().find(|(known, _, _)| *known == name).ok_or_else(|| {
            let known: Vec<&str> = OAUTH_PROVIDERS.iter().map(|(known, _, _)| *known).collect();
            CliError::new(format!("unknown OAuth provider `{name}`"))
                .hint(format!("--oauth accepts {} (comma-separated)", known.join(", ")))
        })?;
        if !providers.contains(&provider) {
            providers.push(provider);
        }
    }
    Ok(providers)
}

/// Adds the `AUTH_RATE_LIMITER` binding to cloudflare.config.ts unless it is there.
fn add_rate_limiter(edits: &mut Edits, app: &str) -> Result<(), CliError> {
    let config = read_config(edits)?;
    if config.binding(RATE_LIMITER).is_some() {
        return Ok(());
    }
    // Bindings with the same namespace share counters across the account's
    // Workers: derive one from the app name so apps do not collide.
    let namespace =
        1000 + app.bytes().fold(0u32, |hash, byte| hash.wrapping_mul(31).wrapping_add(u32::from(byte))) % 9_000_000;
    let entry = format!(
        "// `ocre g auth`: login, sign-up, token and emailed-link routes allow 10 attempts
// a minute per IP address and Cloudflare location (Workers Rate Limiting,
// free plan, no storage used). `period` is 10 or 60 seconds.
{RATE_LIMITER}: bindings.rateLimit({{ namespace: \"{namespace}\", simple: {{ limit: 10, period: 60 }} }}),"
    );
    edits.update(config::FILE, config.insert(ENV_MARKER, &entry)?);
    Ok(())
}

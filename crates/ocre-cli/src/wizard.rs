//! Guided `ocre new` for humans in a terminal. Every question is skipped when
//! the matching flag was given.

use std::path::Path;

use cliclack::{confirm, input, intro, note, outro, select, spinner};

use crate::{
    CliResult,
    new::{NewArgs, Plan, Starter, check_new_app},
    output::CliError,
    template::Template,
    wrangler::{Echo, Session, Wrangler, pick_account},
};

pub fn new_app(args: NewArgs, cwd: &Path) -> CliResult {
    ask(intro(" ocre "))?;

    let name = match args.name {
        Some(name) => name,
        None => {
            let cwd = cwd.to_owned();
            ask(input("What is your app called?")
                .placeholder("my-app")
                .validate(move |name: &String| check_new_app(&cwd, name).map_err(|err| err.to_string_with_hint()))
                .interact())?
        }
    };

    let api = match args.api {
        Some(api) => api,
        None => ask(select("What are you building?")
            .item(false, "Full-stack app", "HTML pages with askama and htmx, JSON APIs when you need them")
            .item(true, "API only", "JSON endpoints, no HTML (like `rails new --api`)")
            .interact())?,
    };

    let starter = match args.starter {
        Some(starter) => starter,
        None => ask(select("Pick a starter")
            .item(Starter::Empty, "Empty", if api { "a status endpoint" } else { "a home page" })
            .item(Starter::Blog, "Blog", "posts with title, body and published, full CRUD")
            .interact())?,
    };

    let wrangler = Wrangler::new(cwd, Echo::Capture);
    let session = cloudflare_session(&wrangler, args.login)?;
    let account_id = match &session {
        Some(session) if session.accounts.len() > 1 && args.account_id.is_none() => {
            let mut choice = select("Which Cloudflare account should host it?");
            for account in &session.accounts {
                choice = choice.item(account.id.clone(), &account.name, &account.id);
            }
            Some(ask(choice.interact())?)
        }
        Some(session) => pick_account(session, args.account_id.as_deref())?,
        None => args.account_id,
    };

    let git = match args.git {
        Some(git) => git,
        None => ask(confirm("Initialize a git repository?").initial_value(true).interact())?,
    };
    let deploy = match (&session, args.deploy) {
        (None, _) => false,
        (Some(_), Some(deploy)) => deploy,
        (Some(_), None) => {
            ask(confirm("Deploy it now? The first build takes about a minute.").initial_value(true).interact())?
        }
    };

    let mut plan = Plan::new(cwd, &name, args.ocre_path.as_deref(), api, starter, git, account_id)?;
    plan.template = args.template.map(|source| Template::load(&source, cwd)).transpose()?;
    let mut report = step("Creating your app", &format!("Created {name}/"), || plan.create())?;
    if deploy {
        let deployed = step("Building and deploying to Cloudflare", "Deployed", || plan.deploy(Echo::Capture))?;
        report.url = deployed.url;
        report.secret_created = deployed.secret_created;
        report.next.retain(|step| step != "ocre deploy");
    } else if session.is_none() {
        report.next.push("ocre login".to_owned());
    }

    ask(note("Next steps", report.next.join("\n")))?;
    let farewell = match &report.url {
        Some(url) => format!("Your app is live at {url}"),
        None => "Happy building!".to_owned(),
    };
    ask(outro(farewell))?;
    report.email = session.and_then(|s| s.email);
    report.rendered = true;
    Ok(report)
}

/// Current session, after offering (or, with `--login`, running) the login.
fn cloudflare_session(wrangler: &Wrangler, login: Option<bool>) -> Result<Option<Session>, CliError> {
    let session = step("Checking your Cloudflare login", "Checked your Cloudflare login", || wrangler.whoami())?;
    if let Some(session) = session {
        let who = session.email.as_deref().unwrap_or("an API token");
        ask(cliclack::log::success(format!("Logged in to Cloudflare as {who}")))?;
        return Ok(Some(session));
    }
    let login = match login {
        Some(login) => login,
        None => ask(select("Connect your Cloudflare account? It is free, and needed to deploy.")
            .item(true, "Log in now", "opens your browser")
            .item(false, "Later", "run `ocre login` when you are ready")
            .interact())?,
    };
    if !login {
        return Ok(None);
    }
    step("Approve access in your browser", "Logged in to Cloudflare", || wrangler.ensure_login()).map(Some)
}

/// Runs `work` behind a spinner.
fn step<T>(doing: &str, done: &str, work: impl FnOnce() -> Result<T, CliError>) -> Result<T, CliError> {
    let spinner = spinner();
    spinner.start(doing);
    let result = work();
    match &result {
        Ok(_) => spinner.stop(done),
        Err(err) => spinner.error(&err.message),
    }
    result
}

/// Esc or Ctrl-C in a prompt cancels the command.
fn ask<T>(result: std::io::Result<T>) -> Result<T, CliError> {
    result.map_err(prompt_error)
}

fn prompt_error(err: std::io::Error) -> CliError {
    match err.kind() {
        std::io::ErrorKind::Interrupted => CliError::new("cancelled")
            .hint("run `ocre new <name>` with flags (see `ocre new --help`) to skip the questions"),
        _ => err.into(),
    }
}

#[cfg(test)]
#[path = "../tests/wizard.rs"]
mod tests;

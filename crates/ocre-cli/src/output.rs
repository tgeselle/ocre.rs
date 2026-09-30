use std::process::ExitCode;

use serde::Serialize;

/// Successful command result. Paths are relative to the app root.
#[derive(Serialize, Default)]
pub struct Report {
    pub command: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub updated: Vec<String>,
    /// Existing files a generator kept (`--skip`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    /// Files `ocre destroy` deleted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// `--pretend`: the files listed were not written.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pretend: bool,
    /// `ocre g override`: the generator templates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub templates: Option<Vec<crate::generate::TemplateInfo>>,
    /// `ocre version` / `ocre about`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<crate::about::About>,
    /// `ocre doctor`: one entry per check.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<crate::doctor::Check>,
    /// `ocre stats`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<crate::stats::Stats>,
    /// `ocre notes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<crate::notes::Note>>,
    /// `ocre db version`: the last applied migration (`null` for none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<Option<String>>,
    /// The command ran but failed (a doctor check, a test step): reported
    /// with `ok: false`, `error` and `hint`, exit code 1.
    #[serde(skip)]
    pub failure: Option<CliError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Cloudflare login email, for commands that check the session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Migrations not yet applied (`ocre migrate --status`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
    /// Steps a database command performed, in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ran: Vec<String>,
    /// `ocre sql`: the D1 query result, one object with `results` per statement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<serde_json::Value>,
    /// `ocre routes`: the app's routes, sorted by path then method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routes: Option<Vec<crate::routes::Route>>,
    /// `ocre schedules`: the Cron Triggers and their tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedules: Option<Vec<crate::schedules::Schedule>>,
    /// The command targeted the production database on Cloudflare.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub remote: bool,
    /// `ocre secrets list`: secret names, local and deployed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Vec<crate::secrets::SecretStatus>>,
    /// `ocre secret`: a new random value for SECRET_KEY_BASE.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// `ocre time-zones`: IANA time zone names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_zones: Option<Vec<&'static str>>,
    /// `ocre domains`: the Worker's custom domains.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domains: Option<Vec<String>>,
    /// The deploy uploaded a new SECRET_KEY_BASE (the Worker had none).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub secret_created: bool,
    /// `ocre deploy`: Cloudflare resources it created because they were
    /// missing, e.g. `queue shop-jobs`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub provisioned: Vec<String>,
    /// Commands to run next, in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// Already shown to the user (interactive wizard); skip the human summary.
    #[serde(skip)]
    pub rendered: bool,
}

impl Report {
    pub fn new(command: &'static str) -> Self {
        Self { command, ..Self::default() }
    }
}

/// Failure with an optional hint that names the fix.
#[derive(Debug)]
pub struct CliError {
    pub message: String,
    pub hint: Option<String>,
}

impl CliError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), hint: None }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// One-line form for inline prompts: `message (hint)`.
    pub fn to_string_with_hint(&self) -> String {
        match &self.hint {
            Some(hint) => format!("{} ({hint})", self.message),
            None => self.message.clone(),
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(err: std::io::Error) -> Self {
        Self::new(err.to_string())
    }
}

pub fn finish(result: Result<Report, CliError>, json: bool) -> ExitCode {
    match result {
        Ok(mut report) => {
            let failure = report.failure.take();
            if json {
                let mut value = serde_json::to_value(&report).expect("report serializes");
                value["ok"] = failure.is_none().into();
                if let Some(err) = &failure {
                    value["error"] = err.message.clone().into();
                    value["hint"] = err.hint.clone().into();
                }
                println!("{value}");
            } else {
                if !report.rendered {
                    print_human(&report);
                }
                if let Some(err) = &failure {
                    print_error(err);
                }
            }
            if failure.is_some() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
        }
        Err(err) => {
            if json {
                let value = serde_json::json!({ "ok": false, "error": err.message, "hint": err.hint });
                println!("{value}");
            } else {
                print_error(&err);
            }
            ExitCode::FAILURE
        }
    }
}

fn print_error(err: &CliError) {
    eprintln!("error: {}", err.message);
    if let Some(hint) = &err.hint {
        eprintln!("hint: {hint}");
    }
}

fn print_human(report: &Report) {
    if let Some(about) = &report.about {
        for (name, value) in about.lines() {
            println!("{name:<20}{value}");
        }
    }
    for check in &report.checks {
        let status = match check.status {
            crate::doctor::Status::Ok => "ok  ",
            crate::doctor::Status::Warn => "warn",
            crate::doctor::Status::Fail => "FAIL",
        };
        println!("  {status}  {:<20}{}", check.name, check.detail);
        if let Some(hint) = check.hint.as_ref().filter(|_| check.status != crate::doctor::Status::Ok) {
            println!("        {:<20}{hint}", "");
        }
    }
    if let Some(stats) = &report.stats {
        print!("{}", crate::stats::table(stats));
    }
    for note in report.notes.iter().flatten() {
        println!("{}:{}: [{}] {}", note.path, note.line, note.tag, note.text);
    }
    if let Some(secret) = &report.secret {
        println!("{secret}");
    }
    if let Some(domains) = &report.domains {
        if domains.is_empty() {
            println!("  no custom domain: the Worker answers on workers.dev");
        }
        for domain in domains {
            println!("  {domain}");
        }
    }
    for zone in report.time_zones.iter().flatten() {
        println!("{zone}");
    }
    for secret in report.secrets.iter().flatten() {
        let local = if secret.local { ".dev.vars" } else { "" };
        let deployed = if secret.deployed { "deployed" } else { "" };
        println!("  {:<32}{local:<12}{deployed}", secret.name);
    }
    if let Some(version) = &report.version {
        println!("{}", version.as_deref().unwrap_or("no migration applied"));
    }
    for path in &report.created {
        println!("  create  {path}");
    }
    for path in &report.updated {
        println!("  update  {path}");
    }
    for path in &report.removed {
        println!("  remove  {path}");
    }
    for path in &report.skipped {
        println!("  skip    {path}");
    }
    if report.pretend {
        println!("(--pretend: nothing was written)");
    }
    if let Some(email) = &report.email {
        println!("Logged in to Cloudflare as {email}");
    }
    for step in &report.ran {
        println!("  {step}");
    }
    for name in &report.pending {
        println!("  pending  {name}");
    }
    if let Some(routes) = &report.routes {
        print!("{}", crate::routes::table(routes));
    }
    if let Some(schedules) = &report.schedules {
        print!("{}", crate::schedules::table(schedules));
    }
    for template in report.templates.iter().flatten() {
        println!("  {}{}", template.path, if template.overridden { "  (overridden in .ocre/templates/)" } else { "" });
    }
    if report.remote {
        println!("Target: remote D1 database on Cloudflare");
    }
    if report.secret_created {
        println!("Created the SECRET_KEY_BASE secret on Cloudflare");
    }
    for resource in &report.provisioned {
        println!("Created {resource} on Cloudflare");
    }
    if let Some(url) = &report.url {
        println!("\n{url}");
    }
    if !report.next.is_empty() {
        println!("\nNext:");
        for step in &report.next {
            println!("  {step}");
        }
    }
}

#[cfg(test)]
#[path = "../tests/output.rs"]
mod tests;

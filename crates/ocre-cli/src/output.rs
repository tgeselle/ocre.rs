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
    /// `ocre sql`: wrangler's JSON, one object with `results` per statement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<serde_json::Value>,
    /// `ocre routes`: the app's routes, sorted by path then method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routes: Option<Vec<crate::routes::Route>>,
    /// The command targeted the production database on Cloudflare.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub remote: bool,
    /// `ocre secret`: a new random value for SECRET_KEY_BASE.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// The deploy uploaded a new SECRET_KEY_BASE (the Worker had none).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub secret_created: bool,
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
        Ok(report) => {
            if json {
                let mut value = serde_json::to_value(&report).expect("report serializes");
                value["ok"] = true.into();
                println!("{value}");
            } else if !report.rendered {
                print_human(&report);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if json {
                let value = serde_json::json!({ "ok": false, "error": err.message, "hint": err.hint });
                println!("{value}");
            } else {
                eprintln!("error: {}", err.message);
                if let Some(hint) = &err.hint {
                    eprintln!("hint: {hint}");
                }
            }
            ExitCode::FAILURE
        }
    }
}

fn print_human(report: &Report) {
    if let Some(secret) = &report.secret {
        println!("{secret}");
    }
    for path in &report.created {
        println!("  create  {path}");
    }
    for path in &report.updated {
        println!("  update  {path}");
    }
    if let Some(email) = &report.email {
        println!("Logged in to Cloudflare as {email}");
    }
    for step in &report.ran {
        println!("  {step}");
    }
    if let Some(routes) = &report.routes {
        print!("{}", crate::routes::table(routes));
    }
    if report.remote {
        println!("Target: remote D1 database on Cloudflare");
    }
    if report.secret_created {
        println!("Created the SECRET_KEY_BASE secret on Cloudflare");
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

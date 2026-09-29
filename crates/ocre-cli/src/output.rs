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
    for path in &report.created {
        println!("  create  {path}");
    }
    for path in &report.updated {
        println!("  update  {path}");
    }
    if let Some(email) = &report.email {
        println!("Logged in to Cloudflare as {email}");
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
mod tests {
    use super::*;

    #[test]
    fn one_line_errors_append_the_hint_when_there_is_one() {
        assert_eq!(CliError::new("bad").to_string_with_hint(), "bad");
        assert_eq!(CliError::new("bad").hint("fix it").to_string_with_hint(), "bad (fix it)");
    }
}

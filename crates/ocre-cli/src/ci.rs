//! `ocre ci`: the app's continuous-integration steps on the developer's
//! machine, like Rails 8.1's `bin/ci`. The same steps, in the same order,
//! run on GitHub Actions in the workflow `ocre g ci` writes:
//!
//! 1. `cargo fmt --check`,
//! 2. `cargo clippy --all-targets -- -D warnings`,
//! 3. `cargo test`,
//! 4. `cargo check --target wasm32-unknown-unknown` (the real build),
//! 5. `ocre i18n missing`, when src/lib.rs declares locales.
//!
//! It stops at the first failing step. With `--signoff`, a green run ends
//! with `gh signoff` (the basecamp/gh-signoff extension of the GitHub CLI),
//! which marks the pushed commit as passing CI.

use std::process::Command;

use crate::{
    CliResult,
    output::{CliError, Report},
    project::{Project, check_wasm_target},
};

/// The cargo steps: the command line as shown (and written in the GitHub
/// workflow), and the arguments passed to `cargo`.
pub const STEPS: [(&str, &[&str]); 4] = [
    ("cargo fmt --check", &["fmt", "--check"]),
    ("cargo clippy --all-targets -- -D warnings", &["clippy", "--all-targets", "--", "-D", "warnings"]),
    ("cargo test", &["test"]),
    ("cargo check --target wasm32-unknown-unknown", &["check", "--target", "wasm32-unknown-unknown"]),
];

/// The translations step, run when src/lib.rs declares locales.
pub const I18N_STEP: &str = "ocre i18n missing";

pub fn run(project: &Project, signoff: bool, json: bool) -> CliResult {
    let mut report = Report::new("ci");
    let mut failure = None;
    for (name, args) in STEPS {
        if args[0] == "check"
            && let Err(err) = check_wasm_target()
        {
            failure = Some(err);
            break;
        }
        let status = Command::new("cargo").args(args).current_dir(&project.root).stdout(output(json)).status();
        match status {
            Ok(status) if status.success() => report.ran.push(format!("{name}: ok")),
            Ok(status) => {
                failure = Some(CliError::new(format!("{name} failed ({status})")).hint(fix(name)));
                break;
            }
            Err(err) => {
                return Err(CliError::new(format!("could not run cargo: {err}"))
                    .hint("install Rust with rustup: https://rustup.rs"));
            }
        }
    }
    if failure.is_none() && has_locales(project) {
        match crate::i18n::missing(project) {
            Ok(_) => report.ran.push(format!("{I18N_STEP}: ok")),
            Err(err) => failure = Some(err),
        }
    }
    if failure.is_none() && signoff {
        match gh_signoff(project, json) {
            Ok(()) => report.ran.push("gh signoff: ok".to_owned()),
            Err(err) => failure = Some(err),
        }
    }
    report.failure = failure;
    Ok(report)
}

/// Whether src/lib.rs declares locales (`ocre g locale`).
pub fn has_locales(project: &Project) -> bool {
    std::fs::read_to_string(project.root.join("src/lib.rs"))
        .is_ok_and(|lib| crate::i18n::declared(&lib).is_some_and(|codes| !codes.is_empty()))
}

/// What to do when a step fails.
fn fix(step: &str) -> &'static str {
    match step {
        "cargo fmt --check" => "run `cargo fmt`, then `ocre ci` again",
        "cargo clippy --all-targets -- -D warnings" => {
            "fix the warnings above (`rustup component add clippy` if cargo has no clippy command)"
        }
        _ => "the cargo output above names the failure",
    }
}

fn gh_signoff(project: &Project, json: bool) -> Result<(), CliError> {
    let status =
        Command::new("gh").arg("signoff").current_dir(&project.root).stdout(output(json)).status().map_err(|_| {
            CliError::new("gh not found").hint(
                "install the GitHub CLI (https://cli.github.com), then `gh extension install basecamp/gh-signoff`",
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::new(format!("gh signoff failed ({status})")).hint(
            "install the extension with `gh extension install basecamp/gh-signoff`, push the branch, and log in with `gh auth login`",
        ))
    }
}

/// Where tool output goes: the terminal, or stderr with `--json`.
fn output(json: bool) -> std::process::Stdio {
    if json { std::process::Stdio::from(std::io::stderr()) } else { std::process::Stdio::inherit() }
}

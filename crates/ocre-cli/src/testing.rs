//! `ocre test`: the app's checks in one command, like `bin/rails test:all`.
//!
//! 1. `cargo test` (native unit tests; extra arguments are passed on),
//! 2. `cargo check --target wasm32-unknown-unknown` (the real build),
//! 3. with `--e2e`: local migrations, one `wrangler dev` for the whole run,
//!    then `tests/e2e.sh` with `BASE_URL` pointing at it; the server stops
//!    when the script ends.
//!
//! It stops at the first failing step.

use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use crate::{
    CliResult,
    output::{CliError, Report},
    project::{Project, check_wasm_target},
    wrangler::{Echo, Wrangler},
};

/// End-to-end script, relative to the app root.
pub const E2E_SCRIPT: &str = "tests/e2e.sh";

/// How long the first `wrangler dev` build may take.
const READY_TIMEOUT: Duration = Duration::from_secs(600);

pub fn run(project: &Project, e2e: bool, port: u16, cargo_args: &[String], json: bool) -> CliResult {
    let mut report = Report::new("test");
    let steps: [(&str, Vec<String>); 2] = [
        ("cargo test", ["test"].iter().map(|s| (*s).to_owned()).chain(cargo_args.iter().cloned()).collect()),
        (
            "cargo check --target wasm32-unknown-unknown",
            vec!["check".to_owned(), "--target".to_owned(), "wasm32-unknown-unknown".to_owned()],
        ),
    ];
    if e2e && !project.root.join(E2E_SCRIPT).is_file() {
        return Err(CliError::new(format!("{E2E_SCRIPT} not found")).hint(format!(
            "create {E2E_SCRIPT}: a shell script sending requests to $BASE_URL (e.g. `curl -fsS \"$BASE_URL/up\"`) that exits non-zero on failure"
        )));
    }
    for (name, args) in steps {
        if args[0] == "check"
            && let Err(err) = check_wasm_target()
        {
            report.failure = Some(err);
            return Ok(report);
        }
        let status = Command::new("cargo").args(&args).current_dir(&project.root).stdout(output(json)).status();
        match status {
            Ok(status) if status.success() => report.ran.push(format!("{name}: ok")),
            Ok(status) => {
                report.failure = Some(
                    CliError::new(format!("{name} failed ({status})")).hint("the cargo output above names the failure"),
                );
                return Ok(report);
            }
            Err(err) => {
                return Err(CliError::new(format!("could not run cargo: {err}"))
                    .hint("install Rust with rustup: https://rustup.rs"));
            }
        }
    }
    if e2e {
        match end_to_end(project, port, json) {
            Ok(()) => report.ran.push(format!("{E2E_SCRIPT} against wrangler dev on port {port}: ok")),
            Err(err) => report.failure = Some(err),
        }
    }
    Ok(report)
}

/// Where tool output goes: the terminal, or stderr with `--json`.
fn output(json: bool) -> Stdio {
    if json { Stdio::from(std::io::stderr()) } else { Stdio::inherit() }
}

fn end_to_end(project: &Project, port: u16, json: bool) -> Result<(), CliError> {
    let wrangler = Wrangler::new(&project.root, Echo::for_json(json)).dev_build();
    wrangler.migrate(&project.database_name, false)?;
    let mut server = wrangler.spawn(&["dev", "--port", &port.to_string()])?;
    let stdout = server.stdout.take().expect("stdout is piped");
    let (ready, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            eprintln!("{line}");
            if line.contains("Ready on") {
                let _ = ready.send(());
            }
        }
    });
    let result = match ready_rx.recv_timeout(READY_TIMEOUT) {
        Ok(()) => {
            let status = Command::new("sh")
                .arg(E2E_SCRIPT)
                .current_dir(&project.root)
                .env("BASE_URL", format!("http://localhost:{port}"))
                .stdout(output(json))
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(CliError::new(format!("{E2E_SCRIPT} failed ({status})"))
                    .hint("its output above shows the failing request"))
            }
        }
        Err(_) => Err(CliError::new("wrangler dev stopped or did not get ready")
            .hint("run `ocre dev` to see the build or startup error")),
    };
    crate::wrangler::stop(server);
    result
}

//! `ocre test`: the app's checks in one command, like `bin/rails test:all`.
//!
//! 1. `cargo test` (native unit tests; extra arguments are passed on),
//! 2. `cargo check --target wasm32-unknown-unknown` (the real build),
//! 3. with `--e2e`, against the real runtime:
//!    - a fresh local database in `.wrangler/test-state` (the development
//!      state in `.wrangler/state` is untouched): migrations, then the
//!      fixtures of `tests/fixtures/`,
//!    - one server for the whole run (the app's wrangler, as `cf dev` runs
//!      it, on the test state), its output in `.wrangler/test-state/dev.log`,
//!    - `cargo test -- --ignored` (the request tests, `#[ignore]`d in plain
//!      `cargo test`) with `OCRE_TEST_URL`, `OCRE_TEST_STATE` and
//!      `OCRE_TEST_LOG` set for `ocre::testing`,
//!    - `tests/e2e.sh` with `BASE_URL`, when the app has one,
//!    - the Playwright browser tests of `tests/system/` (`ocre g system_test`), when there are some.
//!
//!    The server stops when they end.
//!
//! It stops at the first failing step.

use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use crate::{
    CliResult,
    cloudflare::{Echo, LocalD1, Sql},
    fixtures,
    generate::system_test::SYSTEM_TESTS,
    output::{CliError, Report},
    project::{Project, check_wasm_target},
};

/// End-to-end script, relative to the app root.
pub const E2E_SCRIPT: &str = "tests/e2e.sh";

/// Fixtures loaded into the test database, relative to the app root (Rails' `test/fixtures`).
pub const TEST_FIXTURES: &str = "tests/fixtures";

/// How long the first build of the test server may take.
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
    for (name, args) in steps {
        if args[0] == "check"
            && let Err(err) = check_wasm_target()
        {
            report.failure = Some(err);
            return Ok(report);
        }
        if let Err(err) = cargo(project, name, &args, &[], json)? {
            report.failure = Some(err);
            return Ok(report);
        }
        report.ran.push(format!("{name}: ok"));
    }
    if e2e && let Err(err) = end_to_end(project, port, cargo_args, json, &mut report.ran) {
        report.failure = Some(err);
    }
    Ok(report)
}

/// Runs `cargo <args>`: `Err` when cargo cannot start, `Ok(Err)` when the step fails.
fn cargo(
    project: &Project,
    name: &str,
    args: &[String],
    env: &[(&str, String)],
    json: bool,
) -> Result<Result<(), CliError>, CliError> {
    let status = Command::new("cargo")
        .args(args)
        .envs(env.iter().map(|(key, value)| (*key, value)))
        .current_dir(&project.root)
        .stdout(output(json))
        .status()
        .map_err(|err| {
            CliError::new(format!("could not run cargo: {err}")).hint("install Rust with rustup: https://rustup.rs")
        })?;
    Ok(if status.success() {
        Ok(())
    } else {
        Err(CliError::new(format!("{name} failed ({status})"))
            .hint("the cargo output above names the failing test; rerun it alone with `ocre test -- <test name>`"))
    })
}

/// Where tool output goes: the terminal, or stderr with `--json`.
fn output(json: bool) -> Stdio {
    if json { Stdio::from(std::io::stderr()) } else { Stdio::inherit() }
}

/// `cargo test <args> -- --ignored --test-threads=1`: only the tests that
/// need the server, one at a time: they share one database (a test's rows
/// would change another's counts) and SQLite takes one writer at a time.
fn ignored_tests(cargo_args: &[String]) -> Vec<String> {
    let mut args = vec!["test".to_owned()];
    args.extend(cargo_args.iter().cloned());
    if !cargo_args.iter().any(|arg| arg == "--") {
        args.push("--".to_owned());
    }
    args.push("--ignored".to_owned());
    if !cargo_args.iter().any(|arg| arg.starts_with("--test-threads")) {
        args.push("--test-threads=1".to_owned());
    }
    args
}

/// A fresh test database: migrations, then the fixtures.
fn prepare_database(project: &Project, echo: Echo) -> Result<(), CliError> {
    let state = project.root.join(LocalD1::TEST_STATE);
    if state.exists() {
        std::fs::remove_dir_all(&state)?;
    }
    std::fs::create_dir_all(&state)?;
    let database = LocalD1::new(project, echo).for_tests();
    database.migrate()?;
    let sql = fixtures::to_sql(&project.root, TEST_FIXTURES)?;
    if !sql.is_empty() {
        let file = format!("{}/fixtures.sql", LocalD1::TEST_STATE);
        std::fs::write(project.root.join(&file), sql)?;
        database.execute(Sql::File(&file))?;
    }
    Ok(())
}

fn end_to_end(
    project: &Project,
    port: u16,
    cargo_args: &[String],
    json: bool,
    ran: &mut Vec<String>,
) -> Result<(), CliError> {
    let echo = Echo::for_json(json);
    prepare_database(project, echo)?;
    ran.push(format!("test database {}: migrated, {TEST_FIXTURES} loaded", LocalD1::TEST_STATE));
    let log_path = project.root.join(LocalD1::TEST_STATE).join("dev.log");
    let log = Arc::new(Mutex::new(File::create(&log_path)?));
    let mut server = LocalD1::new(project, echo).for_tests().spawn_server(port)?;
    let (ready, ready_rx) = mpsc::channel();
    let stdout = server.stdout.take().expect("stdout is piped");
    let stderr = server.stderr.take().expect("stderr is piped");
    for pipe in [Box::new(stdout) as Box<dyn Read + Send>, Box::new(stderr)] {
        let (log, ready) = (Arc::clone(&log), ready.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                eprintln!("{line}");
                let _ = writeln!(log.lock().expect("no panic while writing"), "{line}");
                if line.contains("Ready on") {
                    let _ = ready.send(());
                }
            }
        });
    }
    // Only the readers hold senders now: a server that exits ends the wait.
    drop(ready);
    let result = match ready_rx.recv_timeout(READY_TIMEOUT) {
        Ok(()) => run_against(project, port, cargo_args, &log_path, json, ran),
        Err(_) => Err(CliError::new("the test server stopped or did not get ready")
            .hint(format!("its output is in {}/dev.log; `ocre dev` shows the same build", LocalD1::TEST_STATE))),
    };
    crate::cloudflare::stop(server);
    result
}

/// The request tests, then `tests/e2e.sh`, against the server on `port`.
fn run_against(
    project: &Project,
    port: u16,
    cargo_args: &[String],
    log_path: &Path,
    json: bool,
    ran: &mut Vec<String>,
) -> Result<(), CliError> {
    let base = format!("http://localhost:{port}");
    let env = [
        ("OCRE_TEST_URL", base.clone()),
        ("OCRE_TEST_STATE", LocalD1::TEST_STATE.to_owned()),
        ("OCRE_TEST_LOG", log_path.display().to_string()),
    ];
    let name = "cargo test -- --ignored";
    cargo(project, name, &ignored_tests(cargo_args), &env, json)??;
    ran.push(format!("{name} against the test server on port {port}: ok"));
    if project.root.join(E2E_SCRIPT).is_file() {
        e2e_script(project, &base, json)?;
        ran.push(format!("{E2E_SCRIPT} against the test server on port {port}: ok"));
    }
    if has_system_tests(&project.root) {
        system_tests(project, &base, json)?;
        ran.push(format!("playwright test ({SYSTEM_TESTS}) against the test server on port {port}: ok"));
    }
    Ok(())
}

/// Whether `tests/system/` holds a Playwright test.
fn has_system_tests(root: &Path) -> bool {
    std::fs::read_dir(root.join(SYSTEM_TESTS)).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| entry.file_name().to_string_lossy().ends_with(".spec.ts"))
    })
}

/// The browser tests of `tests/system/` (`ocre g system_test`), with Playwright.
fn system_tests(project: &Project, base: &str, json: bool) -> Result<(), CliError> {
    let playwright = project.root.join("node_modules/.bin/playwright");
    if !playwright.exists() {
        return Err(CliError::new(format!("{SYSTEM_TESTS} has tests but Playwright is not installed"))
            .hint("run `npm install` (package.json lists @playwright/test), then `npx playwright install chromium`"));
    }
    let status = Command::new(playwright)
        .arg("test")
        .current_dir(&project.root)
        .env("BASE_URL", base)
        .stdout(output(json))
        .status()?;
    if !status.success() {
        return Err(CliError::new(format!("playwright test failed ({status})")).hint(
            "its output above names the failing test; test-results/ has a screenshot and a trace of each (`npx playwright show-trace <file>`)",
        ));
    }
    Ok(())
}

fn e2e_script(project: &Project, base: &str, json: bool) -> Result<(), CliError> {
    let status = Command::new("sh")
        .arg(E2E_SCRIPT)
        .current_dir(&project.root)
        .env("BASE_URL", base)
        .stdout(output(json))
        .status()?;
    if !status.success() {
        return Err(
            CliError::new(format!("{E2E_SCRIPT} failed ({status})")).hint("its output above shows the failing request")
        );
    }
    Ok(())
}

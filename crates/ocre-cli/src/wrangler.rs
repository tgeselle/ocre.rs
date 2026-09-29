//! Commands that drive wrangler: migrate, dev, deploy.

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

use crate::{
    CliResult,
    output::{CliError, Report},
    project::{Project, check_wasm_target},
};

/// Pinned major version, so generated apps and the CLI agree on flags.
const WRANGLER: &str = "wrangler@4";

pub fn migrate(remote: bool, json: bool) -> CliResult {
    let project = Project::find()?;
    apply_migrations(&project, remote, json)?;
    Ok(Report::new("migrate"))
}

/// Runs until stopped. The app is served at `http://localhost:<port>`.
pub fn dev(port: u16, json: bool) -> CliResult {
    let project = Project::find()?;
    check_wasm_target()?;
    apply_migrations(&project, false, json)?;
    let port_arg = port.to_string();
    stream(&project, &["dev", "--port", &port_arg], json)?;
    Ok(Report { url: Some(format!("http://localhost:{port}")), ..Report::new("dev") })
}

/// Deploys, then migrates. An existing database is migrated before the new
/// code goes live; a new one is created by the first deploy, then migrated.
pub fn deploy(json: bool) -> CliResult {
    let project = Project::find()?;
    check_wasm_target()?;
    let output = if database_exists(&project)? {
        apply_migrations(&project, true, json)?;
        stream(&project, &["deploy"], json)?
    } else {
        let output = stream(&project, &["deploy"], json)?;
        apply_migrations(&project, true, json)?;
        output
    };
    let url = output
        .split_whitespace()
        .find(|word| word.starts_with("https://") && word.contains(".workers.dev"))
        .map(str::to_owned);
    Ok(Report { url, ..Report::new("deploy") })
}

fn apply_migrations(project: &Project, remote: bool, json: bool) -> Result<(), CliError> {
    let target = if remote { "--remote" } else { "--local" };
    stream(project, &["d1", "migrations", "apply", &project.database_name, target], json).map(drop)
}

fn database_exists(project: &Project) -> Result<bool, CliError> {
    let output = wrangler(project)
        .args(["d1", "list", "--json"])
        .stderr(Stdio::piped())
        .output()
        .map_err(npx_missing)?;
    if !output.status.success() {
        return Err(CliError::new(format!(
            "`wrangler d1 list` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .hint("log in with `npx wrangler login`, or set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID"));
    }
    let databases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
        .map_err(|err| CliError::new(format!("unexpected `wrangler d1 list --json` output: {err}")))?;
    Ok(databases.iter().any(|db| db["name"] == project.database_name.as_str()))
}

/// Runs wrangler, echoing its stdout (to stderr in JSON mode) and returning it.
fn stream(project: &Project, args: &[&str], json: bool) -> Result<String, CliError> {
    let mut child = wrangler(project).args(args).stdout(Stdio::piped()).spawn().map_err(npx_missing)?;
    let mut captured = String::new();
    let reader = BufReader::new(child.stdout.take().expect("stdout is piped"));
    for line in reader.lines() {
        let line = line?;
        if json {
            writeln!(std::io::stderr(), "{line}")?;
        } else {
            writeln!(std::io::stdout(), "{line}")?;
        }
        captured.push_str(&line);
        captured.push('\n');
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(CliError::new(format!("`wrangler {}` failed ({status})", args.join(" ")))
            .hint("read the wrangler output above; the first error line names the cause"));
    }
    Ok(captured)
}

fn wrangler(project: &Project) -> Command {
    let mut command = Command::new("npx");
    command.args(["--yes", WRANGLER]).current_dir(&project.root);
    command
}

fn npx_missing(err: std::io::Error) -> CliError {
    CliError::new(format!("could not run npx: {err}")).hint("install Node.js 20 or newer (it provides npx)")
}

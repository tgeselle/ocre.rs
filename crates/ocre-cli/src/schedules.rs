//! `ocre schedules`: the app's Cron Triggers and the task each one runs,
//! read from wrangler.toml and src/schedules/mod.rs; `ocre schedules run
//! <task>` fires one on the running `ocre dev`, like Loco's
//! `cargo loco scheduler --name`.

use std::{
    io::{Read, Write},
    net::TcpStream,
    time::Duration,
};

use serde::Serialize;

use crate::{
    CliResult,
    output::{CliError, Report},
    project::Project,
};

#[derive(Serialize, Debug, PartialEq)]
pub struct Schedule {
    /// The expression in `[triggers] crons`, UTC.
    pub cron: String,
    /// The task module in src/schedules/, `None` when no task handles the cron.
    pub task: Option<String>,
}

/// Lists every cron of wrangler.toml with the task that handles it.
pub fn list(project: &Project) -> CliResult {
    let schedules = read(project)?;
    let mut report = Report::new("schedules");
    if schedules.iter().any(|s| s.task.is_none()) {
        report.next.push(
            "a cron without a task fails when it fires: add it to the match in src/schedules/mod.rs, or remove it \
             from [triggers] crons in wrangler.toml"
                .to_owned(),
        );
    }
    report.schedules = Some(schedules);
    Ok(report)
}

/// Fires the cron of `task` on `ocre dev` (wrangler's local scheduled endpoint).
pub fn run(project: &Project, task: &str, port: u16) -> CliResult {
    let schedules = read(project)?;
    let schedule = schedules.iter().find(|s| s.task.as_deref() == Some(task)).ok_or_else(|| {
        let known: Vec<&str> = schedules.iter().filter_map(|s| s.task.as_deref()).collect();
        CliError::new(format!("no scheduled task `{task}`")).hint(if known.is_empty() {
            "create one with `ocre g schedule <name> \"every day at 3am\"`".to_owned()
        } else {
            format!("tasks: {}", known.join(", "))
        })
    })?;
    let query = schedule.cron.replace(' ', "+").replace('#', "%23");
    let path = format!("/cdn-cgi/local/scheduled?cron={query}");
    let status = get(port, &path).map_err(|err| {
        CliError::new(format!("cannot reach `ocre dev` on port {port}: {err}"))
            .hint("start the app with `ocre dev` in another terminal (or pass --port), then run this again")
    })?;
    if status != 200 {
        return Err(CliError::new(format!("the scheduled task answered HTTP {status}"))
            .hint("read the `[ocre cron]` line in the `ocre dev` output"));
    }
    let mut report = Report::new("schedules run");
    report.ran.push(format!("fired {} ({}); its `[ocre cron]` line is in the `ocre dev` output", task, schedule.cron));
    Ok(report)
}

/// Human form: an aligned `CRON  TASK` table.
pub fn table(schedules: &[Schedule]) -> String {
    if schedules.is_empty() {
        return "No schedules. Add one with `ocre g schedule <name> \"every day at 3am\"`.\n".to_owned();
    }
    let width = schedules.iter().map(|s| s.cron.len()).max().unwrap_or(0).max("CRON (UTC)".len());
    let mut out = format!("{:<width$}  TASK\n", "CRON (UTC)");
    for schedule in schedules {
        let task = match &schedule.task {
            Some(task) => format!("src/schedules/{task}.rs"),
            None => "(no task: fails when it fires)".to_owned(),
        };
        out.push_str(&format!("{:<width$}  {task}\n", schedule.cron));
    }
    out
}

/// Crons from wrangler.toml, matched with the `"<cron>" => <task>::run(...)`
/// arms of src/schedules/mod.rs.
fn read(project: &Project) -> Result<Vec<Schedule>, CliError> {
    let wrangler = std::fs::read_to_string(project.root.join("wrangler.toml"))?;
    let config: toml::Table = wrangler.parse().expect("Project::find parsed it");
    let crons: Vec<String> = config
        .get("triggers")
        .and_then(|triggers| triggers.get("crons"))
        .and_then(|crons| crons.as_array())
        .into_iter()
        .flatten()
        .filter_map(|cron| cron.as_str().map(str::to_owned))
        .collect();
    let registry = std::fs::read_to_string(project.root.join("src/schedules/mod.rs")).unwrap_or_default();
    let arms: Vec<(String, String)> = registry
        .lines()
        .filter_map(|line| {
            let (cron, rest) = line.trim().strip_prefix('"')?.split_once("\" => ")?;
            let task = rest.split_once("::run(")?.0;
            Some((cron.to_owned(), task.to_owned()))
        })
        .collect();
    let task = |cron: &str| arms.iter().find(|(c, _)| c == cron).map(|(_, task)| task.clone());
    Ok(crons.iter().map(|cron| Schedule { cron: cron.clone(), task: task(cron) }).collect())
}

/// `GET <path>` on `ocre dev`; returns the HTTP status once the status line
/// arrived (the task has run by then).
fn get(port: u16, path: &str) -> std::io::Result<u16> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes())?;
    let mut response = Vec::new();
    let mut chunk = [0u8; 256];
    while !response.contains(&b'\n') {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8_lossy(&response)
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| std::io::Error::other("not an HTTP answer"))
}

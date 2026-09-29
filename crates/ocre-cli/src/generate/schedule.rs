//! `ocre generate schedule`: `src/schedules/<name>.rs`, a task run by a
//! Cloudflare Cron Trigger, dispatched by cron expression in
//! `src/schedules/mod.rs`, with the expression added to `[triggers] crons` in
//! wrangler.toml. The first schedule wires the Worker's `scheduled` event in
//! src/lib.rs.

use super::{Edits, MODULES_MARKER, fields::RESERVED, insert_after_marker};
use crate::{
    CliResult,
    names::{humanize, is_identifier, split_words},
    output::CliError,
    project::Project,
};

const MODS_MARKER: &str = "// ocre:schedules";
const DISPATCH_MARKER: &str = "// ocre:schedule-dispatch";
/// Local endpoint of `wrangler dev` that fires the `scheduled` event.
const LOCAL_ENDPOINT: &str = "http://localhost:8787/cdn-cgi/local/scheduled";
/// Cron Triggers per account on the Workers Free plan.
const FREE_CRONS: usize = 5;

const REGISTRY: &str = r#"//! Scheduled tasks (Cloudflare Cron Triggers). `ocre g schedule` adds them
//! below and their cron expression to `[triggers] crons` in wrangler.toml.
//! Times are UTC. Free plan: 5 Cron Triggers per account, 10 ms of CPU per run.

use ocre::{Ctx, Error, Result};

// ocre:schedules

/// Runs the task for `cron`, the expression from wrangler.toml that fired.
/// Called by `ocre::jobs::cron` from the `scheduled` event in src/lib.rs.
pub async fn run(ctx: Ctx, cron: String) -> Result<()> {
    match cron.as_str() {
        // ocre:schedule-dispatch
        _ => Err(Error::internal(format!(
            "no scheduled task for cron `{cron}`. Fix: add it to the match in src/schedules/mod.rs, or remove it \
             from [triggers] crons in wrangler.toml"
        ))),
    }
}
"#;

const SCHEDULED_EVENT: &str = r#"
/// Cron Triggers (`[triggers] crons` in wrangler.toml), run by `schedules::run` (src/schedules/mod.rs).
#[worker::event(scheduled)]
async fn scheduled(event: worker::ScheduledEvent, env: worker::Env, _ctx: worker::ScheduleContext) {
    ocre::jobs::cron(event, env, schedules::run).await
}
"#;

pub fn schedule(project: &Project, name: &str, schedule: &str) -> CliResult {
    let module = split_words(name).join("_");
    if !is_identifier(&module) || RESERVED.contains(&module.as_str()) {
        return Err(CliError::new(format!("invalid schedule name `{name}`"))
            .hint("use snake_case starting with a letter, not a Rust keyword, e.g. `nightly_cleanup`"));
    }
    let cron = parse_cron(schedule)?;
    let mut edits = Edits::new(project);
    let path = format!("src/schedules/{module}.rs");
    let phrase = schedule.split_whitespace().collect::<Vec<_>>().join(" ");
    edits.create(&path, task_rs(&module, &cron, name, &phrase))?;

    let wrangler = edits.read("wrangler.toml")?.unwrap_or_default();
    let crons = with_cron(&wrangler, &cron)?;
    edits.update("wrangler.toml", crons.text);

    let registry = match edits.read("src/schedules/mod.rs")? {
        Some(registry) => registry,
        None => {
            let lib = edits.read("src/lib.rs")?.unwrap_or_default();
            if lib.contains("event(scheduled)") {
                return Err(CliError::new("src/lib.rs already handles the `scheduled` event").hint(
                    "a Worker has one scheduled entry point: call `ocre::jobs::cron(event, env, schedules::run)` \
                     from it and create src/schedules/mod.rs by hand",
                ));
            }
            let lib = insert_after_marker(&lib, MODULES_MARKER, "mod schedules;").ok_or_else(|| {
                CliError::new("src/lib.rs is missing the `// ocre:modules` marker")
                    .hint("put `// ocre:modules` on its own line where `mod` declarations go")
            })?;
            edits.update("src/lib.rs", lib + SCHEDULED_EVENT);
            REGISTRY.to_owned()
        }
    };
    let missing = |marker: &str| {
        CliError::new(format!("src/schedules/mod.rs is missing the `{marker}` marker"))
            .hint(format!("keep `{MODS_MARKER}` above the modules and `{DISPATCH_MARKER}` inside the `match` of `run`"))
    };
    let registry = insert_after_marker(&registry, MODS_MARKER, &format!("pub mod {module};"))
        .ok_or_else(|| missing(MODS_MARKER))?;
    let registry =
        insert_after_marker(&registry, DISPATCH_MARKER, &format!("\"{cron}\" => {module}::run(&ctx).await,"))
            .ok_or_else(|| missing(DISPATCH_MARKER))?;
    edits.update("src/schedules/mod.rs", registry);

    let mut report = edits.apply("generate schedule")?;
    report.next = vec![
        format!("ocre dev, then: ocre schedules run {module}"),
        format!("ocre deploy (Cron Triggers only fire on the deployed Worker; this one runs at `{cron}`, UTC)"),
    ];
    if crons.count > FREE_CRONS {
        report.next.push(format!(
            "this app now has {} crons; the free plan allows {FREE_CRONS} per account: run several tasks from one cron",
            crons.count
        ));
    }
    Ok(report)
}

/// A schedule as Cloudflare's five cron fields (minute, hour, day of month,
/// month, day of week), from a cron expression or a plain-English phrase
/// such as "every 15 minutes" or "every monday at 9am".
pub(crate) fn parse_cron(schedule: &str) -> Result<String, CliError> {
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    let cron_like = fields.len() == 5
        && fields.iter().all(|field| field.chars().all(|c| c.is_ascii_alphanumeric() || "*,-/#".contains(c)))
        && fields[0].chars().any(|c| c.is_ascii_digit() || c == '*');
    if cron_like {
        return Ok(fields.join(" "));
    }
    english(schedule).ok_or_else(|| {
        CliError::new(format!("invalid schedule `{schedule}`")).hint(
            "quote a phrase, in UTC: \"every 15 minutes\", \"every hour\", \"every 6 hours\", \"every day at 3am\", \
             \"every monday at 9:30\", \"every weekday at 18:00\", \"midnight on tuesdays\", \"every month\"; or five \
             cron fields: minute hour day-of-month month day-of-week, e.g. \"0 3 * * *\" or \"*/15 * * * *\". Cron \
             Triggers run at most once a minute",
        )
    })
}

/// Plain English to cron, like Loco's scheduler: "every 5 minutes" is `*/5 * * * *`.
/// `None` when the phrase is not understood. Cron Triggers run at most once
/// a minute, so seconds are refused.
fn english(phrase: &str) -> Option<String> {
    let phrase = phrase.trim().to_lowercase().replace(',', " ");
    let (frequency, time) = match phrase.split_once(" at ") {
        Some((frequency, time)) => (frequency.to_owned(), Some(clock(time)?)),
        None if phrase.starts_with("at ") => (String::new(), Some(clock(&phrase[3..])?)),
        None => (phrase.clone(), None),
    };
    let mut words: Vec<&str> =
        frequency.split_whitespace().filter(|word| !["every", "each", "on", "and", "the"].contains(word)).collect();
    // "midnight on tuesdays", "noon every day": the time comes first.
    let mut time = time;
    if let Some(position) = words.iter().position(|word| ["midnight", "noon"].contains(word)) {
        if time.is_some() {
            return None;
        }
        time = Some(if words[position] == "noon" { (12, 0) } else { (0, 0) });
        words.remove(position);
    }
    let (hour, minute) = time.unwrap_or((0, 0));
    let at = |day_of_month: &str, day_of_week: &str| format!("{minute} {hour} {day_of_month} * {day_of_week}");
    let every = |n: &str| n.parse::<u32>().ok();
    match words.as_slice() {
        [] if time.is_some() => Some(at("*", "*")),
        ["day" | "days" | "daily"] => Some(at("*", "*")),
        ["minute"] if time.is_none() => Some("* * * * *".to_owned()),
        [n, "minute" | "minutes" | "mins"] if time.is_none() => match every(n)? {
            1 => Some("* * * * *".to_owned()),
            n @ 2..=59 => Some(format!("*/{n} * * * *")),
            _ => None,
        },
        ["hour"] | ["hourly"] if time.is_none() => Some("0 * * * *".to_owned()),
        [n, "hour" | "hours"] if time.is_none() => match every(n)? {
            1 => Some("0 * * * *".to_owned()),
            n @ 2..=23 => Some(format!("0 */{n} * * *")),
            _ => None,
        },
        ["week"] | ["weekly"] => Some(at("*", "SUN")),
        ["month"] | ["monthly"] => Some(at("1", "*")),
        ["weekday" | "weekdays"] => Some(at("*", "MON-FRI")),
        ["weekend" | "weekends"] => Some(at("*", "SAT,SUN")),
        [] => None,
        days => {
            let days: Option<Vec<&str>> = days.iter().map(|day| weekday(day)).collect();
            Some(at("*", &days?.join(",")))
        }
    }
}

/// `monday`, `mondays`, `mon` -> `MON`.
fn weekday(word: &str) -> Option<&'static str> {
    const DAYS: [(&str, &str); 7] = [
        ("monday", "MON"),
        ("tuesday", "TUE"),
        ("wednesday", "WED"),
        ("thursday", "THU"),
        ("friday", "FRI"),
        ("saturday", "SAT"),
        ("sunday", "SUN"),
    ];
    let word = word.strip_suffix('s').filter(|w| w.ends_with("day")).unwrap_or(word);
    DAYS.iter().find(|(name, short)| *name == word || short.eq_ignore_ascii_case(word)).map(|(_, short)| *short)
}

/// `4pm`, `4:30 pm`, `16:30`, `9`, `midnight`, `noon` -> (hour, minute), 24-hour.
fn clock(text: &str) -> Option<(u32, u32)> {
    let text = text.trim().replace(' ', "");
    match text.as_str() {
        "midnight" => return Some((0, 0)),
        "noon" => return Some((12, 0)),
        _ => {}
    }
    let (digits, meridiem) = match text.strip_suffix("am").or_else(|| text.strip_suffix("a.m.")) {
        Some(rest) => (rest, Some(false)),
        None => match text.strip_suffix("pm").or_else(|| text.strip_suffix("p.m.")) {
            Some(rest) => (rest, Some(true)),
            None => (text.as_str(), None),
        },
    };
    let (hour, minute) = digits.split_once(':').unwrap_or((digits, "0"));
    let (hour, minute): (u32, u32) = (hour.parse().ok()?, minute.parse().ok()?);
    if minute > 59 {
        return None;
    }
    let hour = match meridiem {
        None if hour <= 23 => hour,
        Some(pm) if (1..=12).contains(&hour) => hour % 12 + if pm { 12 } else { 0 },
        _ => return None,
    };
    Some((hour, minute))
}

/// `0 3 * * *` -> `0+3+*+*+*`, for the local endpoint's query string.
fn query_value(cron: &str) -> String {
    cron.replace(' ', "+").replace('#', "%23")
}

fn task_rs(module: &str, cron: &str, name: &str, schedule: &str) -> String {
    let when = if schedule == cron { format!("`{cron}`") } else { format!("`{cron}` ({schedule})") };
    format!(
        r#"//! {}: scheduled task. Generated by `ocre g schedule {name} "{schedule}"`.
//!
//! Runs at {when}, UTC, on the deployed Worker, from `[triggers] crons` in
//! wrangler.toml. Run it now while `ocre dev` runs: `ocre schedules run {module}`
//! (or `curl '{LOCAL_ENDPOINT}?cron={}'`).

use ocre::{{Ctx, Result}};

/// Does the work. Free plan: 10 ms of CPU per run, so keep it to a few
/// queries and enqueue jobs (`ocre::jobs::enqueue_all`) for anything longer
/// or per-row. An `Err` is logged (`[ocre cron]`); the task runs again at
/// the next scheduled time.
pub async fn run(_ctx: &Ctx) -> Result<()> {{
    Ok(())
}}
"#,
        humanize(module),
        query_value(cron),
    )
}

struct Crons {
    /// wrangler.toml with the cron added.
    text: String,
    /// Crons in `[triggers]` afterwards.
    count: usize,
}

/// Adds `cron` to `[triggers] crons`, creating the table or the key.
fn with_cron(text: &str, cron: &str) -> Result<Crons, CliError> {
    let config: toml::Table = text.parse().expect("Project::find parsed wrangler.toml");
    let unsupported = || {
        CliError::new("wrangler.toml defines `triggers` in a form Ocre cannot edit")
            .hint("write it as a table: `[triggers]` on its own line, then `crons = [\"0 3 * * *\"]`")
    };
    let mut crons: Vec<String> = match config.get("triggers") {
        None => Vec::new(),
        Some(triggers) => match triggers.get("crons") {
            None => Vec::new(),
            Some(value) => value
                .as_array()
                .and_then(|items| items.iter().map(|item| item.as_str().map(str::to_owned)).collect())
                .ok_or_else(unsupported)?,
        },
    };
    if crons.iter().any(|existing| existing == cron) {
        return Err(CliError::new(format!("cron `{cron}` is already in [triggers] crons")).hint(
            "one task per cron: call the new work from the existing task in src/schedules/, or pick another time \
             (e.g. one minute later)",
        ));
    }
    crons.push(cron.to_owned());
    let line = format!("crons = [{}]", crons.iter().map(|cron| format!("\"{cron}\"")).collect::<Vec<_>>().join(", "));
    let lines: Vec<&str> = text.lines().collect();
    let header = lines.iter().position(|l| l.trim() == "[triggers]");
    let text = match (config.get("triggers"), header) {
        (None, _) => format!(
            "{text}\n# Cron Triggers (`ocre g schedule`), in UTC, run by src/schedules/. Free plan: 5 per account.\n\
             [triggers]\n{line}\n"
        ),
        (Some(_), None) => return Err(unsupported()),
        (Some(_), Some(header)) => {
            let end = lines[header + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with('['))
                .map_or(lines.len(), |i| i + header + 1);
            let mut out: Vec<String> = lines[..=header].iter().map(|l| (*l).to_owned()).collect();
            let key =
                lines[header + 1..end].iter().position(|l| l.trim_start().starts_with("crons")).map(|i| i + header + 1);
            match key {
                None => {
                    out.push(line);
                    out.extend(lines[header + 1..].iter().map(|l| (*l).to_owned()));
                }
                Some(key) => {
                    // The array may span lines; it ends at the first `]` (cron expressions have none).
                    let close = lines[key..]
                        .iter()
                        .position(|l| l.contains(']'))
                        .map(|i| i + key)
                        .expect("valid TOML closes the array");
                    out.extend(lines[header + 1..key].iter().map(|l| (*l).to_owned()));
                    out.push(line);
                    out.extend(lines[close + 1..].iter().map(|l| (*l).to_owned()));
                }
            }
            out.join("\n") + "\n"
        }
    };
    Ok(Crons { text, count: crons.len() })
}

#[cfg(test)]
#[path = "../../tests/generate/schedule.rs"]
mod tests;

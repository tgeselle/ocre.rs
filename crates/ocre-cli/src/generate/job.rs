//! `ocre generate job`: `src/jobs/<name>.rs`, a struct holding the job's
//! arguments with a `perform` method, added to the `Job` enum and the
//! `perform` dispatch match in `src/jobs/mod.rs`. The first job also wires
//! the `JOBS` queue in wrangler.toml and the Worker's `queue` event in
//! src/lib.rs.

use std::fmt::Write as _;

use super::{
    Edits, MODULES_MARKER,
    fields::{RESERVED, parse_fields},
    insert_after_marker,
    mailer::pascal,
};
use crate::{
    CliResult,
    names::{humanize, is_identifier, split_words},
    output::CliError,
    project::Project,
};

const MODS_MARKER: &str = "// ocre:jobs";
const VARIANTS_MARKER: &str = "// ocre:job-variants";
const DISPATCH_MARKER: &str = "// ocre:job-dispatch";

const REGISTRY: &str = r#"//! Background jobs, run by Cloudflare Queues. `ocre g job` adds them below.
//!
//! Enqueue one from a handler; it runs moments later in the `queue` event of
//! src/lib.rs, which hands each message to `perform`:
//! `SendWelcome { user_id }.perform_later(&ctx).await?`, or
//! `ocre::jobs::enqueue_in(&ctx, &Job::SendWelcome(..), Duration::from_secs(600))` to delay it
//! (24 hours at most), `ocre::jobs::enqueue_all(&ctx, &jobs)` for many at once.
//! Run one now, in the request, with `job.perform(&ctx).await?`.

// A job is often generated before a handler enqueues it.
#![allow(dead_code)]

use ocre::{Ctx, Result};
use serde::{Deserialize, Serialize};

// ocre:jobs

/// Every job of the app. A queue message holds one as JSON:
/// `{"send_welcome": {"user_id": 1}}`. Renaming a variant or changing its
/// fields makes messages already queued undecodable (they are logged and
/// dropped), so change jobs when the queue is empty or add a new variant.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Job {
    // ocre:job-variants
}

/// Runs one job; called by `ocre::jobs::consume` for each queue message.
/// `Ok` acknowledges it; `Err(Error::NotFound)` and other 4xx errors drop
/// it (logged); other errors retry it later. Code here runs around every
/// job, like Rails' `around_perform`.
pub async fn perform(ctx: Ctx, job: Job) -> Result<()> {
    match job {
        // ocre:job-dispatch
    }
}
"#;

const QUEUE_EVENT: &str = r#"
/// Background jobs from the `JOBS` queue, run by `jobs::perform` (src/jobs/mod.rs).
#[worker::event(queue)]
async fn queue(batch: worker::MessageBatch<String>, env: worker::Env, _ctx: worker::Context) -> worker::Result<()> {
    ocre::jobs::consume(batch, env, jobs::perform).await
}
"#;

pub fn job(project: &Project, name: &str, specs: &[String], queue: Option<&str>) -> CliResult {
    let module = module_name(name)?;
    let pascal = pascal(&module);
    let fields = parse_fields(specs)?;
    let queue = queue_name(queue.unwrap_or(DEFAULT_QUEUE))?;
    if let Some(field) = fields.iter().find(|field| field.is_attachment()) {
        return Err(CliError::new(format!("job field `{}` cannot be an attachment", field.name)).hint(
            "files do not fit in a queue message (128 KB): store the file first and pass its record id, e.g. \
             `post_id:integer`",
        ));
    }
    if let Some(field) = fields.iter().find(|field| field.unique) {
        return Err(CliError::new(format!("job field `{}` cannot be unique", field.name))
            .hint("`^` adds a unique index to a table column; jobs have no table: drop the `^`"));
    }
    let command = std::iter::once(format!("ocre g job {name}"))
        .chain(specs.iter().cloned())
        .chain((queue != DEFAULT_QUEUE).then(|| format!("--queue {queue}")))
        .collect::<Vec<_>>()
        .join(" ");
    let mut struct_fields = String::new();
    for field in &fields {
        writeln!(struct_fields, "    pub {}: {},", field.name, field.column_type()).expect("writing to a String");
    }
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    let literal =
        if names.is_empty() { format!("{pascal} {{}}") } else { format!("{pascal} {{ {} }}", names.join(", ")) };

    let mut edits = Edits::new(project);
    edits.create(
        &format!("src/jobs/{module}.rs"),
        job_rs(&module, &pascal, &struct_fields, &literal, &command, &queue),
    )?;
    let mut created = Vec::new();
    if edits.read("src/jobs/mod.rs")?.is_none() {
        created.push(wire(&mut edits)?);
    }
    if queue != DEFAULT_QUEUE {
        created.extend(wire_queue(&mut edits, &queue)?);
    }
    register(&mut edits, &module, &pascal)?;
    let mut report = edits.apply("generate job")?;
    report.next = vec![
        format!("enqueue it from a handler: jobs::{literal}.perform_later(&ctx).await?"),
        "ocre dev (jobs run locally; look for `[ocre jobs]` lines in the output)".to_owned(),
    ];
    for queue in created {
        report.next.push(format!("ocre deploy creates the queue {queue} and its dead-letter queue"));
    }
    Ok(report)
}

/// The queue `ocre::jobs::enqueue` uses.
const DEFAULT_QUEUE: &str = "default";

/// A queue name as `ocre::jobs::queue` accepts it: lowercase letters, digits and `-`.
fn queue_name(queue: &str) -> Result<String, CliError> {
    let valid = !queue.is_empty()
        && queue.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !queue.starts_with('-')
        && !queue.ends_with('-');
    if !valid {
        return Err(CliError::new(format!("invalid queue name `{queue}`"))
            .hint("use lowercase letters, digits and `-`, e.g. `--queue urgent`"));
    }
    Ok(queue.to_owned())
}

/// `urgent` -> `JOBS_URGENT`.
fn queue_binding(queue: &str) -> String {
    format!("JOBS_{}", queue.to_ascii_uppercase().replace('-', "_"))
}

/// `SendWelcomeJob`, `send_welcome` or `SendWelcome` -> `send_welcome`.
fn module_name(name: &str) -> Result<String, CliError> {
    let mut words = split_words(name);
    if words.len() > 1 && words.last().is_some_and(|w| w == "job") {
        words.pop();
    }
    let module = words.join("_");
    if !is_identifier(&module) || RESERVED.contains(&module.as_str()) || module == "job" {
        return Err(CliError::new(format!("invalid job name `{name}`"))
            .hint("use a verb phrase starting with a letter, not a Rust keyword, e.g. `SendWelcome` or `ImportCsv`"));
    }
    Ok(module)
}

fn job_rs(module: &str, pascal: &str, fields: &str, literal: &str, command: &str, queue: &str) -> String {
    let body = if fields.is_empty() { " {}\n".to_owned() } else { format!(" {{\n{fields}}}\n") };
    let (sender, on) = if queue == DEFAULT_QUEUE {
        ("ocre::jobs::enqueue(ctx, &super::Job::".to_owned(), "the `default` queue".to_owned())
    } else {
        (format!("ocre::jobs::queue(ctx, \"{queue}\").enqueue(&super::Job::"), format!("the `{queue}` queue"))
    };
    format!(
        r#"//! {} job. Generated by `{command}`.
//!
//! Enqueue it from a handler; it runs moments later, outside the request:
//! `{literal}.perform_later(&ctx).await?` (with `use crate::jobs::{pascal};`).

use ocre::{{Ctx, Result}};
use serde::{{Deserialize, Serialize}};

/// The job's arguments, stored in the queue message as JSON (128 KB at
/// most): pass ids and small values, and load records in `perform`.
#[derive(Debug, Serialize, Deserialize)]
pub struct {pascal}{body}
impl {pascal} {{
    /// Sends the job to {on}; it runs moments later through `perform`.
    pub fn perform_later(self, ctx: &Ctx) -> impl Future<Output = Result<()>> + Send + use<> {{
        {sender}{pascal}(self))
    }}

    /// Does the work, e.g. `ctx.db()?` queries or
    /// `ocre::mail::send(ctx, crate::mailers::user::welcome(&email)?).await?`.
    /// `Err(Error::NotFound)` (a record deleted since) and other 4xx errors
    /// drop the job; any other `Err` retries it later (30 s, 1 min, 3 min,
    /// 9 min, 27 min), then moves it to the dead-letter queue. A job may run
    /// more than once: make it safe to repeat. Free plan: 10 ms of CPU per
    /// batch of messages.
    pub async fn perform(self, _ctx: &Ctx) -> Result<()> {{
        Ok(())
    }}
}}
"#,
        humanize(module),
    )
}

/// First job: the registry, `mod jobs;` and the `queue` event in src/lib.rs,
/// and the queues in wrangler.toml unless a `JOBS` producer exists. Returns
/// the queue's name.
fn wire(edits: &mut Edits) -> Result<String, CliError> {
    let lib = edits.read("src/lib.rs")?.unwrap_or_default();
    if lib.contains("event(queue)") {
        return Err(CliError::new("src/lib.rs already handles the `queue` event").hint(
            "a Worker has one queue entry point: call `ocre::jobs::consume(batch, env, jobs::perform)` from it and \
             create src/jobs/mod.rs by hand",
        ));
    }
    let lib = insert_after_marker(&lib, MODULES_MARKER, "mod jobs;").ok_or_else(|| {
        CliError::new("src/lib.rs is missing the `// ocre:modules` marker")
            .hint("put `// ocre:modules` on its own line where `mod` declarations go")
    })?;
    edits.update("src/lib.rs", lib + QUEUE_EVENT);
    edits.update("src/jobs/mod.rs", REGISTRY.to_owned());
    let text = edits.read("wrangler.toml")?.unwrap_or_default();
    let config: toml::Table = text.parse().expect("Project::find parsed wrangler.toml");
    let producer = config
        .get("queues")
        .and_then(|queues| queues.get("producers"))
        .and_then(|producers| producers.as_array())
        .into_iter()
        .flatten()
        .find(|p| p.get("binding").and_then(|b| b.as_str()) == Some("JOBS"));
    if let Some(producer) = producer {
        return producer.get("queue").and_then(|q| q.as_str()).map(str::to_owned).ok_or_else(|| {
            CliError::new("the JOBS queue producer in wrangler.toml has no `queue`")
                .hint("add `queue = \"<app-name>-jobs\"` to the [[queues.producers]] entry with binding = \"JOBS\"")
        });
    }
    let app = config.get("name").and_then(|name| name.as_str()).ok_or_else(|| {
        CliError::new("wrangler.toml has no `name`").hint("add `name = \"<app-name>\"` at the top of wrangler.toml")
    })?;
    let queue = format!("{app}-jobs");
    edits.update("wrangler.toml", text + &queues_toml(app));
    Ok(queue)
}

/// A named queue: its producer `JOBS_<NAME>` and consumer, unless wrangler.toml
/// already binds it. Returns the queue created, if any.
fn wire_queue(edits: &mut Edits, queue: &str) -> Result<Option<String>, CliError> {
    let binding = queue_binding(queue);
    let text = edits.read("wrangler.toml")?.unwrap_or_default();
    if text.contains(&format!("binding = \"{binding}\"")) {
        return Ok(None);
    }
    let config: toml::Table = text.parse().expect("Project::find parsed wrangler.toml");
    let app = config.get("name").and_then(|name| name.as_str()).ok_or_else(|| {
        CliError::new("wrangler.toml has no `name`").hint("add `name = \"<app-name>\"` at the top of wrangler.toml")
    })?;
    let name = format!("{app}-jobs-{queue}");
    edits.update(
        "wrangler.toml",
        text + &format!(
            r#"
# The `{queue}` job queue (`ocre::jobs::queue(&ctx, "{queue}")`): its own
# consumer, so its jobs never wait behind the `default` queue. Same Queues
# costs per message; batches wait at most 1 s.
[[queues.producers]]
binding = "{binding}"
queue = "{name}"

[[queues.consumers]]
queue = "{name}"
max_batch_size = 10
max_batch_timeout = 1
max_retries = 5
dead_letter_queue = "{name}-failed"
"#
        ),
    );
    Ok(Some(name))
}

fn queues_toml(app: &str) -> String {
    format!(
        r#"
# Background jobs (`ocre g job`): the Worker sends jobs to this queue and runs
# them (src/jobs/). `ocre deploy` creates both queues. Free plan: 10,000
# Queues operations a day (a job costs 3: write, read, delete; a retry one more
# read), messages kept 24 hours, 128 KB each.
[[queues.producers]]
binding = "JOBS"
queue = "{app}-jobs"

# Up to 10 messages per run, waiting at most 5 s to fill a batch. Each run is
# one Worker request with 10 ms of CPU on the free plan: lower max_batch_size
# for CPU-heavy jobs. A failing job is retried with a growing delay (30 s,
# 1 min, 3 min, 9 min, 27 min), then moved to {app}-jobs-failed, where it
# stays 24 hours (dashboard: Queues > {app}-jobs-failed).
[[queues.consumers]]
queue = "{app}-jobs"
max_batch_size = 10
max_batch_timeout = 5
max_retries = 5
dead_letter_queue = "{app}-jobs-failed"
"#
    )
}

/// Adds the module, the enum variant and the dispatch arm to src/jobs/mod.rs.
fn register(edits: &mut Edits, module: &str, pascal: &str) -> Result<(), CliError> {
    let missing = |marker: &str| {
        CliError::new(format!("src/jobs/mod.rs is missing the `{marker}` marker")).hint(format!(
            "keep `{MODS_MARKER}` above the modules, `{VARIANTS_MARKER}` inside `enum Job` and \
             `{DISPATCH_MARKER}` inside the `match` of `perform`"
        ))
    };
    let registry = edits.read("src/jobs/mod.rs")?.expect("created or present");
    let registry =
        insert_after_marker(&registry, MODS_MARKER, &format!("pub mod {module};\npub use {module}::{pascal};"))
            .ok_or_else(|| missing(MODS_MARKER))?;
    let registry = insert_after_marker(&registry, VARIANTS_MARKER, &format!("{pascal}({pascal}),"))
        .ok_or_else(|| missing(VARIANTS_MARKER))?;
    let registry =
        insert_after_marker(&registry, DISPATCH_MARKER, &format!("Job::{pascal}(job) => job.perform(&ctx).await,"))
            .ok_or_else(|| missing(DISPATCH_MARKER))?;
    edits.update("src/jobs/mod.rs", registry);
    Ok(())
}

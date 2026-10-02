//! `ocre generate job`: `src/jobs/<name>.rs`, a struct holding the job's
//! arguments with a `perform` method, added to the `Job` enum and the
//! `perform` dispatch match in `src/jobs/mod.rs`. The first job also wires
//! the `JOBS` queue in cloudflare.config.ts and the Worker's `queue` event in
//! src/lib.rs.

use std::fmt::Write as _;

use super::{
    Edits, MODULES_MARKER, ROUTES_MARKER,
    fields::{RESERVED, parse_fields},
    insert_after_marker,
    mailer::pascal,
    read_config,
};
use crate::{
    CliResult,
    config::{self, Config, ENV_MARKER, TRIGGERS_MARKER},
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

/// Steps and lock of `ocre g job --steps a,b --lock field`.
#[derive(Clone, Debug, Default)]
pub struct JobSteps {
    /// Step names, run one queue message each, in order.
    pub steps: Vec<String>,
    /// A field: one run per value at a time.
    pub lock: Option<String>,
}

pub fn job(project: &Project, name: &str, specs: &[String], queue: Option<&str>, stepped: &JobSteps) -> CliResult {
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
    let steps = step_names(stepped, &fields)?;
    let command = std::iter::once(format!("ocre g job {name}"))
        .chain(specs.iter().cloned())
        .chain((queue != DEFAULT_QUEUE).then(|| format!("--queue {queue}")))
        .chain((!stepped.steps.is_empty()).then(|| format!("--steps {}", stepped.steps.join(","))))
        .chain(stepped.lock.as_ref().map(|field| format!("--lock {field}")))
        .collect::<Vec<_>>()
        .join(" ");
    let mut struct_fields = String::new();
    for field in &fields {
        writeln!(struct_fields, "    pub {}: {},", field.name, field.column_type()).expect("writing to a String");
    }
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    let literal = if !steps.is_empty() {
        format!("{pascal}::new({})", names.join(", "))
    } else if names.is_empty() {
        format!("{pascal} {{}}")
    } else {
        format!("{pascal} {{ {} }}", names.join(", "))
    };

    let mut edits = Edits::new(project);
    let source = if steps.is_empty() {
        job_rs(&module, &pascal, &struct_fields, &literal, &command, &queue)
    } else {
        let typed: Vec<(String, String)> =
            fields.iter().map(|field| (field.name.clone(), field.column_type().to_string())).collect();
        stepped_job_rs(&module, &pascal, &typed, &steps, stepped.lock.as_deref(), &command, &queue)
    };
    edits.create(&format!("src/jobs/{module}.rs"), source)?;
    let locks_table = stepped.lock.is_some() && !edits.has_create_migration("job_locks")?;
    if locks_table {
        let path = super::next_migration_path(&edits, "create_job_locks")?;
        let sql = format!(
            "-- Generated by `{command}`: locks of jobs that run one at a time per key.\n{}",
            ocre::jobs::LOCKS_TABLE_SQL
        );
        edits.create(&path, sql)?;
    }
    let mut created = Vec::new();
    if edits.read("src/jobs/mod.rs")?.is_none() {
        created.push(wire(&mut edits)?);
    }
    if queue != DEFAULT_QUEUE {
        created.extend(wire_queue(&mut edits, &queue)?);
    }
    register(&mut edits, &module, &pascal)?;
    let mut report = edits.apply("generate job")?;
    report.next = vec![format!("enqueue it from a handler: jobs::{literal}.perform_later(&ctx).await?")];
    if locks_table {
        report.next.insert(0, "ocre migrate".to_owned());
    }
    report.next.extend(["ocre dev (jobs run locally; look for `[ocre jobs]` lines in the output)".to_owned()]);
    for queue in created {
        report.next.push(format!("ocre deploy creates the queue {queue} and its dead-letter queue"));
    }
    Ok(report)
}

/// The steps of a stepped job: those of `--steps`, or one `work` step for a
/// job with a lock only. Empty for a plain job.
fn step_names(stepped: &JobSteps, fields: &[super::fields::Field]) -> Result<Vec<String>, CliError> {
    if let Some(lock) = &stepped.lock
        && !fields.iter().any(|field| &field.name == lock)
    {
        return Err(CliError::new(format!("--lock {lock} is not a field of the job"))
            .hint("lock on one of the job's fields, e.g. `ocre g job ProcessVideo video_id:integer --lock video_id`"));
    }
    let steps = if stepped.steps.is_empty() && stepped.lock.is_some() {
        vec!["work".to_owned()]
    } else {
        stepped.steps.clone()
    };
    for (i, step) in steps.iter().enumerate() {
        let reserved =
            ["new", "perform", "perform_later"].contains(&step.as_str()) || RESERVED.contains(&step.as_str());
        if !is_identifier(step) || step.chars().any(|c| c.is_ascii_uppercase()) || reserved {
            return Err(CliError::new(format!("invalid step name `{step}`"))
                .hint("name steps in snake_case, e.g. `--steps download,transcode,notify`"));
        }
        if steps[..i].contains(step) {
            return Err(CliError::new(format!("step `{step}` is listed twice")).hint("give each step its own name"));
        }
        if fields.iter().any(|field| &field.name == step) {
            return Err(
                CliError::new(format!("step `{step}` has the name of a field")).hint("rename the step or the field")
            );
        }
    }
    Ok(steps)
}

/// A job that runs `steps` one queue message each, optionally one run per
/// value of the `lock` field at a time.
fn stepped_job_rs(
    module: &str,
    pascal: &str,
    fields: &[(String, String)],
    steps: &[String],
    lock: Option<&str>,
    command: &str,
    queue: &str,
) -> String {
    let sender = if queue == DEFAULT_QUEUE {
        "ocre::jobs::enqueue(ctx, &super::Job::".to_owned()
    } else {
        format!("ocre::jobs::queue(ctx, \"{queue}\").enqueue(&super::Job::")
    };
    let delayed = if queue == DEFAULT_QUEUE {
        "ocre::jobs::enqueue_in(ctx, &super::Job::".to_owned()
    } else {
        format!("ocre::jobs::queue(ctx, \"{queue}\").enqueue_in(&super::Job::")
    };
    let mut struct_fields = String::new();
    let mut params = Vec::new();
    for (name, ty) in fields {
        writeln!(struct_fields, "    pub {name}: {ty},").expect("writing to a String");
        params.push(format!("{name}: {ty}"));
    }
    let names: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
    let init = if names.is_empty() { String::new() } else { format!("{}, ", names.join(", ")) };
    let count = steps.len();
    let list = steps.iter().map(|step| format!("`{step}`")).collect::<Vec<_>>().join(", ");
    let mut arms = String::new();
    let mut methods = String::new();
    for (i, step) in steps.iter().enumerate() {
        writeln!(arms, "            {i} => self.{step}(ctx).await?,").expect("writing to a String");
        write!(
            methods,
            "\n    /// Step {} of {count}: `{step}`. Await D1, `fetch` and R2 freely (waiting costs no CPU);\n    \
             /// return `Err` to retry this step.\n    async fn {step}(&self, _ctx: &Ctx) -> Result<()> {{\n        Ok(())\n    }}\n",
            i + 1
        )
        .expect("writing to a String");
    }
    let (lock_doc, lock_take, finish) = match lock {
        Some(field) => (
            format!(
                "//!\n//! One run per `{field}` at a time: the run takes the lock `{module}:<{field}>`\n\
                 //! (table `job_locks`) at its first step and releases it after the last;\n\
                 //! a second run for the same `{field}` waits, enqueued again every 30 s (3\n\
                 //! Queues operations each time). A run whose step exhausted its retries\n\
                 //! loses its lock {LOCK_TTL_MINUTES} minutes after that step started.\n"
            ),
            format!(
                "        let db = ctx.db()?;\n        let key = format!(\"{module}:{{}}\", self.{field});\n        \
                 if !jobs::lock(&db, &key, &self.run, LOCK_TTL).await? {{\n            \
                 // Another run holds it: try again in 30 s, without using up this job's retries.\n            \
                 return {delayed}{pascal}(self), Duration::from_secs(30)).await;\n        }}\n"
            ),
            "            jobs::unlock(&db, &key, &self.run).await".to_owned(),
        ),
        None => (String::new(), String::new(), "            Ok(())".to_owned()),
    };
    let lock_const = if lock.is_some() {
        format!(
            "\n/// Seconds a run keeps its lock after starting a step: more than the queue's retries of a step.\nconst LOCK_TTL: i64 = {};\n",
            LOCK_TTL_MINUTES * 60
        )
    } else {
        String::new()
    };
    let imports = if lock.is_some() {
        "use std::time::Duration;\n\nuse ocre::{Ctx, Result, jobs};"
    } else {
        "use ocre::{Ctx, Result};"
    };
    format!(
        r#"//! {human} job. Generated by `{command}`.
//!
//! Enqueue it from a handler: `{pascal}::new({names}).perform_later(&ctx).await?`
//! (with `use crate::jobs::{pascal};`).
//!
//! It runs in {count} steps, one queue message each: {list}. Each step
//! enqueues the next once it succeeded; a step that fails is retried on its
//! own (30 s, 1 min, 3 min, 9 min, 27 min), from that step. Keep each step
//! within the free plan's 10 ms of CPU: work that takes longer belongs to
//! another service (see the webhooks guide).
{lock_doc}
{imports}
use serde::{{Deserialize, Serialize}};
{lock_const}
/// The job's arguments and progress, stored in the queue message as JSON.
#[derive(Debug, Serialize, Deserialize)]
pub struct {pascal} {{
{struct_fields}    /// The step to run next (0 is `{first}`).
    #[serde(default)]
    pub step: usize,
    /// This run's id, set by `perform_later`; kept from step to step.
    #[serde(default)]
    pub run: String,
}}

impl {pascal} {{
    /// The steps, in order.
    pub const STEPS: [&str; {count}] = [{quoted}];

    /// A run starting at the first step.
    pub fn new({params}) -> Self {{
        Self {{ {init}step: 0, run: String::new() }}
    }}

    /// Sends the job (its next step) to the queue; a new run gets its id.
    pub fn perform_later(mut self, ctx: &Ctx) -> impl Future<Output = Result<()>> + Send + use<> {{
        if self.run.is_empty() {{
            self.run = ocre::token::generate();
        }}
        {sender}{pascal}(self))
    }}

    /// Runs the current step, then enqueues the next one.
    pub async fn perform(mut self, ctx: &Ctx) -> Result<()> {{
        if self.run.is_empty() {{
            // Enqueued without `perform_later`: give the run its id first.
            return self.perform_later(ctx).await;
        }}
{lock_take}        match self.step {{
{arms}            _ => {{}}
        }}
        self.step += 1;
        if self.step < Self::STEPS.len() {{
            self.perform_later(ctx).await
        }} else {{
{finish}
        }}
    }}
{methods}}}
"#,
        human = humanize(module),
        names = names.join(", "),
        first = steps[0],
        quoted = steps.iter().map(|step| format!("\"{step}\"")).collect::<Vec<_>>().join(", "),
        params = params.join(", "),
    )
}

/// How long a run keeps its lock after a step: longer than the queue's
/// retries of a step (about 40 minutes), so a retried step keeps it.
const LOCK_TTL_MINUTES: i64 = 60;

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
/// and the queue in cloudflare.config.ts unless a `JOBS` producer exists.
/// Returns the queue's name.
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
    // `GET /ocre/dev/jobs.json` for request tests (`Client::jobs`); a 404 in release builds.
    let lib = insert_after_marker(&lib, ROUTES_MARKER, ".merge(ocre::jobs::dev_routes())").ok_or_else(|| {
        CliError::new(format!("src/lib.rs is missing the `{ROUTES_MARKER}` marker"))
            .hint(format!("put `{ROUTES_MARKER}` on its own line inside `routes()`, where `.route(...)` calls go"))
    })?;
    edits.update("src/lib.rs", lib + QUEUE_EVENT);
    edits.update("src/jobs/mod.rs", REGISTRY.to_owned());
    let config = read_config(edits)?;
    if config.binding("JOBS").is_some() {
        return config.binding_field("JOBS", "name", "JOBS: bindings.queue({ name: \"<app-name>-jobs\" }),");
    }
    let app = config.worker_name()?.to_owned();
    let queue = format!("{app}-jobs");
    add_queue(
        edits,
        &config,
        &format!(
            "// Background jobs (`ocre g job`): the Worker sends jobs to this queue and runs
// them (src/jobs/). `ocre deploy` creates it and its dead-letter queue. Free
// plan: 10,000 Queues operations a day (a job costs 3: write, read, delete; a
// retry one more read), messages kept 24 hours, 128 KB each.
JOBS: bindings.queue({{ name: \"{queue}\" }}),"
        ),
        &format!(
            "// Runs the jobs of {queue}: up to 10 messages per run, waiting at most 5 s
// to fill a batch. Each run is one Worker request with 10 ms of CPU on the free
// plan: lower maxBatchSize for CPU-heavy jobs. A failing job is retried with a
// growing delay (30 s, 1 min, 3 min, 9 min, 27 min), then moved to
// {queue}-failed, where it stays 24 hours (dashboard: Queues > {queue}-failed).
triggers.queue({{ name: \"{queue}\", deadLetterQueue: \"{queue}-failed\", maxBatchSize: 10, maxBatchTimeout: 5, maxRetries: 5 }}),"
        ),
    )?;
    Ok(queue)
}

/// A named queue: its producer `JOBS_<NAME>` and consumer, unless
/// cloudflare.config.ts already binds it. Returns the queue created, if any.
fn wire_queue(edits: &mut Edits, queue: &str) -> Result<Option<String>, CliError> {
    let binding = queue_binding(queue);
    let config = read_config(edits)?;
    if config.binding(&binding).is_some() {
        return Ok(None);
    }
    let name = format!("{}-jobs-{queue}", config.worker_name()?);
    add_queue(
        edits,
        &config,
        &format!(
            "// The `{queue}` job queue (`ocre::jobs::queue(&ctx, \"{queue}\")`): its own
// consumer, so its jobs never wait behind the `default` queue. Same Queues
// costs per message; batches wait at most 1 s.
{binding}: bindings.queue({{ name: \"{name}\" }}),"
        ),
        &format!(
            "triggers.queue({{ name: \"{name}\", deadLetterQueue: \"{name}-failed\", maxBatchSize: 10, maxBatchTimeout: 1, maxRetries: 5 }}),"
        ),
    )?;
    Ok(Some(name))
}

/// Adds a queue's producer binding and its consumer trigger.
fn add_queue(edits: &mut Edits, config: &Config, producer: &str, consumer: &str) -> Result<(), CliError> {
    let text = config.insert(ENV_MARKER, producer)?;
    let text = Config::parse(text)?.insert(TRIGGERS_MARKER, consumer)?;
    edits.update(config::FILE, text);
    Ok(())
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

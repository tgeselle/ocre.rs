//! Everything that runs Cloudflare's `cf` CLI: login, dev, deploy, remote
//! databases, secrets; and [`LocalD1`], the one place that still runs
//! wrangler. Database commands built on them live in `db.rs`.
//!
//! Apps pin `cf` and `wrangler` in their package.json; commands run the
//! app's `node_modules/.bin/cf` (fast), or `npx cf@<pin>` outside an app.

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    CliResult,
    config::Config,
    output::{CliError, Report},
    project::{Project, check_wasm_target},
    secret::{self, SECRET_KEY_BASE},
};

/// The `cf` release apps and this CLI agree on (package.json of new apps).
pub const CF_VERSION: &str = "1.0.0-beta.5";
/// The wrangler cf delegates builds to, and [`LocalD1`] runs.
pub const WRANGLER_VERSION: &str = "4.144.0";
/// Type-checks cloudflare.config.ts (`npx tsc -p .`).
pub const TYPESCRIPT_VERSION: &str = "5.9.3";
/// Oldest wrangler with the `cf-wrangler` entry point cf delegates to.
pub const WRANGLER_MIN: (u32, u32) = (4, 136);
/// cf's `engines.node`.
pub const NODE_MIN: u32 = 22;

/// Hint for remote failures that are usually a missing login.
const LOGIN_HINT: &str = "log in with `ocre login`, or set CLOUDFLARE_API_TOKEN (and CLOUDFLARE_ACCOUNT_ID when the token sees several accounts)";

/// Where a tool's stdout goes while it runs.
#[derive(Clone, Copy)]
pub enum Echo {
    /// Human mode: show it.
    Stdout,
    /// `--json` mode: keep stdout for the JSON result.
    Stderr,
    /// Interactive wizard: hide it behind a spinner; include it in errors.
    Capture,
}

impl Echo {
    pub fn for_json(json: bool) -> Self {
        if json { Self::Stderr } else { Self::Stdout }
    }
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct Account {
    pub id: String,
    pub name: String,
}

/// The Cloudflare session, from `cf auth whoami` (JSON, exit 0 even when
/// logged out: `authenticated` says).
#[derive(Debug, Deserialize, PartialEq)]
pub struct Session {
    #[serde(default)]
    authenticated: bool,
    pub email: Option<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

/// A failed cf API command: the API error code from cf's error box
/// (`[10006] ...`), and the box itself.
#[derive(Debug)]
struct ApiFailure {
    code: Option<u32>,
    message: String,
}

pub struct Cloudflare<'a> {
    root: &'a Path,
    echo: Echo,
    /// `worker-build` mode, read by wrangler.config.ts's build command as `$OCRE_BUILD`.
    build: &'static str,
}

impl<'a> Cloudflare<'a> {
    /// `root` is the app (or, before `ocre new`, the working directory).
    pub fn new(root: &'a Path, echo: Echo) -> Self {
        Self { root, echo, build: "--release" }
    }

    /// Unoptimized builds for `ocre dev`: much faster to compile.
    pub fn dev_build(self) -> Self {
        Self { build: "--dev", ..self }
    }

    /// The current session, or `None` when not logged in.
    pub fn whoami(&self) -> Result<Option<Session>, CliError> {
        let output = self.command().args(["auth", "whoami"]).stderr(Stdio::piped()).output().map_err(cf_missing)?;
        if !output.status.success() {
            return Ok(None);
        }
        let session: Session = serde_json::from_slice(&output.stdout)
            .map_err(|err| CliError::new(format!("unexpected `cf auth whoami` output: {err}")))?;
        Ok(session.authenticated.then_some(session))
    }

    /// Returns the session, running the browser login first if needed.
    pub fn ensure_login(&self) -> Result<Session, CliError> {
        if let Some(session) = self.whoami()? {
            return Ok(session);
        }
        self.run(&["auth", "login"])?;
        self.whoami()?.ok_or_else(|| {
            CliError::new("Cloudflare login did not complete")
                .hint("run `ocre login` and approve access in the browser, or set CLOUDFLARE_API_TOKEN")
        })
    }

    /// The id of the D1 database named `name`, `None` when it does not exist.
    pub fn database_id(&self, name: &str) -> Result<Option<String>, CliError> {
        let databases: Vec<Value> = self.api_json(&["d1", "list", "--name", name])?;
        Ok(databases.iter().find(|db| db["name"] == name).and_then(|db| db["uuid"].as_str()).map(str::to_owned))
    }

    /// The id of the D1 database named `name`, created when missing; `true`
    /// when this call created it.
    pub fn ensure_database(&self, name: &str) -> Result<(String, bool), CliError> {
        if let Some(id) = self.database_id(name)? {
            return Ok((id, false));
        }
        let created: Value = self.api_json(&["d1", "create", "--name", name])?;
        let id = created["uuid"].as_str().ok_or_else(|| {
            CliError::new(format!("`cf d1 create --name {name}` returned no uuid: {created}"))
                .hint("run `ocre deploy` again: it finds the database by name once Cloudflare lists it")
        })?;
        Ok((id.to_owned(), true))
    }

    /// Applies the pending migrations of `migrations/` to the remote database.
    pub fn migrate_remote(&self, id: &str) -> Result<(), CliError> {
        self.run(&["d1", "migrations", "apply", id]).map(drop)
    }

    /// Names of the migrations not applied to the remote database yet.
    pub fn pending_remote(&self, id: &str) -> Result<Vec<String>, CliError> {
        let listed: Vec<Value> = self.api_json(&["d1", "migrations", "list", id])?;
        Ok(listed.iter().filter_map(|migration| migration["Name"].as_str().map(str::to_owned)).collect())
    }

    /// Runs `sql` (any number of statements) on the remote database; returns
    /// the JSON result, one `{results, success, meta}` object per statement.
    /// The SQL goes through a file: cf reads a `--sql` value starting with
    /// `--` (a comment) as a flag.
    pub fn query_remote(&self, id: &str, sql: &str) -> Result<String, CliError> {
        const BATCH: &str = ".wrangler/ocre-batch.json";
        let path = self.root.join(BATCH);
        std::fs::create_dir_all(path.parent().expect("the batch file is in .wrangler/"))?;
        std::fs::write(&path, serde_json::json!([{ "sql": sql }]).to_string())?;
        let result = self.api(&["d1", "query", id, "--batch", &format!("@{BATCH}")]);
        let _ = std::fs::remove_file(&path);
        result?.map_err(|failure| failure.error(&format!("cf d1 query {id}")))
    }

    /// Creates each queue of the config (producers, consumers, dead-letter
    /// queues) that `cf queues list` does not have; returns `queue <name>`
    /// for each one created.
    fn ensure_queues(&self, config: &Config) -> Result<Vec<String>, CliError> {
        let wanted = config.queue_names()?;
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let existing: Vec<Value> = self.api_json(&["queues", "list"])?;
        let mut created = Vec::new();
        for queue in wanted {
            if existing.iter().any(|q| q["queue_name"] == queue.as_str()) {
                continue;
            }
            self.api_json::<Value>(&["queues", "create", "--queue-name", &queue])?;
            created.push(format!("queue {queue}"));
        }
        Ok(created)
    }

    /// Creates each R2 bucket of the config that `cf r2 buckets get` reports
    /// missing (API code 10006); returns `R2 bucket <name>` for each one created.
    fn ensure_buckets(&self, config: &Config) -> Result<Vec<String>, CliError> {
        let mut created = Vec::new();
        for bucket in config.bucket_names()? {
            match self.api(&["r2", "buckets", "get", &bucket])? {
                Ok(_) => continue,
                Err(failure) if failure.code == Some(10006) => {}
                Err(failure) if failure.code == Some(10042) => {
                    return Err(CliError::new(format!("`cf r2 buckets get {bucket}` failed: {}", failure.message)).hint(
                        "enable R2 once in the Cloudflare dashboard (Storage & databases > R2; the free plan asks for a payment method but charges nothing within 10 GB, 1M writes and 10M reads a month), then run `ocre deploy` again",
                    ));
                }
                Err(failure) => return Err(failure.error(&format!("cf r2 buckets get {bucket}"))),
            }
            self.api_json::<Value>(&["r2", "buckets", "create", "--name", &bucket])?;
            created.push(format!("R2 bucket {bucket}"));
        }
        Ok(created)
    }

    /// Gives each KV binding without an `id` the namespace titled
    /// `<worker>-<binding>` (`blog-cache`), creating it when missing, and
    /// writes the id into cloudflare.config.ts so every later command uses
    /// the same namespace. Returns `KV namespace <title>` for each one created.
    fn ensure_kv_namespaces(&self, config: Config) -> Result<Vec<String>, CliError> {
        let bindings: Vec<String> = config.kv_without_id().into_iter().map(str::to_owned).collect();
        if bindings.is_empty() {
            return Ok(Vec::new());
        }
        let worker = config.worker_name()?.to_owned();
        let namespaces = self.kv_namespaces()?;
        let mut config = config;
        let mut created = Vec::new();
        for binding in bindings {
            let title = format!("{worker}-{}", binding.to_ascii_lowercase().replace('_', "-"));
            let id = match namespaces.iter().find(|ns| ns["title"] == title.as_str()).and_then(|ns| ns["id"].as_str()) {
                Some(id) => id.to_owned(),
                None => {
                    let namespace: Value = self.api_json(&["kv", "namespaces", "create", "--title", &title])?;
                    created.push(format!("KV namespace {title} (id written to {})", crate::config::FILE));
                    namespace["id"].as_str().map(str::to_owned).ok_or_else(|| {
                        CliError::new(format!("`cf kv namespaces create --title {title}` returned no id: {namespace}"))
                            .hint("run `ocre deploy` again: it links the namespace once Cloudflare lists it")
                    })?
                }
            };
            let text = config.set_kv_id(&binding, &id)?;
            std::fs::write(self.root.join(crate::config::FILE), &text)?;
            config = Config::parse(text)?;
        }
        Ok(created)
    }

    /// Every KV namespace of the account (the list is paginated).
    fn kv_namespaces(&self) -> Result<Vec<Value>, CliError> {
        const PER_PAGE: usize = 100;
        let mut all = Vec::new();
        for page in 1.. {
            let batch: Vec<Value> =
                self.api_json(&["kv", "namespaces", "list", "--per-page", "100", "--page", &page.to_string()])?;
            let last = batch.len() < PER_PAGE;
            all.extend(batch);
            if last {
                break;
            }
        }
        Ok(all)
    }

    /// Names of the deployed Worker's secrets, or `None` when the Worker does
    /// not exist yet (values cannot be read back). Any other failure is an
    /// error, so an existing secret is never overwritten by mistake.
    pub fn secret_names(&self, worker: &str) -> Result<Option<Vec<String>>, CliError> {
        match self.api(&["workers", "secrets", "list", "--worker", worker])? {
            Ok(stdout) => {
                let secrets: Vec<Value> = serde_json::from_str(&stdout)
                    .map_err(|err| CliError::new(format!("unexpected `cf workers secrets list` output: {err}")))?;
                Ok(Some(secrets.iter().filter_map(|secret| secret["name"].as_str().map(str::to_owned)).collect()))
            }
            Err(failure) if failure.code == Some(10007) => Ok(None),
            Err(failure) => {
                Err(CliError::new(format!("`cf workers secrets list --worker {worker}` failed: {}", failure.message))
                    .hint(format!("{LOGIN_HINT}; Ocre only creates SECRET_KEY_BASE when sure the Worker has none")))
            }
        }
    }

    /// Uploads `values` as secrets of the deployed Worker in one call (a new
    /// Worker version, no rebuild). The values are on disk only during the call.
    /// The body is wrapped in `secrets` with each entry naming itself: cf
    /// 1.0.0-beta.5 accepts an unwrapped map but creates nothing
    /// (tests/support/cf_fixtures/secrets_bulk_unwrapped_*).
    pub fn put_secrets(&self, worker: &str, values: &BTreeMap<String, String>) -> Result<(), CliError> {
        const PUSH_FILE: &str = ".wrangler/ocre-secrets-push.json";
        let secrets: serde_json::Map<String, Value> = values
            .iter()
            .map(|(name, text)| {
                (name.clone(), serde_json::json!({ "name": name, "type": "secret_text", "text": text }))
            })
            .collect();
        let body = serde_json::json!({ "secrets": secrets });
        let file = PrivateFile::create(self.root, PUSH_FILE, &body.to_string())?;
        let result = self.run(&["workers", "secrets", "bulk", "--worker", worker, "--file", PUSH_FILE]);
        drop(file);
        result.map(drop)
    }

    /// Deploys and returns the workers.dev URL. Ocre provisions everything
    /// first: the D1 database, queues, KV namespaces (ids written back) and
    /// R2 buckets. A Worker without SECRET_KEY_BASE gets the one of
    /// `.prod.vars`, else a new one, which is also written to `.prod.vars`:
    /// Cloudflare never gives a secret back, and losing it signs everyone
    /// out and makes encrypted columns unreadable. An existing one is never
    /// replaced. Migrations run before the new code goes live.
    ///
    /// `--secrets-file` is passed on every deploy, as `{}` when there is no
    /// secret to add: cf 1.0.0-beta.5 uploads the new version with
    /// `keepSecrets: keepVars || !!secretsFile`, so a deploy without it drops
    /// every secret of the Worker (`ocre secrets push` values included).
    /// cf rejects an empty `.env` file, hence JSON.
    pub fn deploy(&self, project: &Project) -> Result<Deployed, CliError> {
        require_install(self.root)?;
        let config = project.config()?;
        let worker = config.worker_name()?.to_owned();
        let (database, created) = self.ensure_database(&project.database_name)?;
        let mut provisioned = Vec::new();
        if created {
            provisioned.push(format!("D1 database {}", project.database_name));
        }
        provisioned.extend(self.ensure_queues(&config)?);
        provisioned.extend(self.ensure_buckets(&config)?);
        provisioned.extend(self.ensure_kv_namespaces(config)?);
        let has_secret = self.secret_names(&worker)?.is_some_and(|names| names.iter().any(|n| n == SECRET_KEY_BASE));
        let kept = crate::secrets::read_vars(self.root, PROD_VARS)?.remove(SECRET_KEY_BASE);
        let new_secret = if has_secret { None } else { Some(kept.clone().unwrap_or_else(secret::generate)) };
        let secrets = match &new_secret {
            Some(value) => serde_json::json!({ (SECRET_KEY_BASE): value }),
            None => serde_json::json!({}),
        };
        let secrets_file = PrivateFile::create(self.root, SECRETS_FILE, &secrets.to_string())?;
        self.migrate_remote(&database)?;
        let output = self.run(&["deploy", "--secrets-file", SECRETS_FILE])?;
        drop(secrets_file);
        let url = output
            .split_whitespace()
            .find(|word| word.starts_with("https://") && word.contains(".workers.dev"))
            .map(str::to_owned);
        // A new secret is kept where the app's other production values live.
        let secret_saved = match (&new_secret, &kept) {
            (Some(value), None) => {
                save_secret(self.root, value)?;
                true
            }
            _ => false,
        };
        Ok(Deployed { url, secret_created: new_secret.is_some(), secret_saved, provisioned })
    }

    /// Runs cf with its stdout routed by `echo`, and returns that stdout.
    pub fn run(&self, args: &[&str]) -> Result<String, CliError> {
        let mut command = self.command();
        command.args(args);
        run(command, &format!("cf {}", args.join(" ")), self.echo)
    }

    /// Runs an API command captured: stdout on success, else the failure.
    fn api(&self, args: &[&str]) -> Result<Result<String, ApiFailure>, CliError> {
        let output = self.command().args(args).stderr(Stdio::piped()).output().map_err(cf_missing)?;
        if output.status.success() {
            return Ok(Ok(String::from_utf8_lossy(&output.stdout).into_owned()));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(Err(ApiFailure { code: api_code(&stderr), message: error_box(&stderr) }))
    }

    /// Runs an API command and parses its JSON result.
    fn api_json<T: DeserializeOwned>(&self, args: &[&str]) -> Result<T, CliError> {
        let label = format!("cf {}", args.join(" "));
        let stdout = self.api(args)?.map_err(|failure| failure.error(&label))?;
        serde_json::from_str(&stdout).map_err(|err| CliError::new(format!("unexpected `{label}` output: {err}")))
    }

    /// The app's `node_modules/.bin/cf` when installed, else `npx cf@<pin>`.
    /// Never reads the terminal: cf then takes its non-interactive defaults.
    fn command(&self) -> Command {
        let local = self.root.join("node_modules/.bin/cf");
        let mut command = if local.is_file() {
            Command::new(local)
        } else {
            let mut npx = Command::new("npx");
            npx.args(["--yes", &format!("cf@{CF_VERSION}")]);
            npx
        };
        command.current_dir(self.root).stdin(Stdio::null()).env("OCRE_BUILD", self.build);
        command
    }
}

impl ApiFailure {
    fn error(self, label: &str) -> CliError {
        CliError::new(format!("`{label}` failed: {}", self.message)).hint(LOGIN_HINT)
    }
}

/// The API error code in cf's error box: `[10006] The specified bucket does not exist`.
fn api_code(stderr: &str) -> Option<u32> {
    stderr.match_indices('[').find_map(|(at, _)| {
        let rest = &stderr[at + 1..];
        let end = rest.find(']')?;
        rest[..end].parse().ok()
    })
}

/// cf's boxed error (`┌ Error ... └`) without the banner and notices before it.
fn error_box(stderr: &str) -> String {
    stderr.find('┌').map_or(stderr, |at| &stderr[at..]).trim().to_owned()
}

/// Runs `command` with its stdout routed by `echo`, and returns that stdout.
/// `label` names it in errors (`cf deploy`, `wrangler d1 ...`).
fn run(mut command: Command, label: &str, echo: Echo) -> Result<String, CliError> {
    command.stdout(Stdio::piped());
    if let Echo::Capture = echo {
        command.stderr(Stdio::piped());
    }
    let tool = label.split(' ').next().expect("labels start with the tool");
    let mut child = command.spawn().map_err(|err| missing(tool, err))?;
    // Drain stderr concurrently so a full pipe never blocks the tool.
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = pipe.read_to_string(&mut text);
            text
        })
    });
    let mut captured = String::new();
    for line in BufReader::new(child.stdout.take().expect("stdout is piped")).lines() {
        let line = line?;
        match echo {
            Echo::Stdout => writeln!(std::io::stdout(), "{line}")?,
            Echo::Stderr => writeln!(std::io::stderr(), "{line}")?,
            Echo::Capture => {}
        }
        captured.push_str(&line);
        captured.push('\n');
    }
    let status = child.wait()?;
    let stderr = stderr.map(|reader| reader.join().expect("stderr reader does not panic")).unwrap_or_default();
    if status.success() {
        return Ok(captured);
    }
    let mut message = format!("`{label}` failed ({status})");
    let hint = if let Echo::Capture = echo {
        captured.push_str(&stderr);
        message.push_str(":\n");
        message.push_str(captured.trim_end());
        format!("the {tool} output above names the cause")
    } else {
        format!("read the {tool} output above; the first error line names the cause")
    };
    Err(CliError::new(message).hint(hint))
}

fn cf_missing(err: std::io::Error) -> CliError {
    missing("cf", err)
}

fn missing(tool: &str, err: std::io::Error) -> CliError {
    CliError::new(format!("could not run {tool}: {err}"))
        .hint(format!("install Node.js {NODE_MIN} or newer, then run `npm install` in the app"))
}

/// Remote commands and `cf dev` need the app's pinned packages.
fn require_install(root: &Path) -> Result<(), CliError> {
    if root.join("node_modules/.bin/cf").is_file() && root.join("node_modules/.bin/wrangler").is_file() {
        return Ok(());
    }
    Err(CliError::new("the app's npm packages are not installed (node_modules/.bin/cf, node_modules/.bin/wrangler)")
        .hint(format!("run `npm install` in {} (needs Node.js {NODE_MIN} or newer)", root.display())))
}

/// SQL for [`LocalD1::execute`]: a statement, or a file of them.
pub enum Sql<'a> {
    Command(&'a str),
    /// Relative to the app root.
    File(&'a str),
}

/// The wrangler fallbacks: the local D1 database, and the server of
/// `ocre test --e2e`. cf 1.0.0-beta.5 addresses D1 only by UUID while
/// `cf dev` keys the local database by its binding name, keeps local state
/// outside the app, and its local writes do not exit; `cf dev` also always
/// uses `.wrangler/state`, so tests could not get their own database. The
/// app's own wrangler reads a config derived from cloudflare.config.ts and
/// shares `cf dev`'s state in `.wrangler/state` (or the test state).
pub struct LocalD1<'a> {
    project: &'a Project,
    echo: Echo,
    state: &'static str,
}

impl<'a> LocalD1<'a> {
    /// Derived wrangler config, relative to the app root.
    pub const CONFIG: &'static str = ".wrangler/ocre-d1.json";
    /// Local state shared with `cf dev`, relative to the app root.
    pub const STATE: &'static str = ".wrangler/state";
    /// Local state of `ocre test --e2e` runs, recreated by each run.
    pub const TEST_STATE: &'static str = ".wrangler/test-state";

    pub fn new(project: &'a Project, echo: Echo) -> Self {
        Self { project, echo, state: Self::STATE }
    }

    /// The same database in the test state instead of the development one.
    pub fn for_tests(self) -> Self {
        Self { state: Self::TEST_STATE, ..self }
    }

    /// Applies the pending migrations.
    pub fn migrate(&self) -> Result<(), CliError> {
        self.run(&["d1", "migrations", "apply", "DB", "--local"], self.echo).map(drop)
    }

    /// Names of the migrations not applied yet.
    pub fn pending(&self) -> Result<Vec<String>, CliError> {
        Ok(pending_migrations(&self.run(&["d1", "migrations", "list", "DB", "--local"], Echo::Capture)?))
    }

    /// Runs `sql`; output routed by the echo mode. `--yes` answers
    /// wrangler's import prompt.
    pub fn execute(&self, sql: Sql) -> Result<String, CliError> {
        let mut args = vec!["d1", "execute", "DB", "--local"];
        match sql {
            Sql::Command(command) => args.extend(["--command", command]),
            Sql::File(file) => args.extend(["--file", file, "--yes"]),
        }
        self.run(&args, self.echo)
    }

    /// Runs `sql` captured; returns wrangler's JSON, one `{results}` per statement.
    pub fn query(&self, sql: &str) -> Result<String, CliError> {
        self.run(&["d1", "execute", "DB", "--local", "--command", sql, "--json"], Echo::Capture)
    }

    /// Starts the app on `port` like `cf dev` does (wrangler's
    /// `--x-new-config` reads cloudflare.config.ts and wrangler.config.ts,
    /// an unoptimized build), but on this state directory. Stdout and stderr
    /// are piped; the server runs in its own process group so [`stop`] ends
    /// wrangler and workerd together.
    pub fn spawn_server(&self, port: u16) -> Result<std::process::Child, CliError> {
        let mut command = Command::new(self.wrangler()?);
        command
            .args(["dev", "--x-new-config", "--port", &port.to_string(), "--persist-to", self.state])
            .current_dir(&self.project.root)
            .env("OCRE_BUILD", "--dev")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command.spawn().map_err(|err| missing("wrangler", err))
    }

    /// The app's `node_modules/.bin/wrangler`, or an error naming the fix.
    fn wrangler(&self) -> Result<std::path::PathBuf, CliError> {
        let root = &self.project.root;
        let wrangler = root.join("node_modules/.bin/wrangler");
        if !wrangler.is_file() {
            return Err(CliError::new("the app's wrangler is not installed (node_modules/.bin/wrangler)").hint(
                format!("run `npm install` in {} (local databases run through the app's wrangler)", root.display()),
            ));
        }
        Ok(wrangler)
    }

    fn run(&self, args: &[&str], echo: Echo) -> Result<String, CliError> {
        let root = &self.project.root;
        let wrangler = self.wrangler()?;
        self.write_config()?;
        let mut command = Command::new(wrangler);
        command
            .args(args)
            .args(["-c", Self::CONFIG, "--persist-to", self.state])
            .current_dir(root)
            .stdin(Stdio::null());
        run(command, &format!("wrangler {}", args.join(" ")), echo)
    }

    /// `migrations_dir` is relative to the config file, in `.wrangler/`.
    fn write_config(&self) -> Result<(), CliError> {
        let config = self.project.config()?;
        let derived = serde_json::json!({
            "name": config.name.as_deref().unwrap_or(&self.project.database_name),
            "compatibility_date": config.compatibility_date.as_deref().unwrap_or("2026-09-01"),
            "d1_databases": [{
                "binding": "DB",
                "database_name": self.project.database_name,
                "migrations_dir": "../migrations",
            }],
        });
        let path = self.project.root.join(Self::CONFIG);
        std::fs::create_dir_all(path.parent().expect("the config is in .wrangler/"))?;
        std::fs::write(path, format!("{derived:#}\n"))?;
        Ok(())
    }
}

/// The app's D1 database: local (through [`LocalD1`]) or remote (through cf,
/// by id).
pub enum Database<'a> {
    Local(LocalD1<'a>),
    Remote(Cloudflare<'a>, String),
}

impl<'a> Database<'a> {
    /// With `remote`, resolves the database id; a missing database is an error.
    pub fn open(project: &'a Project, echo: Echo, remote: bool) -> Result<Self, CliError> {
        if !remote {
            return Ok(Self::Local(LocalD1::new(project, echo)));
        }
        let cf = Cloudflare::new(&project.root, echo);
        let id = cf.database_id(&project.database_name)?.ok_or_else(|| {
            CliError::new(format!("the D1 database {} does not exist on Cloudflare yet", project.database_name))
                .hint("run `ocre deploy` (or `ocre db create --remote`) first")
        })?;
        Ok(Self::Remote(cf, id))
    }

    pub fn migrate(&self) -> Result<(), CliError> {
        match self {
            Self::Local(local) => local.migrate(),
            Self::Remote(cf, id) => cf.migrate_remote(id),
        }
    }

    pub fn pending(&self) -> Result<Vec<String>, CliError> {
        match self {
            Self::Local(local) => local.pending(),
            Self::Remote(cf, id) => cf.pending_remote(id),
        }
    }

    /// Runs the statements of `file` (relative to the app root) for their effect.
    pub fn run_file(&self, file: &str) -> Result<(), CliError> {
        match self {
            Self::Local(local) => local.execute(Sql::File(file)).map(drop),
            Self::Remote(cf, id) => cf.query_remote(id, &std::fs::read_to_string(cf.root.join(file))?).map(drop),
        }
    }

    /// Runs `sql` captured; returns the JSON result, one `{results}` per statement.
    pub fn query(&self, sql: &str) -> Result<String, CliError> {
        match self {
            Self::Local(local) => local.query(sql),
            Self::Remote(cf, id) => cf.query_remote(id, sql),
        }
    }

    /// `--local` or `--remote`, for reports.
    pub fn target(&self) -> &'static str {
        match self {
            Self::Local(_) => "--local",
            Self::Remote(..) => "--remote",
        }
    }
}

/// Names from the "Migrations to be applied" table of `wrangler d1 migrations list`.
pub(crate) fn pending_migrations(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.trim().strip_prefix('│')?.strip_suffix('│'))
        .map(str::trim)
        .filter(|name| name.ends_with(".sql"))
        .map(str::to_owned)
        .collect()
}

/// Stops a process started by [`LocalD1::spawn_server`], with its children.
pub fn stop(mut child: std::process::Child) {
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-TERM", &format!("-{}", child.id())]).status();
    let _ = child.kill();
    let _ = child.wait();
}

/// Which account to write into cloudflare.config.ts, if any. One account
/// needs no `accountId`; several need the caller to choose.
pub fn pick_account(session: &Session, requested: Option<&str>) -> Result<Option<String>, CliError> {
    let listing = || session.accounts.iter().map(|a| format!("{} ({})", a.id, a.name)).collect::<Vec<_>>().join(", ");
    match requested {
        Some(id) if session.accounts.iter().any(|a| a.id == id) => Ok(Some(id.to_owned())),
        Some(id) => Err(CliError::new(format!("account `{id}` is not available to this Cloudflare login"))
            .hint(format!("use one of: {}", listing()))),
        None if session.accounts.len() == 1 => Ok(None),
        None if session.accounts.is_empty() => Err(CliError::new("this Cloudflare login has no accounts")
            .hint("create an account at https://dash.cloudflare.com/sign-up, then run `ocre login` again")),
        None => Err(CliError::new("this Cloudflare login has several accounts")
            .hint(format!("pass --account-id with one of: {}", listing()))),
    }
}

pub fn login(json: bool) -> CliResult {
    let cwd = std::env::current_dir()?;
    let session = Cloudflare::new(&cwd, Echo::for_json(json)).ensure_login()?;
    Ok(Report { email: session.email, ..Report::new("login") })
}

pub fn migrate(remote: bool, json: bool) -> CliResult {
    let project = Project::find()?;
    Database::open(&project, Echo::for_json(json), remote)?.migrate()?;
    Ok(Report { remote, ..Report::new("migrate") })
}

/// Runs until stopped. The app is served at `http://localhost:<port>`.
/// `cache`: `Some(false)` writes `CACHE_STORE=null` into .dev.vars
/// (`ocre::cache` computes every value), `Some(true)` takes it out; the
/// choice stays for the next runs, like `bin/rails dev:cache`.
pub fn dev(port: u16, cache: Option<bool>, json: bool) -> CliResult {
    let project = Project::find()?;
    crate::i18n::check_syntax(&project.root)?;
    check_wasm_target()?;
    require_install(&project.root)?;
    let ran = match cache {
        Some(on) => vec![crate::secrets::set_dev_cache(&project.root, on)?],
        None => Vec::new(),
    };
    LocalD1::new(&project, Echo::for_json(json)).migrate()?;
    Cloudflare::new(&project.root, Echo::for_json(json)).dev_build().run(&["dev", "--port", &port.to_string()])?;
    Ok(Report { url: Some(format!("http://localhost:{port}")), ran, ..Report::new("dev") })
}

/// `ocre logs`: streams the deployed Worker's live logs until stopped
/// (Ctrl-C) through the app's wrangler (`wrangler tail`): cf 1.0.0-beta.5
/// has no tail command. `format` is `pretty` or `json`; `status` keeps
/// invocations by outcome (`ok`, `error`, `canceled`); `search` keeps
/// events whose console lines contain the text.
pub fn logs(format: &str, status: &[String], search: Option<&str>, json: bool) -> CliResult {
    let project = Project::find()?;
    require_install(&project.root)?;
    let config = project.config()?;
    let name = config.worker_name()?;
    let mut args = vec!["tail", name, "--format", format];
    for status in status {
        args.extend(["--status", status.as_str()]);
    }
    if let Some(search) = search {
        args.extend(["--search", search]);
    }
    let mut command = Command::new(project.root.join("node_modules/.bin/wrangler"));
    command.args(&args).current_dir(&project.root).stdin(Stdio::null());
    run(command, &format!("wrangler {}", args.join(" ")), Echo::for_json(json)).map_err(|err| {
        err.hint(format!(
            "wrangler tail uses wrangler's own login: run `npx wrangler login` in {}, or set CLOUDFLARE_API_TOKEN; the Worker must be deployed (`ocre deploy`)",
            project.root.display()
        ))
    })?;
    Ok(Report::new("logs"))
}

pub fn deploy(json: bool) -> CliResult {
    let project = Project::find()?;
    crate::i18n::check_syntax(&project.root)?;
    check_wasm_target()?;
    let deployed = Cloudflare::new(&project.root, Echo::for_json(json)).deploy(&project)?;
    Ok(Report {
        url: deployed.url,
        secret_created: deployed.secret_created,
        secret_saved: deployed.secret_saved.then_some(PROD_VARS),
        provisioned: deployed.provisioned,
        ..Report::new("deploy")
    })
}

/// Result of [`Cloudflare::deploy`].
pub struct Deployed {
    /// The workers.dev URL, when cf printed one.
    pub url: Option<String>,
    /// A new SECRET_KEY_BASE was uploaded with this deploy.
    pub secret_created: bool,
    /// It was generated and written to `.prod.vars`.
    pub secret_saved: bool,
    /// Resources created because they were missing, e.g. `queue shop-jobs`.
    pub provisioned: Vec<String>,
}

/// Git-ignored file of production values (`ocre secrets push --file .prod.vars`).
const PROD_VARS: &str = ".prod.vars";

/// Appends a generated SECRET_KEY_BASE to `.prod.vars` (created readable by its owner only).
fn save_secret(root: &Path, value: &str) -> Result<(), CliError> {
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let line = format!(
        "# Created by `ocre deploy` and uploaded to the Worker, which never gives it back.\n\
         # Back it up (a password manager): losing it signs everyone out and makes encrypted columns unreadable.\n\
         {SECRET_KEY_BASE}={value}\n"
    );
    options.open(root.join(PROD_VARS))?.write_all(line.as_bytes())?;
    Ok(())
}

/// The secrets `cf deploy --secrets-file` adds (a new SECRET_KEY_BASE, or
/// none), as JSON, relative to the app root.
const SECRETS_FILE: &str = ".wrangler/ocre-secrets.json";

/// A file of secret values in the app's git-ignored `.wrangler/`, readable
/// by its owner only; deleted when dropped.
struct PrivateFile(PathBuf);

impl PrivateFile {
    fn create(root: &Path, relative: &str, contents: &str) -> Result<Self, CliError> {
        let file = Self(root.join(relative));
        std::fs::create_dir_all(file.0.parent().expect("the path has a parent"))?;
        // Left over by an interrupted run: replace it.
        let _ = std::fs::remove_file(&file.0);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&file.0)?.write_all(contents.as_bytes())?;
        Ok(file)
    }
}

impl Drop for PrivateFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
#[path = "../tests/cloudflare.rs"]
mod tests;

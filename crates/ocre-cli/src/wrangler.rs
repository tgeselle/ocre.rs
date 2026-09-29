//! Everything that runs wrangler: login, migrate, dev, deploy.
//! Database commands built on it live in `db.rs`.

use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::Deserialize;

use crate::{
    CliResult,
    output::{CliError, Report},
    project::{Project, check_wasm_target},
    secret::{self, SECRET_KEY_BASE},
};

/// Pinned major version, so generated apps and the CLI agree on flags.
const WRANGLER: &str = "wrangler@4";

/// Where wrangler's stdout goes while it runs.
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

/// Logged-in Cloudflare user, from `wrangler whoami --json`.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    #[serde(default)]
    logged_in: bool,
    pub email: Option<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

pub struct Wrangler<'a> {
    cwd: &'a Path,
    echo: Echo,
    /// `worker-build` mode, read by the app's `[build]` command as `$OCRE_BUILD`.
    build: &'static str,
}

impl<'a> Wrangler<'a> {
    pub fn new(cwd: &'a Path, echo: Echo) -> Self {
        Self { cwd, echo, build: "--release" }
    }

    /// Unoptimized builds for `ocre dev`: much faster to compile.
    pub fn dev_build(self) -> Self {
        Self { build: "--dev", ..self }
    }

    /// The current session, or `None` when not logged in.
    pub fn whoami(&self) -> Result<Option<Session>, CliError> {
        let output = self.command().args(["whoami", "--json"]).stderr(Stdio::piped()).output().map_err(npx_missing)?;
        if !output.status.success() {
            return Ok(None);
        }
        let session: Session = serde_json::from_slice(&output.stdout)
            .map_err(|err| CliError::new(format!("unexpected `wrangler whoami --json` output: {err}")))?;
        Ok(session.logged_in.then_some(session))
    }

    /// Returns the session, running the browser login first if needed.
    pub fn ensure_login(&self) -> Result<Session, CliError> {
        if let Some(session) = self.whoami()? {
            return Ok(session);
        }
        self.run(&["login"])?;
        self.whoami()?.ok_or_else(|| {
            CliError::new("Cloudflare login did not complete")
                .hint("run `ocre login` and approve access in the browser, or set CLOUDFLARE_API_TOKEN")
        })
    }

    pub fn migrate(&self, database: &str, remote: bool) -> Result<(), CliError> {
        self.run(&["d1", "migrations", "apply", database, target(remote)]).map(drop)
    }

    /// Deploys and returns the workers.dev URL. An existing database is
    /// migrated before the new code goes live; a new one is created by the
    /// first deploy, then migrated. A Worker without SECRET_KEY_BASE gets a
    /// new one with the deploy; an existing one is never replaced, since that
    /// would sign everyone out. Queues named in wrangler.toml are created
    /// first when missing (a consumer on a missing queue fails the deploy),
    /// and KV namespaces without an `id` are created or found and linked.
    /// R2 buckets (`[[r2_buckets]]`, e.g. `STORAGE` for `ocre::storage`) are
    /// created when missing.
    pub fn deploy(&self, database: &str) -> Result<Deployed, CliError> {
        let mut provisioned = self.ensure_queues()?;
        provisioned.extend(self.ensure_kv_namespaces()?);
        provisioned.extend(self.ensure_buckets()?);
        let secrets = if self.has_secret_key_base()? { None } else { Some(SecretsFile::create(self.cwd)?) };
        let mut deploy = vec!["deploy"];
        if secrets.is_some() {
            deploy.extend(["--secrets-file", SecretsFile::PATH]);
        }
        let output = if self.database_exists(database)? {
            self.migrate(database, true)?;
            self.run(&deploy)?
        } else {
            let output = self.run(&deploy)?;
            self.migrate(database, true)?;
            output
        };
        let url = output
            .split_whitespace()
            .find(|word| word.starts_with("https://") && word.contains(".workers.dev"))
            .map(str::to_owned);
        Ok(Deployed { url, secret_created: secrets.is_some(), provisioned })
    }

    /// Creates each queue of wrangler.toml (producers, consumers, dead-letter
    /// queues) that `wrangler queues info` reports missing; returns
    /// `queue <name>` for each one created.
    fn ensure_queues(&self) -> Result<Vec<String>, CliError> {
        let mut created = Vec::new();
        for queue in configured_queues(&std::fs::read_to_string(self.cwd.join("wrangler.toml"))?) {
            let output = self
                .command()
                .args(["queues", "info", &queue])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .map_err(npx_missing)?;
            if output.status.success() {
                continue;
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("does not exist") {
                return Err(CliError::new(format!("`wrangler queues info {queue}` failed: {}", stderr.trim()))
                    .hint("log in with `ocre login`, or set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID"));
            }
            self.run(&["queues", "create", &queue])?;
            created.push(format!("queue {queue}"));
        }
        Ok(created)
    }

    /// Creates each `bucket_name` of `[[r2_buckets]]` that `wrangler r2
    /// bucket info` reports missing (API code 10006); returns `R2 bucket
    /// <name>` for each one created.
    fn ensure_buckets(&self) -> Result<Vec<String>, CliError> {
        let mut created = Vec::new();
        for bucket in configured_buckets(&std::fs::read_to_string(self.cwd.join("wrangler.toml"))?) {
            let output = self
                .command()
                .args(["r2", "bucket", "info", &bucket, "--json"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .map_err(npx_missing)?;
            if output.status.success() {
                continue;
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("10006") && !stderr.contains("does not exist") {
                return Err(CliError::new(format!("`wrangler r2 bucket info {bucket}` failed: {}", stderr.trim())).hint(
                    if stderr.contains("10042") {
                        "enable R2 once in the Cloudflare dashboard (Storage & databases > R2; the free plan asks for a payment method but charges nothing within 10 GB, 1M writes and 10M reads a month), then run `ocre deploy` again"
                    } else {
                        "log in with `ocre login`, or set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID"
                    },
                ));
            }
            self.run(&["r2", "bucket", "create", &bucket])?;
            created.push(format!("R2 bucket {bucket}"));
        }
        Ok(created)
    }

    /// Gives each `[[kv_namespaces]]` entry without an `id` the namespace
    /// titled `<worker>-<binding>` (`blog-cache`), creating it when
    /// `wrangler kv namespace list` does not have it, and writes the id into
    /// wrangler.toml so every later command uses the same namespace. Returns
    /// `KV namespace <title>` for each one created.
    fn ensure_kv_namespaces(&self) -> Result<Vec<String>, CliError> {
        let path = self.cwd.join("wrangler.toml");
        let mut wrangler_toml = std::fs::read_to_string(&path)?;
        let (worker, bindings) = kv_bindings_without_id(&wrangler_toml);
        let mut created = Vec::new();
        for binding in bindings {
            let title = format!("{worker}-{}", binding.to_ascii_lowercase().replace('_', "-"));
            let id = match self.kv_namespace_id(&title)? {
                Some(id) => id,
                None => {
                    self.run(&["kv", "namespace", "create", &title])?;
                    created.push(format!("KV namespace {title} (id written to wrangler.toml)"));
                    self.kv_namespace_id(&title)?.ok_or_else(|| {
                        CliError::new(format!("KV namespace {title} was created but is not listed"))
                            .hint("run `ocre deploy` again; it links the namespace once Cloudflare lists it")
                    })?
                }
            };
            wrangler_toml = with_kv_id(&wrangler_toml, &binding, &id);
            std::fs::write(&path, &wrangler_toml)?;
        }
        Ok(created)
    }

    /// The id of the KV namespace titled `title`, from `wrangler kv namespace list`.
    fn kv_namespace_id(&self, title: &str) -> Result<Option<String>, CliError> {
        let output =
            self.command().args(["kv", "namespace", "list"]).stderr(Stdio::piped()).output().map_err(npx_missing)?;
        if !output.status.success() {
            return Err(CliError::new(format!(
                "`wrangler kv namespace list` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .hint("log in with `ocre login`, or set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID"));
        }
        let namespaces: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .map_err(|err| CliError::new(format!("unexpected `wrangler kv namespace list` output: {err}")))?;
        Ok(namespaces.iter().find(|ns| ns["title"] == title).and_then(|ns| ns["id"].as_str()).map(str::to_owned))
    }

    /// Whether the deployed Worker has SECRET_KEY_BASE; `false` when the
    /// Worker does not exist yet. Any other failure is an error, so an
    /// existing secret is never overwritten by mistake.
    fn has_secret_key_base(&self) -> Result<bool, CliError> {
        Ok(self.secret_names()?.is_some_and(|names| names.iter().any(|name| name == SECRET_KEY_BASE)))
    }

    /// Names of the deployed Worker's secrets, or `None` when the Worker does
    /// not exist yet (`wrangler secret list`; values cannot be read back).
    pub fn secret_names(&self) -> Result<Option<Vec<String>>, CliError> {
        let output = self
            .command()
            .args(["secret", "list", "--format", "json"])
            .stderr(Stdio::piped())
            .output()
            .map_err(npx_missing)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            if stderr.contains("not found") {
                return Ok(None);
            }
            return Err(CliError::new(format!("`wrangler secret list` failed: {}", stderr.trim())).hint(
                "log in with `ocre login`, or set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID; \
                 Ocre only creates SECRET_KEY_BASE when sure the Worker has none",
            ));
        }
        let secrets: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .map_err(|err| CliError::new(format!("unexpected `wrangler secret list` output: {err}")))?;
        Ok(Some(secrets.iter().filter_map(|secret| secret["name"].as_str().map(str::to_owned)).collect()))
    }

    pub(crate) fn database_exists(&self, name: &str) -> Result<bool, CliError> {
        let output =
            self.command().args(["d1", "list", "--json"]).stderr(Stdio::piped()).output().map_err(npx_missing)?;
        if !output.status.success() {
            return Err(CliError::new(format!(
                "`wrangler d1 list` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .hint("log in with `ocre login`, or set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID"));
        }
        let databases: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .map_err(|err| CliError::new(format!("unexpected `wrangler d1 list --json` output: {err}")))?;
        Ok(databases.iter().any(|db| db["name"] == name))
    }

    /// Runs wrangler with its stdout routed by `echo`, and returns that stdout.
    pub fn run(&self, args: &[&str]) -> Result<String, CliError> {
        let mut command = self.command();
        command.args(args).stdout(Stdio::piped());
        if let Echo::Capture = self.echo {
            command.stderr(Stdio::piped());
        }
        let mut child = command.spawn().map_err(npx_missing)?;
        // Drain stderr concurrently so a full pipe never blocks wrangler.
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
            match self.echo {
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
        let mut message = format!("`wrangler {}` failed ({status})", args.join(" "));
        let hint = if let Echo::Capture = self.echo {
            captured.push_str(&stderr);
            message.push_str(":\n");
            message.push_str(captured.trim_end());
            "the wrangler output above names the cause"
        } else {
            "read the wrangler output above; the first error line names the cause"
        };
        Err(CliError::new(message).hint(hint))
    }

    /// Starts wrangler in the background with stdout piped, in its own
    /// process group so [`stop`] ends npx, wrangler and workerd together.
    pub fn spawn(&self, args: &[&str]) -> Result<std::process::Child, CliError> {
        let mut command = self.command();
        command.args(args).stdout(Stdio::piped());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command.spawn().map_err(npx_missing)
    }

    fn command(&self) -> Command {
        let mut command = Command::new("npx");
        command.args(["--yes", WRANGLER]).current_dir(self.cwd).env("OCRE_BUILD", self.build);
        command
    }
}

fn npx_missing(err: std::io::Error) -> CliError {
    CliError::new(format!("could not run npx: {err}")).hint("install Node.js 20 or newer (it provides npx)")
}

/// Stops a process started by [`Wrangler::spawn`], with its children.
pub fn stop(mut child: std::process::Child) {
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-TERM", &format!("-{}", child.id())]).status();
    let _ = child.kill();
    let _ = child.wait();
}

/// Wrangler's D1 location flag.
pub fn target(remote: bool) -> &'static str {
    if remote { "--remote" } else { "--local" }
}

/// Which account to write into wrangler.toml, if any. One account needs no
/// `account_id`; several need the caller to choose.
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
    let session = Wrangler::new(&cwd, Echo::for_json(json)).ensure_login()?;
    Ok(Report { email: session.email, ..Report::new("login") })
}

pub fn migrate(remote: bool, json: bool) -> CliResult {
    let project = Project::find()?;
    Wrangler::new(&project.root, Echo::for_json(json)).migrate(&project.database_name, remote)?;
    Ok(Report::new("migrate"))
}

/// Runs until stopped. The app is served at `http://localhost:<port>`.
pub fn dev(port: u16, json: bool) -> CliResult {
    let project = Project::find()?;
    crate::i18n::check_syntax(&project.root)?;
    check_wasm_target()?;
    let wrangler = Wrangler::new(&project.root, Echo::for_json(json)).dev_build();
    wrangler.migrate(&project.database_name, false)?;
    wrangler.run(&["dev", "--port", &port.to_string()])?;
    Ok(Report { url: Some(format!("http://localhost:{port}")), ..Report::new("dev") })
}

pub fn deploy(json: bool) -> CliResult {
    let project = Project::find()?;
    crate::i18n::check_syntax(&project.root)?;
    check_wasm_target()?;
    let deployed = Wrangler::new(&project.root, Echo::for_json(json)).deploy(&project.database_name)?;
    Ok(Report {
        url: deployed.url,
        secret_created: deployed.secret_created,
        provisioned: deployed.provisioned,
        ..Report::new("deploy")
    })
}

/// Result of [`Wrangler::deploy`].
pub struct Deployed {
    /// The workers.dev URL, when wrangler printed one.
    pub url: Option<String>,
    /// A new SECRET_KEY_BASE was uploaded with this deploy.
    pub secret_created: bool,
    /// Resources created because they were missing, e.g. `queue shop-jobs`.
    pub provisioned: Vec<String>,
}

/// Every queue wrangler.toml names, once each, in order: producers'
/// `queue`, consumers' `queue` and `dead_letter_queue`.
fn configured_queues(wrangler_toml: &str) -> Vec<String> {
    let config: toml::Table = wrangler_toml.parse().expect("Project::find parsed wrangler.toml");
    let entries = |kind: &str| {
        config.get("queues").and_then(|q| q.get(kind)).and_then(|v| v.as_array()).cloned().unwrap_or_default()
    };
    let mut names: Vec<String> = Vec::new();
    for (kind, keys) in [("producers", &["queue"][..]), ("consumers", &["queue", "dead_letter_queue"])] {
        for entry in entries(kind) {
            for key in keys {
                if let Some(name) = entry.get(*key).and_then(|v| v.as_str())
                    && !names.iter().any(|known| known == name)
                {
                    names.push(name.to_owned());
                }
            }
        }
    }
    names
}

/// The `bucket_name` of each `[[r2_buckets]]` entry, once each.
fn configured_buckets(wrangler_toml: &str) -> Vec<String> {
    let config: toml::Table = wrangler_toml.parse().expect("Project::find parsed wrangler.toml");
    let mut names: Vec<String> = Vec::new();
    for entry in config.get("r2_buckets").and_then(|v| v.as_array()).into_iter().flatten() {
        if let Some(name) = entry.get("bucket_name").and_then(|v| v.as_str())
            && !names.iter().any(|known| known == name)
        {
            names.push(name.to_owned());
        }
    }
    names
}

/// The Worker's `name` and the bindings of `[[kv_namespaces]]` entries
/// without an `id`.
fn kv_bindings_without_id(wrangler_toml: &str) -> (String, Vec<String>) {
    let config: toml::Table = wrangler_toml.parse().expect("Project::find parsed wrangler.toml");
    let worker = config.get("name").and_then(|name| name.as_str()).unwrap_or("app").to_owned();
    let bindings = config
        .get("kv_namespaces")
        .and_then(|namespaces| namespaces.as_array())
        .into_iter()
        .flatten()
        .filter(|namespace| namespace.get("id").is_none())
        .filter_map(|namespace| namespace.get("binding").and_then(|b| b.as_str()).map(str::to_owned))
        .collect();
    (worker, bindings)
}

/// Adds `id = "<id>"` after `binding = "<binding>"` in its `[[kv_namespaces]]` entry.
fn with_kv_id(wrangler_toml: &str, binding: &str, id: &str) -> String {
    let mut out = String::with_capacity(wrangler_toml.len() + id.len() + 8);
    let mut in_kv = false;
    for line in wrangler_toml.lines() {
        out.push_str(line);
        out.push('\n');
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_kv = trimmed == "[[kv_namespaces]]";
        } else if in_kv
            && trimmed
                .split_once('=')
                .is_some_and(|(key, value)| key.trim() == "binding" && value.trim().trim_matches('"') == binding)
        {
            out.push_str(&format!("id = \"{id}\"\n"));
        }
    }
    out
}

/// A new SECRET_KEY_BASE for `wrangler deploy --secrets-file`, in the app's
/// git-ignored `.wrangler/`. Readable by its owner only; deleted when dropped.
struct SecretsFile(PathBuf);

impl SecretsFile {
    /// Relative to the app root, where wrangler runs.
    const PATH: &str = ".wrangler/ocre-secrets.env";

    fn create(root: &Path) -> Result<Self, CliError> {
        let file = Self(root.join(Self::PATH));
        std::fs::create_dir_all(file.0.parent().expect("the path has a parent"))?;
        // Left over by an interrupted deploy: replace it with a fresh secret.
        let _ = std::fs::remove_file(&file.0);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&file.0)?.write_all(format!("{SECRET_KEY_BASE}={}\n", secret::generate()).as_bytes())?;
        Ok(file)
    }
}

impl Drop for SecretsFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
#[path = "../tests/wrangler.rs"]
mod tests;

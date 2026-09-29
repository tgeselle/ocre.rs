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
    /// would sign everyone out.
    pub fn deploy(&self, database: &str) -> Result<Deployed, CliError> {
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
        Ok(Deployed { url, secret_created: secrets.is_some() })
    }

    /// Whether the deployed Worker has SECRET_KEY_BASE; `false` when the
    /// Worker does not exist yet. Any other failure is an error, so an
    /// existing secret is never overwritten by mistake.
    fn has_secret_key_base(&self) -> Result<bool, CliError> {
        let output = self
            .command()
            .args(["secret", "list", "--format", "json"])
            .stderr(Stdio::piped())
            .output()
            .map_err(npx_missing)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            if stderr.contains("not found") {
                return Ok(false);
            }
            return Err(CliError::new(format!("`wrangler secret list` failed: {}", stderr.trim())).hint(
                "log in with `ocre login`, or set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID; \
                 Ocre only creates SECRET_KEY_BASE when sure the Worker has none",
            ));
        }
        let secrets: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .map_err(|err| CliError::new(format!("unexpected `wrangler secret list` output: {err}")))?;
        Ok(secrets.iter().any(|secret| secret["name"] == SECRET_KEY_BASE))
    }

    fn database_exists(&self, name: &str) -> Result<bool, CliError> {
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

    fn command(&self) -> Command {
        let mut command = Command::new("npx");
        command.args(["--yes", WRANGLER]).current_dir(self.cwd).env("OCRE_BUILD", self.build);
        command
    }
}

fn npx_missing(err: std::io::Error) -> CliError {
    CliError::new(format!("could not run npx: {err}")).hint("install Node.js 20 or newer (it provides npx)")
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
    check_wasm_target()?;
    let wrangler = Wrangler::new(&project.root, Echo::for_json(json)).dev_build();
    wrangler.migrate(&project.database_name, false)?;
    wrangler.run(&["dev", "--port", &port.to_string()])?;
    Ok(Report { url: Some(format!("http://localhost:{port}")), ..Report::new("dev") })
}

pub fn deploy(json: bool) -> CliResult {
    let project = Project::find()?;
    check_wasm_target()?;
    let deployed = Wrangler::new(&project.root, Echo::for_json(json)).deploy(&project.database_name)?;
    Ok(Report { url: deployed.url, secret_created: deployed.secret_created, ..Report::new("deploy") })
}

/// Result of [`Wrangler::deploy`].
pub struct Deployed {
    /// The workers.dev URL, when wrangler printed one.
    pub url: Option<String>,
    /// A new SECRET_KEY_BASE was uploaded with this deploy.
    pub secret_created: bool,
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
mod tests;

//! `ocre doctor`: checks the tools and the app's setup, and exits non-zero
//! when a check fails, so it can gate CI. Warnings (not logged in, pending
//! migrations) do not fail it.
//!
//! Checks: the Rust wasm32 target, Node.js (npx), the Cloudflare login, and
//! inside an app: bindings in wrangler.toml for what the code uses, pending
//! local migrations, SECRET_KEY_BASE in .dev.vars, and the production
//! secrets when logged in.

use std::process::{Command, Stdio};

use serde::Serialize;

use crate::{
    CliResult,
    output::{CliError, Report},
    project::{Project, check_wasm_target},
    secret::SECRET_KEY_BASE,
    wrangler::{Echo, Wrangler},
};

#[derive(Serialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

/// One line of `ocre doctor`.
#[derive(Serialize, Debug)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self { name, status: Status::Ok, detail: detail.into(), hint: None }
    }

    fn warn(name: &'static str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self { name, status: Status::Warn, detail: detail.into(), hint: Some(hint.into()) }
    }

    fn fail(name: &'static str, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self { name, status: Status::Fail, detail: detail.into(), hint: Some(hint.into()) }
    }

    fn from_error(name: &'static str, err: CliError) -> Self {
        Self { name, status: Status::Fail, detail: err.message, hint: err.hint }
    }
}

/// Code patterns that need a binding: check name, what the code contains
/// (as generated: `#[worker::event(queue)]`...), binding check, fix.
struct Needs {
    name: &'static str,
    uses: &'static [&'static str],
    has: fn(&toml::Table) -> bool,
    fix: &'static str,
}

const NEEDS: [Needs; 5] = [
    Needs {
        name: "cache binding",
        uses: &["ocre::cache::"],
        has: |w| array_has(w, "kv_namespaces", "binding", "CACHE"),
        fix: "run `ocre g cache` (adds the CACHE KV binding)",
    },
    Needs {
        name: "storage binding",
        uses: &["ocre::storage::", "storage::{"],
        has: |w| array_has(w, "r2_buckets", "binding", "STORAGE"),
        fix: "add the STORAGE [[r2_buckets]] binding: generating an `attachment` field adds it",
    },
    Needs {
        name: "jobs queue",
        uses: &["event(queue)"],
        has: |w| {
            let queues = w.get("queues");
            ["producers", "consumers"]
                .iter()
                .all(|kind| queues.and_then(|q| q.get(*kind)).and_then(|v| v.as_array()).is_some_and(|v| !v.is_empty()))
        },
        fix: "restore the [[queues.producers]] and [[queues.consumers]] entries `ocre g job` added",
    },
    Needs {
        name: "cron triggers",
        uses: &["event(scheduled)"],
        has: |w| {
            w.get("triggers").and_then(|t| t.get("crons")).and_then(|c| c.as_array()).is_some_and(|c| !c.is_empty())
        },
        fix: "add the schedules' expressions to `[triggers] crons` (`ocre g schedule` does)",
    },
    Needs {
        name: "realtime binding",
        uses: &["ocre::realtime::", "realtime::WebSocketUpgrade", "realtime::broadcast"],
        has: |w| {
            w.get("durable_objects")
                .and_then(|d| d.get("bindings"))
                .and_then(|b| b.as_array())
                .is_some_and(|b| b.iter().any(|o| o.get("class_name").and_then(|c| c.as_str()) == Some("OcreChannel")))
        },
        fix: "restore the OcreChannel [[durable_objects.bindings]] and [[migrations]] entries `ocre g scaffold --realtime` added",
    },
];

fn array_has(wrangler: &toml::Table, key: &str, field: &str, value: &str) -> bool {
    wrangler
        .get(key)
        .and_then(|v| v.as_array())
        .is_some_and(|entries| entries.iter().any(|e| e.get(field).and_then(|f| f.as_str()) == Some(value)))
}

pub fn doctor() -> CliResult {
    let mut checks = vec![match check_wasm_target() {
        Ok(()) => Check::ok("rust", "wasm32-unknown-unknown target installed"),
        Err(err) => Check::from_error("rust", err),
    }];
    let npx = Command::new("npx").arg("--version").stdout(Stdio::piped()).stderr(Stdio::null()).output();
    let node = match npx {
        Ok(output) if output.status.success() => {
            Check::ok("node", format!("npx {}", String::from_utf8_lossy(&output.stdout).trim()))
        }
        _ => Check::fail("node", "npx not found", "install Node.js 20 or newer (it provides npx, which runs wrangler)"),
    };
    let has_node = node.status == Status::Ok;
    checks.push(node);
    let project = Project::find().ok();
    let cwd = std::env::current_dir()?;
    let root = project.as_ref().map_or(cwd.as_path(), |p| p.root.as_path());
    let wrangler = Wrangler::new(root, Echo::Capture);
    let logged_in = has_node && {
        let login = match wrangler.whoami() {
            Ok(Some(session)) => {
                Check::ok("cloudflare login", format!("logged in as {}", session.email.unwrap_or_default()))
            }
            Ok(None) => Check::warn("cloudflare login", "not logged in (needed by ocre deploy)", "run `ocre login`"),
            Err(err) => Check::from_error("cloudflare login", err),
        };
        let ok = login.status == Status::Ok;
        checks.push(login);
        ok
    };
    if let Some(project) = &project {
        app_checks(project, &wrangler, has_node, logged_in, &mut checks)?;
    }
    let failed: Vec<&str> = checks.iter().filter(|c| c.status == Status::Fail).map(|c| c.name).collect();
    let failure = (!failed.is_empty()).then(|| {
        CliError::new(format!("{} check(s) failed: {}", failed.len(), failed.join(", ")))
            .hint("fix each failed check as its hint says, then run `ocre doctor` again")
    });
    Ok(Report { checks, failure, ..Report::new("doctor") })
}

fn app_checks(
    project: &Project,
    wrangler: &Wrangler,
    has_node: bool,
    logged_in: bool,
    checks: &mut Vec<Check>,
) -> Result<(), CliError> {
    let config: toml::Table =
        std::fs::read_to_string(project.root.join("wrangler.toml"))?.parse().expect("Project::find parsed it");
    let code = source_text(&project.root.join("src"))?;
    for needs in &NEEDS {
        if needs.uses.iter().any(|pattern| code.contains(pattern)) {
            checks.push(if (needs.has)(&config) {
                Check::ok(needs.name, "configured in wrangler.toml")
            } else {
                Check::fail(needs.name, "the code uses it but wrangler.toml does not declare it", needs.fix)
            });
        }
    }
    if has_node {
        let listing = wrangler.run(&["d1", "migrations", "list", &project.database_name, "--local"]);
        checks.push(match listing.map(|output| crate::db::pending_migrations(&output)) {
            Ok(pending) if pending.is_empty() => Check::ok("migrations", "local database up to date"),
            Ok(pending) => {
                Check::warn("migrations", format!("pending locally: {}", pending.join(", ")), "run `ocre migrate`")
            }
            Err(err) => Check::warn("migrations", err.message, "run `ocre migrate --status` to see the error"),
        });
    }
    let dev_vars = std::fs::read_to_string(project.root.join(".dev.vars")).unwrap_or_default();
    let prefix = format!("{SECRET_KEY_BASE}=");
    checks.push(if dev_vars.lines().any(|line| line.trim_start().starts_with(&prefix)) {
        Check::ok("local secrets", format!("{SECRET_KEY_BASE} set in .dev.vars"))
    } else {
        Check::fail(
            "local secrets",
            format!("{SECRET_KEY_BASE} missing from .dev.vars: sessions and signed cookies fail in `ocre dev`"),
            format!("run `echo {SECRET_KEY_BASE}=$(ocre secret) >> .dev.vars`"),
        )
    });
    if logged_in {
        checks.push(remote_secrets(wrangler, &config));
    }
    Ok(())
}

/// Secrets the deployed Worker needs: SECRET_KEY_BASE, and RESEND_API_KEY
/// when `MAIL_ADAPTER = "resend"`.
fn remote_secrets(wrangler: &Wrangler, config: &toml::Table) -> Check {
    let name = "production secrets";
    let output = match wrangler.run(&["secret", "list", "--format", "json"]) {
        Ok(output) => output,
        Err(err) if err.message.contains("not found") => return Check::ok(name, "not deployed yet"),
        Err(err) => return Check::warn(name, err.message, "run `npx wrangler secret list` to see the error"),
    };
    let listed: Vec<serde_json::Value> = serde_json::from_str(&output).unwrap_or_default();
    let has = |secret: &str| listed.iter().any(|s| s["name"] == secret);
    let mut needed = vec![SECRET_KEY_BASE];
    if config.get("vars").and_then(|v| v.get("MAIL_ADAPTER")).and_then(|a| a.as_str()) == Some("resend") {
        needed.push("RESEND_API_KEY");
    }
    let missing: Vec<&str> = needed.into_iter().filter(|secret| !has(secret)).collect();
    if missing.is_empty() {
        Check::ok(name, "set on the deployed Worker")
    } else {
        Check::warn(
            name,
            format!("missing on the deployed Worker: {}", missing.join(", ")),
            "`ocre deploy` creates SECRET_KEY_BASE; set others with `npx wrangler secret put NAME`",
        )
    }
}

/// Every `.rs` file under `dir`, concatenated (empty when it is missing).
fn source_text(dir: &std::path::Path) -> Result<String, CliError> {
    let mut text = String::new();
    for path in crate::stats::files(dir)? {
        if path.extension().is_some_and(|ext| ext == "rs") {
            text.push_str(&std::fs::read_to_string(&path)?);
        }
    }
    Ok(text)
}

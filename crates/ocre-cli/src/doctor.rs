//! `ocre doctor`: checks the tools and the app's setup, and exits non-zero
//! when a check fails, so it can gate CI. Warnings (not logged in, pending
//! migrations) do not fail it.
//!
//! Checks: the Rust wasm32 target, Node.js, the Cloudflare login, and inside
//! an app: the npm packages against the pinned versions, cloudflare.config.ts
//! (cf's own loader, then `tsc`), bindings for what the code uses, pending
//! local migrations, SECRET_KEY_BASE in .dev.vars, and the production
//! secrets when logged in.

use std::{
    path::Path,
    process::{Command, Stdio},
};

use serde::Serialize;

use crate::{
    CliResult,
    cloudflare::{CF_VERSION, Cloudflare, Echo, LocalD1, NODE_MIN, WRANGLER_MIN, WRANGLER_VERSION},
    config::{self, Config},
    output::{CliError, Report},
    project::{Project, check_wasm_target},
    secret::SECRET_KEY_BASE,
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
    has: fn(&Config) -> bool,
    fix: &'static str,
}

const NEEDS: [Needs; 5] = [
    Needs {
        name: "cache binding",
        uses: &["ocre::cache::"],
        has: |c| c.binding("CACHE").is_some_and(|call| call.kind == "kv"),
        fix: "run `ocre g cache` (adds `CACHE: bindings.kv(),` to cloudflare.config.ts)",
    },
    Needs {
        name: "storage binding",
        uses: &["ocre::storage::", "storage::{"],
        has: |c| c.binding("STORAGE").is_some_and(|call| call.kind == "r2"),
        fix: "add `STORAGE: bindings.r2({ name: \"<app>-storage\" }),` to worker.env: generating an `attachment` field adds it",
    },
    Needs {
        name: "jobs queue",
        uses: &["event(queue)"],
        has: |c| c.bindings("queue").next().is_some() && c.triggers("queue").next().is_some(),
        fix: "restore the `JOBS: bindings.queue(...)` binding and the `triggers.queue(...)` trigger `ocre g job` added",
    },
    Needs {
        name: "cron triggers",
        uses: &["event(scheduled)"],
        has: |c| c.triggers("scheduled").next().is_some(),
        fix: "add the schedules' expressions as `triggers.scheduled({ schedule: \"...\" }),` (`ocre g schedule` does)",
    },
    Needs {
        name: "realtime binding",
        uses: &["ocre::realtime::", "realtime::WebSocketUpgrade", "realtime::broadcast"],
        has: |c| {
            c.bindings("durableObject").any(|call| call.field("exportName") == Some("OcreChannel"))
                && c.export("OcreChannel").is_some()
        },
        fix: "restore the `CHANNELS: bindings.durableObject(...)` binding and the `OcreChannel: exports.durableObject(...)` export `ocre g scaffold --realtime` added",
    },
];

pub fn doctor() -> CliResult {
    let mut checks = vec![match check_wasm_target() {
        Ok(()) => Check::ok("rust", "wasm32-unknown-unknown target installed"),
        Err(err) => Check::from_error("rust", err),
    }];
    let node = node_check(Command::new("node").arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output());
    let has_node = node.status == Status::Ok;
    checks.push(node);
    let project = Project::find().ok();
    let cwd = std::env::current_dir()?;
    let root = project.as_ref().map_or(cwd.as_path(), |p| p.root.as_path());
    let cloudflare = Cloudflare::new(root, Echo::Capture);
    let logged_in = has_node && {
        let login = match cloudflare.whoami() {
            Ok(Some(session)) => {
                Check::ok("cloudflare login", format!("logged in as {}", session.email.unwrap_or_default()))
            }
            Ok(None) => Check::warn("cloudflare login", "not logged in (needed by ocre deploy)", login_hint()),
            Err(err) => Check::from_error("cloudflare login", err),
        };
        let ok = login.status == Status::Ok;
        checks.push(login);
        ok
    };
    if let Some(project) = &project {
        app_checks(project, &cloudflare, has_node, logged_in, &mut checks)?;
    }
    let failed: Vec<&str> = checks.iter().filter(|c| c.status == Status::Fail).map(|c| c.name).collect();
    let failure = (!failed.is_empty()).then(|| {
        CliError::new(format!("{} check(s) failed: {}", failed.len(), failed.join(", ")))
            .hint("fix each failed check as its hint says, then run `ocre doctor` again")
    });
    Ok(Report { checks, failure, ..Report::new("doctor") })
}

/// `node --version` must say v22 or newer (cf's `engines`).
fn node_check(output: std::io::Result<std::process::Output>) -> Check {
    let install = format!("install Node.js {NODE_MIN} or newer (cf needs it; it provides npm)");
    let version = match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        _ => return Check::fail("node", "node not found", install),
    };
    let major = version.trim_start_matches('v').split('.').next().and_then(|major| major.parse::<u32>().ok());
    match major {
        Some(major) if major >= NODE_MIN => Check::ok("node", format!("node {version}")),
        _ => Check::fail("node", format!("node {version} is too old"), install),
    }
}

/// cf keeps its own login: someone logged in with wrangler must log in once more.
fn login_hint() -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let wrangler_login = [".wrangler/config/default.toml", "Library/Preferences/.wrangler/config/default.toml"]
        .iter()
        .any(|path| home.join(path).is_file());
    if wrangler_login {
        "run `ocre login`: cf keeps its own login, separate from wrangler's, so log in once more".to_owned()
    } else {
        "run `ocre login`".to_owned()
    }
}

fn app_checks(
    project: &Project,
    cloudflare: &Cloudflare,
    has_node: bool,
    logged_in: bool,
    checks: &mut Vec<Check>,
) -> Result<(), CliError> {
    let installed = packages_check(&project.root);
    let has_packages = installed.status != Status::Fail;
    checks.push(installed);
    if has_node && has_packages {
        checks.push(config_check(&project.root));
    }
    let config = project.config()?;
    let code = source_text(&project.root.join("src"))?;
    for needs in &NEEDS {
        if needs.uses.iter().any(|pattern| code.contains(pattern)) {
            checks.push(if (needs.has)(&config) {
                Check::ok(needs.name, format!("configured in {}", config::FILE))
            } else {
                Check::fail(needs.name, format!("the code uses it but {} does not declare it", config::FILE), needs.fix)
            });
        }
    }
    if has_node && has_packages {
        checks.push(match LocalD1::new(project, Echo::Capture).pending() {
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
        checks.push(remote_secrets(cloudflare, &config));
    }
    Ok(())
}

/// `version` of `node_modules/<package>/package.json`.
fn installed_version(root: &Path, package: &str) -> Option<String> {
    let text = std::fs::read_to_string(root.join("node_modules").join(package).join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&text).ok()?;
    manifest["version"].as_str().map(str::to_owned)
}

/// cf and wrangler installed in the app, at the versions this CLI pins.
fn packages_check(root: &Path) -> Check {
    let name = "npm packages";
    let (Some(cf), Some(wrangler)) = (installed_version(root, "cf"), installed_version(root, "wrangler")) else {
        return Check::fail(name, "cf and wrangler are not installed in the app", "run `npm install` in the app");
    };
    let (major, minor) = WRANGLER_MIN;
    let numbers: Vec<u32> = wrangler.split('.').take(2).filter_map(|n| n.parse().ok()).collect();
    if numbers.len() < 2 || (numbers[0], numbers[1]) < WRANGLER_MIN {
        return Check::fail(
            name,
            format!("wrangler {wrangler} is older than {major}.{minor}, which cf delegates builds to"),
            format!("run `npm install --save-dev --save-exact wrangler@{WRANGLER_VERSION}`"),
        );
    }
    if cf != CF_VERSION || wrangler != WRANGLER_VERSION {
        return Check::warn(
            name,
            format!(
                "cf {cf}, wrangler {wrangler}; this Ocre CLI is tested with cf {CF_VERSION}, wrangler {WRANGLER_VERSION}"
            ),
            format!("run `npm install --save-dev --save-exact cf@{CF_VERSION} wrangler@{WRANGLER_VERSION}`"),
        );
    }
    Check::ok(name, format!("cf {cf}, wrangler {wrangler}"))
}

/// Parses cloudflare.config.ts with cf's own loader (`@cloudflare/config`),
/// then type-checks it with the app's TypeScript when installed.
fn config_check(root: &Path) -> Check {
    const LOADER: &str = r#"import { loadAndParseConfig } from "@cloudflare/config";
const { result } = await loadAndParseConfig(process.argv[1], { isPreview: false, mode: undefined });
if (!result.success) { console.error(JSON.stringify(result.error.issues)); process.exit(1); }"#;
    let name = "config";
    let path = root.join(config::FILE);
    let loaded = Command::new("node")
        .args(["--input-type=module", "-e", LOADER])
        .arg(&path)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    // `node` answered the version check just before, so only its verdict matters here.
    if let Ok(output) = loaded
        && !output.status.success()
    {
        return Check::fail(
            name,
            format!("{} is invalid: {}", config::FILE, lossy(&output.stderr)),
            "fix the entries named above; `npx tsc -p .` shows the same errors with line numbers",
        );
    }
    let tsc = root.join("node_modules/.bin/tsc");
    if tsc.is_file() {
        let checked = Command::new(tsc).args(["-p", "."]).current_dir(root).stdin(Stdio::null()).output();
        if let Ok(output) = checked
            && !output.status.success()
        {
            return Check::fail(
                name,
                format!("`tsc -p .` failed:\n{}", lossy(&output.stdout)),
                format!("fix the type errors in {} or {}", config::FILE, config::BUILD_FILE),
            );
        }
    }
    Check::ok(name, format!("{} is valid", config::FILE))
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

/// Secrets the deployed Worker needs: SECRET_KEY_BASE, and RESEND_API_KEY
/// when `MAIL_ADAPTER` is `"resend"`.
fn remote_secrets(cloudflare: &Cloudflare, config: &Config) -> Check {
    let name = "production secrets";
    let listed = match config.worker_name().and_then(|worker| cloudflare.secret_names(worker)) {
        Ok(Some(listed)) => listed,
        Ok(None) => return Check::ok(name, "not deployed yet"),
        Err(err) => return Check::warn(name, err.message, "run `ocre secrets list` to see the error"),
    };
    let mut needed = vec![SECRET_KEY_BASE];
    if config.vars().any(|(key, value)| key == "MAIL_ADAPTER" && value == Some("resend")) {
        needed.push("RESEND_API_KEY");
    }
    let missing: Vec<&str> = needed.into_iter().filter(|secret| !listed.iter().any(|s| s == secret)).collect();
    if missing.is_empty() {
        Check::ok(name, "set on the deployed Worker")
    } else {
        Check::warn(
            name,
            format!("missing on the deployed Worker: {}", missing.join(", ")),
            "`ocre deploy` creates SECRET_KEY_BASE; upload others with `ocre secrets push NAME --file .prod.vars`",
        )
    }
}

/// Every `.rs` file under `dir`, concatenated (empty when it is missing).
fn source_text(dir: &Path) -> Result<String, CliError> {
    let mut text = String::new();
    for path in crate::stats::files(dir)? {
        if path.extension().is_some_and(|ext| ext == "rs") {
            text.push_str(&std::fs::read_to_string(&path)?);
        }
    }
    Ok(text)
}

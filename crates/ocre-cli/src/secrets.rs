//! `ocre secrets list` and `ocre secrets push`: the Worker secrets of
//! production next to the local ones in `.dev.vars`.
//!
//! Worker secrets are Ocre's credentials (Rails' `credentials.yml.enc`):
//! Cloudflare stores them encrypted, the Worker reads them with
//! `ctx.env().secret(NAME)`, and nothing secret is committed. Values can be
//! written but never read back, so `list` shows names only.

use std::{collections::BTreeMap, fs, path::Path};

use serde::Serialize;

use crate::{
    CliResult,
    output::{CliError, Report},
    project::Project,
    secret::SECRET_KEY_BASE,
    wrangler::{Echo, Wrangler},
};

/// A secret known locally, in production, or both.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SecretStatus {
    pub name: String,
    /// Set in `.dev.vars` (used by `ocre dev`).
    pub local: bool,
    /// Set on the deployed Worker.
    pub deployed: bool,
}

/// Where `ocre secrets push` uploads the values from (git-ignored).
const PUSH_FILE: &str = ".wrangler/ocre-secrets-push.json";

/// `NAME=value` lines of a `.dev.vars`-style file; comments and blank lines skipped.
pub fn parse_vars(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| {
            let value = value.trim();
            let unquoted = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|value| value.strip_suffix('\'')))
                .unwrap_or(value);
            (name.trim().to_owned(), unquoted.to_owned())
        })
        .collect()
}

fn read_vars(root: &Path, file: &str) -> Result<BTreeMap<String, String>, CliError> {
    match fs::read_to_string(root.join(file)) {
        Ok(text) => Ok(parse_vars(&text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(err) => Err(CliError::new(format!("cannot read {file}: {err}"))),
    }
}

/// Every secret name of `.dev.vars` and of the deployed Worker.
pub fn list(project: &Project, json: bool) -> CliResult {
    let local = read_vars(&project.root, ".dev.vars")?;
    let deployed = Wrangler::new(&project.root, Echo::for_json(json)).secret_names()?.unwrap_or_default();
    let mut names: Vec<&String> = local.keys().chain(&deployed).collect();
    names.sort();
    names.dedup();
    let secrets = names
        .into_iter()
        .map(|name| SecretStatus {
            name: name.clone(),
            local: local.contains_key(name),
            deployed: deployed.contains(name),
        })
        .collect::<Vec<_>>();
    let mut report = Report::new("secrets list");
    let missing: Vec<&str> = secrets
        .iter()
        .filter(|secret| secret.local && !secret.deployed && !DEV_ONLY.contains(&secret.name.as_str()))
        .map(|secret| secret.name.as_str())
        .collect();
    if !missing.is_empty() {
        report.next.push(format!("ocre secrets push {} --file <production values>", missing.join(" ")));
    }
    report.secrets = Some(secrets);
    Ok(report)
}

/// Names whose `.dev.vars` value is for development only.
const DEV_ONLY: [&str; 2] = [SECRET_KEY_BASE, "MAIL_ADAPTER"];

/// Uploads `names` with their values from `file` to the deployed Worker in
/// one `wrangler secret bulk` call (a new Worker version, no rebuild).
pub fn push(project: &Project, names: &[String], file: &str, json: bool) -> CliResult {
    if names.is_empty() {
        return Err(CliError::new("name the secrets to upload")
            .hint("e.g. `ocre secrets push GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET`; `ocre secrets list` shows them"));
    }
    let vars = read_vars(&project.root, file)?;
    let mut values = BTreeMap::new();
    for name in names {
        if file == ".dev.vars" && DEV_ONLY.contains(&name.as_str()) {
            return Err(CliError::new(format!("{name} in .dev.vars is a development value")).hint(format!(
                "production needs its own: `ocre deploy` creates SECRET_KEY_BASE; for others put the production \
                 value in another git-ignored file and pass `--file <it>`, or run `npx wrangler secret put {name}`"
            )));
        }
        let value = vars.get(name).ok_or_else(|| {
            CliError::new(format!("{name} is not set in {file}"))
                .hint(format!("add `{name}=<value>` to {file}, or run `npx wrangler secret put {name}`"))
        })?;
        values.insert(name.clone(), value.clone());
    }
    let path = project.root.join(PUSH_FILE);
    fs::create_dir_all(path.parent().expect("the push file is in .wrangler/"))?;
    fs::write(&path, serde_json::to_string(&values).expect("strings serialize"))?;
    let result = Wrangler::new(&project.root, Echo::for_json(json)).run(&["secret", "bulk", PUSH_FILE]);
    // The values must not stay on disk, whatever wrangler did.
    let removed = fs::remove_file(&path);
    result?;
    removed?;
    let mut report = Report::new("secrets push");
    report.ran = names.iter().map(|name| format!("uploaded {name}")).collect();
    Ok(report)
}

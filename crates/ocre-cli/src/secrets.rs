//! `ocre secrets list` and `ocre secrets push`: the Worker secrets of
//! production next to the local ones in `.dev.vars`.
//!
//! Worker secrets are Ocre's credentials (Rails' `credentials.yml.enc`):
//! Cloudflare stores them encrypted, the Worker reads them with
//! `ctx.secret(NAME).await`, and nothing secret is committed. Values can be
//! written but never read back, so `list` shows names only. Secrets shared by
//! several Workers can live in the account's Secrets Store instead
//! (`push --store`), bound to the Worker in cloudflare.config.ts.

use std::{collections::BTreeMap, fs, path::Path};

use serde::Serialize;

use crate::{
    CliResult,
    cloudflare::{Cloudflare, Echo},
    config::{self, Config, ENV_MARKER},
    output::{CliError, Report},
    project::Project,
    secret::SECRET_KEY_BASE,
};

/// A secret known locally, in production, or both.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SecretStatus {
    pub name: String,
    /// Set in `.dev.vars` (used by `ocre dev`).
    pub local: bool,
    /// Set on the deployed Worker, or in the Secrets Store it is bound to.
    pub deployed: bool,
    /// Bound to a Secrets Store secret: the store's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

/// `(binding, store id, secret name)` of the Secrets Store bindings of the config.
fn store_bindings(config: &Config) -> Vec<(String, String, String)> {
    config
        .bindings("secretsStoreSecret")
        .filter_map(|call| {
            Some((call.key.clone()?, call.field("storeId")?.to_owned(), call.field("secretName")?.to_owned()))
        })
        .collect()
}

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

pub(crate) fn read_vars(root: &Path, file: &str) -> Result<BTreeMap<String, String>, CliError> {
    match fs::read_to_string(root.join(file)) {
        Ok(text) => Ok(parse_vars(&text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(err) => Err(CliError::new(format!("cannot read {file}: {err}"))),
    }
}

/// Every secret name of `.dev.vars` and of the deployed Worker.
pub fn list(project: &Project, json: bool) -> CliResult {
    let local = read_vars(&project.root, ".dev.vars")?;
    let config = project.config()?;
    let worker = config.worker_name()?.to_owned();
    let cloudflare = Cloudflare::new(&project.root, Echo::for_json(json));
    let mut deployed = cloudflare.secret_names(&worker)?.unwrap_or_default();
    let bound = store_bindings(&config);
    // One list per store the Worker binds secrets of.
    let mut stores: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (_, store, _) in &bound {
        if !stores.contains_key(store.as_str()) {
            let names = cloudflare.store_secrets(store)?.into_iter().map(|(_, name)| name).collect();
            stores.insert(store, names);
        }
    }
    for (binding, store, secret) in &bound {
        if stores[store.as_str()].contains(secret) {
            deployed.push(binding.clone());
        }
    }
    let mut names: Vec<&String> =
        local.keys().chain(&deployed).chain(bound.iter().map(|(binding, ..)| binding)).collect();
    names.sort();
    names.dedup();
    let secrets = names
        .into_iter()
        .map(|name| SecretStatus {
            name: name.clone(),
            local: local.contains_key(name),
            deployed: deployed.contains(name),
            store: bound.iter().find(|(binding, ..)| binding == name).map(|(_, store, _)| store.clone()),
        })
        .collect::<Vec<_>>();
    let mut report = Report::new("secrets list");
    let missing: Vec<&str> = secrets
        .iter()
        .filter(|secret| secret.local && !secret.deployed && !DEV_ONLY.contains(&secret.name.as_str()))
        .map(|secret| secret.name.as_str())
        .collect();
    let (in_store, own): (Vec<&str>, Vec<&str>) =
        missing.iter().partition(|name| bound.iter().any(|(binding, ..)| binding == *name));
    if !own.is_empty() {
        report.next.push(format!("ocre secrets push {} --file <production values>", own.join(" ")));
    }
    if !in_store.is_empty() {
        report.next.push(format!("ocre secrets push {} --store --file <production values>", in_store.join(" ")));
    }
    report.secrets = Some(secrets);
    Ok(report)
}

/// Names whose `.dev.vars` value is for development only.
const DEV_ONLY: [&str; 3] = [SECRET_KEY_BASE, "MAIL_ADAPTER", CACHE_STORE];

/// `.dev.vars` variable that turns `ocre::cache` off (`ocre dev --no-cache`).
pub const CACHE_STORE: &str = ocre::cache::STORE_VAR;

/// Turns `ocre::cache` on (removes `CACHE_STORE` from .dev.vars) or off
/// (`CACHE_STORE=null`) for `ocre dev`; returns what it did.
pub fn set_dev_cache(root: &Path, on: bool) -> Result<String, CliError> {
    let path = root.join(".dev.vars");
    let text = fs::read_to_string(&path).unwrap_or_default();
    let prefix = format!("{CACHE_STORE}=");
    let mut lines: Vec<&str> = text.lines().filter(|line| !line.trim_start().starts_with(&prefix)).collect();
    let off = format!("{CACHE_STORE}=null");
    if !on {
        lines.push(&off);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    fs::write(&path, out)?;
    Ok(if on {
        "caching on: CACHE_STORE removed from .dev.vars".to_owned()
    } else {
        "caching off: CACHE_STORE=null in .dev.vars".to_owned()
    })
}

/// Prints the value of `name` from `file` (Rails' `credentials:fetch`), for
/// scripts: e.g. `export TOKEN=$(ocre secrets fetch TOKEN --file .prod.vars)`.
/// Only local files: Cloudflare never returns a secret's value.
pub fn fetch(project: &Project, name: &str, file: &str) -> CliResult {
    let vars = read_vars(&project.root, file)?;
    let value = vars.get(name).ok_or_else(|| {
        CliError::new(format!("{name} is not set in {file}")).hint(format!(
            "add `{name}=<value>` to {file}; deployed values cannot be read back (Cloudflare only stores them)"
        ))
    })?;
    Ok(Report { secret: Some(value.clone()), ..Report::new("secrets fetch") })
}

/// Uploads `names` with their values from `file` to the deployed Worker in
/// one `cf workers secrets bulk` call (a new Worker version, no rebuild).
pub fn push(project: &Project, names: &[String], file: &str, json: bool) -> CliResult {
    if names.is_empty() {
        return Err(CliError::new("name the secrets to upload")
            .hint("e.g. `ocre secrets push GITHUB_CLIENT_ID GITHUB_CLIENT_SECRET`; `ocre secrets list` shows them"));
    }
    let vars = read_vars(&project.root, file)?;
    let mut values = BTreeMap::new();
    for name in names {
        if file == ".dev.vars" && DEV_ONLY.contains(&name.as_str()) {
            return Err(CliError::new(format!("{name} in .dev.vars is a development value")).hint(
                "production needs its own: `ocre deploy` creates SECRET_KEY_BASE; for others put the production \
                 value in another git-ignored file (e.g. .prod.vars) and pass `--file <it>`",
            ));
        }
        let value = vars.get(name).ok_or_else(|| {
            CliError::new(format!("{name} is not set in {file}"))
                .hint(format!("add `{name}=<value>` to {file} (a git-ignored file), then run this again"))
        })?;
        values.insert(name.clone(), value.clone());
    }
    let worker = project.config()?.worker_name()?.to_owned();
    Cloudflare::new(&project.root, Echo::for_json(json)).put_secrets(&worker, &values)?;
    let mut report = Report::new("secrets push");
    report.ran = names.iter().map(|name| format!("uploaded {name}")).collect();
    Ok(report)
}

/// `push --store`: puts `names` (values from `file`) in the account's Secrets
/// Store and binds each to the Worker in cloudflare.config.ts. A name bound
/// already keeps its store and secret name; a new one goes to `store_id`, else
/// to the account's first store. The Worker reads them after its next deploy.
pub fn push_to_store(project: &Project, names: &[String], file: &str, store_id: Option<&str>, json: bool) -> CliResult {
    if names.is_empty() {
        return Err(CliError::new("name the secrets to upload")
            .hint("e.g. `ocre secrets push RESEND_API_KEY --store --file .prod.vars`"));
    }
    let vars = read_vars(&project.root, file)?;
    let mut config = project.config()?;
    let mut values = Vec::new();
    for name in names {
        if [SECRET_KEY_BASE, "R2_ACCESS_KEY_ID", "R2_SECRET_ACCESS_KEY"].contains(&name.as_str()) {
            return Err(CliError::new(format!("{name} cannot live in the Secrets Store")).hint(format!(
                "Ocre reads {name} without waiting for the store (sessions, presigned URLs): keep it a Worker secret, \
                 `ocre secrets push {name} --file {file}`"
            )));
        }
        if let Some(call) = config.binding(name).filter(|call| call.kind != "secretsStoreSecret") {
            return Err(CliError::new(format!("{name} is already a `{}` binding in {}", call.kind, config::FILE))
                .hint(format!(
                    "remove `{name}: bindings.{}(...)` from {}, then run this again",
                    call.kind,
                    config::FILE
                )));
        }
        let value = vars.get(name).ok_or_else(|| {
            CliError::new(format!("{name} is not set in {file}"))
                .hint(format!("add `{name}=<value>` to {file} (a git-ignored file), then run this again"))
        })?;
        values.push((name, value));
    }
    let cloudflare = Cloudflare::new(&project.root, Echo::for_json(json));
    let bound = store_bindings(&config);
    let mut default_store = store_id.map(str::to_owned);
    let mut report = Report::new("secrets push");
    for (name, value) in values {
        let (store, secret) = match bound.iter().find(|(binding, ..)| binding == name) {
            Some((_, store, secret)) => (store.clone(), secret.clone()),
            None => {
                if default_store.is_none() {
                    default_store = Some(first_store(&cloudflare)?);
                }
                let store = default_store.clone().expect("set above");
                let entry =
                    format!("{name}: bindings.secretsStoreSecret({{ storeId: \"{store}\", secretName: \"{name}\" }}),");
                let text = config.insert(ENV_MARKER, &entry)?;
                std::fs::write(project.root.join(config::FILE), &text)?;
                config = Config::parse(text)?;
                report.updated.push(config::FILE.to_owned());
                (store, name.clone())
            }
        };
        cloudflare.put_store_secret(&store, &secret, value)?;
        report.ran.push(format!("stored {name} in the Secrets Store {store}"));
    }
    report.updated.dedup();
    let worker = config.worker_name()?.to_owned();
    let worker_secrets = cloudflare.secret_names(&worker)?.unwrap_or_default();
    let clashes: Vec<&str> = names.iter().filter(|name| worker_secrets.contains(name)).map(String::as_str).collect();
    if !clashes.is_empty() {
        report.next.push(format!(
            "the Worker also has its own {} secret: delete it in the dashboard (Workers > {worker} > Settings > \
             Variables and Secrets) so the binding takes its name",
            clashes.join(", ")
        ));
    }
    if !report.updated.is_empty() {
        report.next.push("ocre deploy (the Worker reads a new binding once deployed)".to_owned());
    }
    Ok(report)
}

/// The account's first Secrets Store.
fn first_store(cloudflare: &Cloudflare) -> Result<String, CliError> {
    cloudflare.secret_stores()?.into_iter().next().map(|(id, _)| id).ok_or_else(|| {
        CliError::new("the account has no Secrets Store").hint(
            "open Secrets Store in the Cloudflare dashboard once (it creates the account's first store), or run \
             `npx wrangler secrets-store store create default --remote`, then run this again",
        )
    })
}

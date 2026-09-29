//! `ocre version` and `ocre about`: versions, then the app's resolved
//! configuration (bindings, variables, Ocre features), read from Cargo.toml,
//! wrangler.toml and rust-toolchain.toml without building anything.

use serde::Serialize;

use crate::{CliResult, output::Report, project::Project};

/// What `ocre about` reports; `ocre version` fills the first four fields.
#[derive(Serialize, Default)]
pub struct About {
    /// Version of this `ocre` CLI.
    pub cli: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// The app's `ocre` dependency: version, git source or path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust_toolchain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility_date: Option<String>,
    /// Cloudflare bindings and triggers, e.g. `D1 DB (blog)`, `cron 0 3 * * *`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<String>,
    /// Names of the `[vars]` (values stay out of reports).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vars: Vec<String>,
    /// Optional Ocre features turned on in Cargo.toml.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

impl About {
    /// Human form: one `name  value` line per known field.
    pub fn lines(&self) -> Vec<(&'static str, String)> {
        let mut lines = vec![("Ocre CLI", self.cli.to_owned())];
        let optional = [
            ("App", &self.app),
            ("App version", &self.app_version),
            ("Ocre crate", &self.ocre),
            ("Rust toolchain", &self.rust_toolchain),
            ("Compatibility date", &self.compatibility_date),
        ];
        for (name, value) in optional {
            if let Some(value) = value {
                lines.push((name, value.clone()));
            }
        }
        if let Some(mode) = self.mode {
            lines.push(("Mode", mode.to_owned()));
        }
        for (name, values) in
            [("Bindings", &self.bindings), ("Variables", &self.vars), ("Ocre features", &self.features)]
        {
            if !values.is_empty() {
                lines.push((name, values.join(", ")));
            }
        }
        lines
    }
}

const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `ocre version`: the CLI's version, and the app's inside an app.
pub fn version() -> CliResult {
    let mut about = About { cli: CLI_VERSION, ..About::default() };
    if let Ok(project) = Project::find() {
        let cargo = read_toml(&project, "Cargo.toml");
        about.app = package(&cargo, "name");
        about.app_version = package(&cargo, "version");
        about.ocre = ocre_dependency(&cargo).map(|(source, _)| source);
    }
    Ok(Report { about: Some(about), ..Report::new("version") })
}

/// `ocre about`: versions and the app's configuration.
pub fn about() -> CliResult {
    let project = Project::find()?;
    let cargo = read_toml(&project, "Cargo.toml");
    let wrangler = read_toml(&project, "wrangler.toml");
    let toolchain = read_toml(&project, "rust-toolchain.toml");
    let (ocre, features) = ocre_dependency(&cargo).unzip();
    let about = About {
        cli: CLI_VERSION,
        app: package(&cargo, "name"),
        app_version: package(&cargo, "version"),
        ocre,
        mode: Some(if project.api_only { "api (JSON only)" } else { "full-stack (HTML and JSON)" }),
        rust_toolchain: toolchain.get("toolchain").and_then(|t| t.get("channel")).and_then(str_value),
        compatibility_date: wrangler.get("compatibility_date").and_then(str_value),
        bindings: bindings(&wrangler),
        vars: wrangler.get("vars").and_then(|v| v.as_table()).map(|v| v.keys().cloned().collect()).unwrap_or_default(),
        features: features.unwrap_or_default(),
    };
    Ok(Report { about: Some(about), ..Report::new("about") })
}

/// A missing or invalid file reads as empty: `about` reports what it can.
fn read_toml(project: &Project, file: &str) -> toml::Table {
    std::fs::read_to_string(project.root.join(file)).ok().and_then(|text| text.parse().ok()).unwrap_or_default()
}

fn str_value(value: &toml::Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

fn package(cargo: &toml::Table, key: &str) -> Option<String> {
    cargo.get("package").and_then(|p| p.get(key)).and_then(str_value)
}

/// (`git https://... branch main`, `path ../ocre` or `0.1`, features on).
fn ocre_dependency(cargo: &toml::Table) -> Option<(String, Vec<String>)> {
    let dependency = cargo.get("dependencies")?.get("ocre")?;
    if let Some(version) = dependency.as_str() {
        return Some((version.to_owned(), Vec::new()));
    }
    let mut source: Vec<String> = Vec::new();
    for key in ["version", "git", "branch", "tag", "rev", "path"] {
        if let Some(value) = dependency.get(key).and_then(str_value) {
            source.push(if key == "version" { value } else { format!("{key} {value}") });
        }
    }
    let features = dependency
        .get("features")
        .and_then(|f| f.as_array())
        .map(|f| f.iter().filter_map(str_value).collect())
        .unwrap_or_default();
    Some((source.join(" "), features))
}

/// Bindings and triggers of wrangler.toml, in a fixed order.
fn bindings(wrangler: &toml::Table) -> Vec<String> {
    let entries = |key: &str| wrangler.get(key).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let field = |entry: &toml::Value, key: &str| entry.get(key).and_then(str_value).unwrap_or_default();
    let mut out = Vec::new();
    for db in entries("d1_databases") {
        out.push(format!("D1 {} ({})", field(&db, "binding"), field(&db, "database_name")));
    }
    for kv in entries("kv_namespaces") {
        out.push(format!("KV {}", field(&kv, "binding")));
    }
    for bucket in entries("r2_buckets") {
        out.push(format!("R2 {} ({})", field(&bucket, "binding"), field(&bucket, "bucket_name")));
    }
    let queues = wrangler.get("queues");
    for producer in queues.and_then(|q| q.get("producers")).and_then(|p| p.as_array()).into_iter().flatten() {
        out.push(format!("Queue {} ({})", field(producer, "binding"), field(producer, "queue")));
    }
    for consumer in queues.and_then(|q| q.get("consumers")).and_then(|c| c.as_array()).into_iter().flatten() {
        out.push(format!("Queue consumer ({})", field(consumer, "queue")));
    }
    let objects = wrangler.get("durable_objects").and_then(|d| d.get("bindings")).and_then(|b| b.as_array());
    for object in objects.into_iter().flatten() {
        out.push(format!("Durable Object {} ({})", field(object, "name"), field(object, "class_name")));
    }
    for email in entries("send_email") {
        out.push(format!("Email {}", field(&email, "name")));
    }
    let crons = wrangler.get("triggers").and_then(|t| t.get("crons")).and_then(|c| c.as_array());
    for cron in crons.into_iter().flatten().filter_map(str_value) {
        out.push(format!("cron {cron}"));
    }
    let assets = wrangler.get("assets").map(|a| a.get("directory").and_then(str_value).unwrap_or_default());
    out.extend(assets.map(|directory| format!("Assets ({directory})")));
    out
}

//! `ocre generate cache`: the `CACHE` Workers KV binding in wrangler.toml,
//! used by `ocre::cache::{fetch, read, write, delete}`. `ocre dev` gives it a
//! local namespace; `ocre deploy` creates the real one and writes its id.

use super::Edits;
use crate::{CliResult, output::CliError, project::Project};

/// Binding name `ocre::cache` reads.
pub const CACHE_BINDING: &str = "CACHE";

const KV_BLOCK: &str = r#"
# Cached values for `ocre::cache` (Workers KV), added by `ocre g cache`. The
# first `ocre deploy` creates the namespace and writes its id here; `ocre dev`
# uses a local one. Free plan: 100,000 reads and 1,000 writes a day, 1 GB.
[[kv_namespaces]]
binding = "CACHE"
"#;

pub fn cache(project: &Project) -> CliResult {
    let mut edits = Edits::new(project);
    let wrangler = edits.read("wrangler.toml")?.unwrap_or_default();
    let config: toml::Table = wrangler.parse().expect("Project::find parsed wrangler.toml");
    let bound = config.get("kv_namespaces").and_then(|namespaces| namespaces.as_array()).is_some_and(|namespaces| {
        namespaces.iter().any(|ns| ns.get("binding").and_then(|b| b.as_str()) == Some(CACHE_BINDING))
    });
    if bound {
        return Err(CliError::new(format!("wrangler.toml already has the `{CACHE_BINDING}` KV binding"))
            .hint("nothing to generate: call `ocre::cache::fetch(&ctx, key, ttl, || async { ... })` in a handler"));
    }
    edits.update("wrangler.toml", format!("{}\n{}", wrangler.trim_end(), KV_BLOCK));
    let mut report = edits.apply("generate cache")?;
    report.next = vec![
        "use it: ocre::cache::fetch(&ctx, \"key:v1\", Duration::from_secs(3600), || async { ... }).await?".to_owned(),
        "ocre dev".to_owned(),
        "ocre deploy (creates the KV namespace)".to_owned(),
    ];
    Ok(report)
}

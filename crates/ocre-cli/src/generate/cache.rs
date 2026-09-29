//! `ocre generate cache`: the `CACHE` Workers KV binding in cloudflare.config.ts,
//! used by `ocre::cache::{fetch, read, write, delete}`. `ocre dev` gives it a
//! local namespace; `ocre deploy` creates the real one and writes its id.

use super::{Edits, read_config};
use crate::{
    CliResult,
    config::{self, ENV_MARKER},
    output::CliError,
    project::Project,
};

/// Binding name `ocre::cache` reads.
pub const CACHE_BINDING: &str = "CACHE";

const KV_ENTRY: &str = "// Cached values for `ocre::cache` (Workers KV), added by `ocre g cache`. The
// first `ocre deploy` creates the namespace and writes its id here; `ocre dev`
// uses a local one. Free plan: 100,000 reads and 1,000 writes a day, 1 GB.
CACHE: bindings.kv(),";

pub fn cache(project: &Project) -> CliResult {
    let mut edits = Edits::new(project);
    let config = read_config(&edits)?;
    if config.binding(CACHE_BINDING).is_some() {
        return Err(CliError::new(format!("{} already has the `{CACHE_BINDING}` binding", config::FILE))
            .hint("nothing to generate: call `ocre::cache::fetch(&ctx, key, ttl, || async { ... })` in a handler"));
    }
    edits.update(config::FILE, config.insert(ENV_MARKER, KV_ENTRY)?);
    let mut report = edits.apply("generate cache")?;
    report.next = vec![
        "use it: ocre::cache::fetch(&ctx, \"key:v1\", Duration::from_secs(3600), || async { ... }).await?".to_owned(),
        "ocre dev".to_owned(),
        "ocre deploy (creates the KV namespace)".to_owned(),
    ];
    Ok(report)
}

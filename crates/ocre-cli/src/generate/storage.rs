//! The R2 bucket behind `ocre::storage`: added to wrangler.toml by the first
//! generator that needs it (a model with an `attachment` field).

use super::Edits;
use crate::output::CliError;

/// Binding name `ocre::storage` reads (`ocre::storage::STORAGE_BINDING`).
pub(crate) const STORAGE_BINDING: &str = "STORAGE";

/// Adds the `STORAGE` `[[r2_buckets]]` entry to wrangler.toml unless it is
/// there. The bucket is named after the Worker: `<name>-storage`.
pub(crate) fn ensure_bucket(edits: &mut Edits) -> Result<(), CliError> {
    let text = edits.read("wrangler.toml")?.unwrap_or_default();
    let config: toml::Table = text.parse().expect("Project::find parsed wrangler.toml");
    let bound = config
        .get("r2_buckets")
        .and_then(|buckets| buckets.as_array())
        .into_iter()
        .flatten()
        .any(|bucket| bucket.get("binding").and_then(|b| b.as_str()) == Some(STORAGE_BINDING));
    if bound {
        return Ok(());
    }
    let app = config.get("name").and_then(|name| name.as_str()).unwrap_or("app");
    let block = format!(
        "\n# Files (`ocre::storage`, `attachment` fields): an R2 bucket. `ocre dev` keeps a\n# local copy under .wrangler/state; `ocre deploy` creates the bucket if needed.\n# Free plan: 10 GB stored, 1M writes and 10M reads a month; deletes are free.\n[[r2_buckets]]\nbinding = \"{STORAGE_BINDING}\"\nbucket_name = \"{}\"\n",
        bucket_name(app)
    );
    let separator = if text.ends_with('\n') || text.is_empty() { "" } else { "\n" };
    edits.update("wrangler.toml", format!("{text}{separator}{block}"));
    Ok(())
}

/// `<app>-storage`, within R2's 63 characters (app names are already
/// lowercase letters, digits and dashes).
pub(crate) fn bucket_name(app: &str) -> String {
    let base: String = app.chars().take(63 - "-storage".len()).collect();
    format!("{}-storage", base.trim_end_matches('-'))
}

#[cfg(test)]
#[path = "../../tests/generate/storage.rs"]
mod tests;

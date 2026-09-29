//! The R2 bucket behind `ocre::storage`: added to cloudflare.config.ts by the
//! first generator that needs it (a model with an `attachment` field).

use super::{Edits, read_config};
use crate::{
    config::{self, ENV_MARKER},
    output::CliError,
};

/// Binding name `ocre::storage` reads (`ocre::storage::STORAGE_BINDING`).
pub(crate) const STORAGE_BINDING: &str = "STORAGE";

/// Adds the `STORAGE` R2 binding to cloudflare.config.ts unless it is there.
/// The bucket is named after the Worker: `<name>-storage`.
pub(crate) fn ensure_bucket(edits: &mut Edits) -> Result<(), CliError> {
    let config = read_config(edits)?;
    if config.binding(STORAGE_BINDING).is_some() {
        return Ok(());
    }
    let entry = format!(
        "// Files (`ocre::storage`, `attachment` fields): an R2 bucket. `ocre dev` keeps a\n// local copy under .wrangler/state; `ocre deploy` creates the bucket if needed.\n// Free plan: 10 GB stored, 1M writes and 10M reads a month; deletes are free.\n{STORAGE_BINDING}: bindings.r2({{ name: \"{}\" }}),",
        bucket_name(config.worker_name()?)
    );
    edits.update(config::FILE, config.insert(ENV_MARKER, &entry)?);
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

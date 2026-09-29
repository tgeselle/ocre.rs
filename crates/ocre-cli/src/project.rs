//! Locating and reading the current Ocre app.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::output::CliError;

/// Binding name every Ocre app uses for its D1 database.
const DB_BINDING: &str = "DB";

pub struct Project {
    pub root: PathBuf,
    /// `database_name` of the `DB` binding in wrangler.toml.
    pub database_name: String,
}

impl Project {
    /// Finds the app root (nearest directory with wrangler.toml) from the cwd.
    pub fn find() -> Result<Self, CliError> {
        let cwd = std::env::current_dir()?;
        let root = cwd
            .ancestors()
            .find(|dir| dir.join("wrangler.toml").is_file())
            .ok_or_else(|| {
                CliError::new("no wrangler.toml found in this directory or its parents")
                    .hint("run this command inside an Ocre app, or create one with `ocre new <name>`")
            })?
            .to_path_buf();
        Self::at(root)
    }

    /// The app whose wrangler.toml is in `root`.
    pub fn at(root: PathBuf) -> Result<Self, CliError> {
        let database_name = read_database_name(&root.join("wrangler.toml"))?;
        Ok(Self { root, database_name })
    }

    /// Path relative to the app root, for reports.
    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).display().to_string()
    }
}

fn read_database_name(path: &Path) -> Result<String, CliError> {
    let text = std::fs::read_to_string(path)?;
    let config: toml::Table =
        text.parse().map_err(|err| CliError::new(format!("wrangler.toml is not valid TOML: {err}")))?;
    config
        .get("d1_databases")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .find(|db| db.get("binding").and_then(|b| b.as_str()) == Some(DB_BINDING))
        .and_then(|db| db.get("database_name").and_then(|n| n.as_str()))
        .map(str::to_owned)
        .ok_or_else(|| {
            CliError::new(format!("wrangler.toml has no D1 database with binding \"{DB_BINDING}\""))
                .hint(format!(
                    "add:\n[[d1_databases]]\nbinding = \"{DB_BINDING}\"\ndatabase_name = \"<app-name>\"\nmigrations_dir = \"migrations\""
                ))
        })
}

/// Fails early when rustc cannot target wasm32, which otherwise surfaces as a
/// long worker-build error.
pub fn check_wasm_target() -> Result<(), CliError> {
    let output = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .map_err(|_| CliError::new("rustc not found").hint("install Rust with rustup: https://rustup.rs"))?;
    let sysroot = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if Path::new(&sysroot).join("lib/rustlib/wasm32-unknown-unknown").is_dir() {
        return Ok(());
    }
    Err(CliError::new(format!("the wasm32-unknown-unknown target is not installed for rustc at {sysroot}"))
        .hint("use a rustup toolchain (Homebrew's `rust` has no wasm target) and run `rustup target add wasm32-unknown-unknown`"))
}

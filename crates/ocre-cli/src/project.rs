//! Locating and reading the current Ocre app.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{config::Config, output::CliError};

pub struct Project {
    pub root: PathBuf,
    /// `name` of the `DB` D1 binding in cloudflare.config.ts.
    pub database_name: String,
    /// `[package.metadata.ocre] mode = "api"` in Cargo.toml: JSON only, no HTML.
    pub api_only: bool,
    /// Flags of the running `ocre generate` (defaults for other commands).
    pub generate: crate::generate::GenerateOptions,
}

impl Project {
    /// Finds the app root (nearest directory with cloudflare.config.ts) from the cwd.
    pub fn find() -> Result<Self, CliError> {
        let cwd = std::env::current_dir()?;
        let root = cwd
            .ancestors()
            .find(|dir| dir.join(crate::config::FILE).is_file() || dir.join("wrangler.toml").is_file())
            .ok_or_else(|| {
                CliError::new(format!("no {} found in this directory or its parents", crate::config::FILE))
                    .hint("run this command inside an Ocre app, or create one with `ocre new <name>`")
            })?
            .to_path_buf();
        if !root.join(crate::config::FILE).is_file() {
            return Err(CliError::new(format!(
                "{} uses wrangler.toml; Ocre now reads {}",
                root.display(),
                crate::config::FILE
            ))
            .hint(format!(
                "convert it: `npm install --save-dev --save-exact cf@{} wrangler@{}`, then `npx cf migrate --no-install`, \
                 then apply the Ocre fixes of the upgrading guide ({}/guides/upgrading.html)",
                crate::cloudflare::CF_VERSION,
                crate::cloudflare::WRANGLER_VERSION,
                crate::new::DOCS_URL
            )));
        }
        Self::at(root)
    }

    /// The app whose cloudflare.config.ts is in `root`.
    pub fn at(root: PathBuf) -> Result<Self, CliError> {
        let database_name = Config::read(&root)?.database_name()?;
        let api_only = read_api_mode(&root.join("Cargo.toml"));
        Ok(Self { root, database_name, api_only, generate: Default::default() })
    }

    /// The app's cloudflare.config.ts, read now.
    pub fn config(&self) -> Result<Config, CliError> {
        Config::read(&self.root)
    }
}

/// A missing or unreadable Cargo.toml means a full-stack app; cargo reports
/// its own errors when building.
fn read_api_mode(path: &Path) -> bool {
    let Some(manifest) = std::fs::read_to_string(path).ok().and_then(|text| text.parse::<toml::Table>().ok()) else {
        return false;
    };
    manifest
        .get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("ocre"))
        .and_then(|o| o.get("mode"))
        .and_then(|mode| mode.as_str())
        == Some("api")
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
    let error = CliError::new(format!("the wasm32-unknown-unknown target is not installed for rustc at {sysroot}"));
    // rustup installed, but another Rust (Homebrew's) comes first in PATH: a plain
    // `cargo check` in a terminal fails the same way, with "can't find crate for `core`".
    let rustup = Command::new("rustup").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status();
    if !sysroot.contains("/.rustup/") && rustup.is_ok_and(|status| status.success()) {
        return Err(error.hint(
            "this rustc is not rustup's but comes first in PATH (Homebrew's `rust` has no wasm target): put rustup's \
             first (`export PATH=\"$HOME/.cargo/bin:$PATH\"` in your shell profile) or `brew uninstall rust`, then \
             `rustup target add wasm32-unknown-unknown`; `which cargo` must point to rustup's",
        ));
    }
    Err(error.hint("use a rustup toolchain (Homebrew's `rust` has no wasm target) and run `rustup target add wasm32-unknown-unknown`"))
}

//! `ocre new <name>`: app skeleton.

use std::path::Path;

use crate::{CliResult, output::CliError, output::Report};

const OCRE_GIT: &str = "https://github.com/tgeselle/ocre.rs";

/// (path in the app, template contents)
const FILES: &[(&str, &str)] = &[
    ("Cargo.toml", include_str!("../templates/new/Cargo.toml.tmpl")),
    ("wrangler.toml", include_str!("../templates/new/wrangler.toml")),
    ("rust-toolchain.toml", include_str!("../templates/new/rust-toolchain.toml")),
    (".gitignore", include_str!("../templates/new/gitignore")),
    ("AGENTS.md", include_str!("../templates/new/AGENTS.md")),
    ("src/lib.rs", include_str!("../templates/new/lib.rs")),
    ("templates/layout.html", include_str!("../templates/new/layout.html")),
    ("templates/home.html", include_str!("../templates/new/home.html")),
    ("migrations/.gitkeep", ""),
];

pub fn run(name: &str, ocre_path: Option<&Path>) -> CliResult {
    validate_app_name(name)?;
    let root = std::env::current_dir()?.join(name);
    if root.exists() {
        return Err(CliError::new(format!("`{}` already exists", root.display()))
            .hint("choose another name or remove the directory"));
    }
    let ocre_dep = match ocre_path {
        Some(path) => {
            let path = path.canonicalize().map_err(|err| {
                CliError::new(format!("--ocre-path {}: {err}", path.display()))
                    .hint("pass the directory of the `ocre` crate (crates/ocre in the Ocre repository)")
            })?;
            format!("ocre = {{ path = {:?} }}", path.display().to_string())
        }
        None => format!("ocre = {{ git = \"{OCRE_GIT}\" }}"),
    };

    let mut report = Report::new("new");
    for (relative, template) in FILES {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("file paths have a parent"))?;
        let contents = template.replace("__APP_NAME__", name).replace("__OCRE_DEP__", &ocre_dep);
        std::fs::write(&path, contents)?;
        report.created.push(format!("{name}/{relative}"));
    }
    report.next = vec![
        format!("cd {name}"),
        "ocre g scaffold Post title:string body:text".to_owned(),
        "ocre dev".to_owned(),
    ];
    Ok(report)
}

/// Worker names: lowercase letters, digits and dashes, at most 63 characters.
fn validate_app_name(name: &str) -> Result<(), CliError> {
    let valid = name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(CliError::new(format!("invalid app name `{name}`"))
            .hint("use lowercase letters, digits and dashes, starting with a letter (max 63), e.g. `my-blog`"))
    }
}

//! `ocre new`: app skeleton, optional starter, git, Cloudflare login and deploy.
//!
//! In a terminal (and without `--json` or `--yes`) the wizard asks for
//! whatever the flags did not answer. Otherwise flags alone decide, with
//! opt-in defaults, so agents never get a prompt.

use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    CliResult, generate,
    output::{CliError, Report},
    project::{Project, check_wasm_target},
    wrangler::{Echo, Wrangler, pick_account},
};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Starter {
    /// Home page only.
    Empty,
    /// A `Post` resource (title, body, published) with CRUD pages.
    Blog,
}

/// `ocre new` flags. `None` means "not given": the wizard asks, flag mode
/// uses the default.
pub struct NewArgs {
    pub name: Option<String>,
    pub ocre_path: Option<PathBuf>,
    pub starter: Option<Starter>,
    pub account_id: Option<String>,
    pub git: Option<bool>,
    pub login: Option<bool>,
    pub deploy: Option<bool>,
    pub yes: bool,
}

pub fn run(args: NewArgs, json: bool) -> CliResult {
    let cwd = std::env::current_dir()?;
    let interactive = !json && !args.yes && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if interactive { crate::wizard::new_app(args, &cwd) } else { run_with_flags(args, &cwd, json) }
}

fn run_with_flags(args: NewArgs, cwd: &Path, json: bool) -> CliResult {
    let name = args.name.ok_or_else(|| {
        CliError::new("missing app name")
            .hint("run `ocre new <name>`, or run `ocre new` in a terminal for the guided setup")
    })?;
    let deploy = args.deploy.unwrap_or(false);
    let starter = args.starter.unwrap_or(Starter::Empty);
    let mut plan =
        Plan::new(cwd, &name, args.ocre_path.as_deref(), starter, args.git.unwrap_or(false), args.account_id)?;
    let echo = Echo::for_json(json);
    // Deploying needs a session, so `--deploy` implies `--login`.
    let session =
        if args.login.unwrap_or(false) || deploy { Some(Wrangler::new(cwd, echo).ensure_login()?) } else { None };
    if let Some(session) = &session {
        plan.account_id = pick_account(session, plan.account_id.as_deref())?;
    }
    let mut report = plan.create()?;
    if deploy {
        report.url = plan.deploy(echo)?;
        report.next.retain(|step| step != "ocre deploy");
    }
    report.email = session.and_then(|s| s.email);
    Ok(report)
}

/// A validated app to create.
pub struct Plan {
    pub name: String,
    pub root: PathBuf,
    ocre_dep: String,
    pub starter: Starter,
    pub git: bool,
    pub account_id: Option<String>,
}

impl Plan {
    /// Checks everything that could fail before a single file is written.
    pub fn new(
        cwd: &Path,
        name: &str,
        ocre_path: Option<&Path>,
        starter: Starter,
        git: bool,
        account_id: Option<String>,
    ) -> Result<Self, CliError> {
        check_new_app(cwd, name)?;
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
        if git && !git_available() {
            return Err(CliError::new("git is not installed").hint("install git, or create the app without `--git`"));
        }
        Ok(Self { name: name.to_owned(), root: cwd.join(name), ocre_dep, starter, git, account_id })
    }

    pub fn create(&self) -> CliResult {
        let name = &self.name;
        let mut report = Report::new("new");
        for (relative, template) in FILES {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().expect("file paths have a parent"))?;
            let mut contents = template.replace("__APP_NAME__", name).replace("__OCRE_DEP__", &self.ocre_dep);
            if let (&"wrangler.toml", Some(id)) = (relative, &self.account_id) {
                contents = with_account_id(&contents, id);
            }
            std::fs::write(&path, contents)?;
            report.created.push(format!("{name}/{relative}"));
        }
        if self.starter == Starter::Blog {
            let fields = ["title:string", "body:text", "published:boolean"].map(String::from);
            let scaffold = generate::scaffold(&Project::at(self.root.clone())?, "Post", &fields)?;
            report.created.extend(scaffold.created.into_iter().map(|path| format!("{name}/{path}")));
        }
        if self.git {
            let status = Command::new("git").args(["init", "--quiet"]).current_dir(&self.root).status()?;
            if !status.success() {
                return Err(CliError::new(format!("`git init` failed ({status})")));
            }
        }
        report.next = vec![format!("cd {name}"), "ocre dev".to_owned(), "ocre deploy".to_owned()];
        Ok(report)
    }

    /// Builds and deploys the new app; returns its workers.dev URL.
    pub fn deploy(&self, echo: Echo) -> Result<Option<String>, CliError> {
        check_wasm_target()?;
        Wrangler::new(&self.root, echo).deploy(&self.name)
    }
}

/// App name rules plus "the directory is free". Shared with the wizard.
pub fn check_new_app(cwd: &Path, name: &str) -> Result<(), CliError> {
    validate_app_name(name)?;
    let root = cwd.join(name);
    if root.exists() {
        return Err(CliError::new(format!("`{}` already exists", root.display()))
            .hint("choose another name or remove the directory"));
    }
    Ok(())
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

/// Adds `account_id` right after the `name` line of wrangler.toml.
fn with_account_id(wrangler_toml: &str, account_id: &str) -> String {
    let (first, rest) = wrangler_toml.split_once('\n').expect("template has several lines");
    format!("{first}\naccount_id = \"{account_id}\"\n{rest}")
}

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|out| out.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_follow_worker_rules() {
        for ok in ["a", "my-blog", "app2", &"a".repeat(63)] {
            assert!(validate_app_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "My-app", "2app", "-app", "app-", "my_app", "my app", &"a".repeat(64)] {
            assert!(validate_app_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn account_id_goes_after_the_name_line() {
        let toml = with_account_id("name = \"x\"\nmain = \"y\"\n", "abc");
        assert_eq!(toml, "name = \"x\"\naccount_id = \"abc\"\nmain = \"y\"\n");
        let parsed: toml::Table = toml.parse().unwrap();
        assert_eq!(parsed["account_id"].as_str(), Some("abc"));
    }
}

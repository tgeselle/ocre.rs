//! `ocre new`: app skeleton, optional starter, git, Cloudflare login and deploy.
//!
//! In a terminal (and without `--json` or `--yes`) the wizard asks for
//! whatever the flags did not answer. Otherwise flags alone decide, with
//! opt-in defaults, so agents never get a prompt.

use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{
    CliResult,
    cloudflare::{
        CF_VERSION, Cloudflare, Deployed, Echo, NODE_MIN, TYPESCRIPT_VERSION, WRANGLER_VERSION, pick_account,
    },
    config, generate,
    output::{CliError, Report},
    project::{Project, check_wasm_target},
    secret::{self, SECRET_KEY_BASE},
    template::Template,
};

const OCRE_GIT: &str = "https://github.com/tgeselle/ocre.rs";
/// Documentation site, linked from the app's AGENTS.md (`__DOCS_URL__`). Also
/// `DEFAULT_BASE_URL` in docs/tool/src/main.rs: change both together.
pub(crate) const DOCS_URL: &str = "https://ocre-docs.raitomm.workers.dev";

/// (path in the app, template contents), shared by both app kinds.
const FILES: &[(&str, &str)] = &[
    ("Cargo.toml", include_str!("../templates/new/Cargo.toml.tmpl")),
    (config::FILE, include_str!("../templates/new/cloudflare.config.ts")),
    (config::BUILD_FILE, include_str!("../templates/new/wrangler.config.ts")),
    ("package.json", include_str!("../templates/new/package.json")),
    ("tsconfig.json", include_str!("../templates/new/tsconfig.json")),
    ("rust-toolchain.toml", include_str!("../templates/new/rust-toolchain.toml")),
    (".gitignore", include_str!("../templates/new/gitignore")),
    ("AGENTS.md", include_str!("../templates/new/AGENTS.md")),
    ("migrations/.gitkeep", ""),
    ("public/robots.txt", include_str!("../templates/new/robots.txt")),
    ("tests/app.rs", include_str!("../templates/new/app_test.rs")),
];

/// Full-stack apps: HTML pages.
const HTML_FILES: &[(&str, &str)] = &[
    ("src/lib.rs", include_str!("../templates/new/lib.rs")),
    ("templates/layout.html", include_str!("../templates/new/layout.html")),
    ("templates/home.html", include_str!("../templates/new/home.html")),
    ("templates/error.html", include_str!("../templates/new/error.html")),
];

/// API-only apps: JSON, no templates, no askama.
const API_FILES: &[(&str, &str)] = &[("src/lib.rs", include_str!("../templates/new/lib_api.rs"))];

const ASKAMA_DEP: &str = "askama = \"0.16.1\"\n";
const API_METADATA: &str = "\n[package.metadata.ocre]\n# JSON only: `ocre g scaffold` generates APIs, Ocre's `html` feature is off.\nmode = \"api\"\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Starter {
    /// Home page (or status endpoint in API mode) only.
    Empty,
    /// A `Post` resource (title, body, published) with CRUD pages or a JSON API.
    Blog,
}

/// `ocre new` flags. `None` means "not given": the wizard asks, flag mode
/// uses the default.
pub struct NewArgs {
    pub name: Option<String>,
    pub ocre_path: Option<PathBuf>,
    pub api: Option<bool>,
    pub starter: Option<Starter>,
    pub account_id: Option<String>,
    pub git: Option<bool>,
    pub login: Option<bool>,
    pub deploy: Option<bool>,
    pub yes: bool,
    /// Run `npm install` in the new app (`--no-install` turns it off).
    pub install: bool,
    /// `--template`: path or URL of an application template.
    pub template: Option<String>,
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
    if deploy && !args.install {
        return Err(CliError::new("--deploy needs the app's npm packages")
            .hint("drop --no-install, or deploy later with `npm install && ocre deploy`"));
    }
    let starter = args.starter.unwrap_or(Starter::Empty);
    let mut plan = Plan::new(
        cwd,
        &name,
        args.ocre_path.as_deref(),
        args.api.unwrap_or(false),
        starter,
        args.git.unwrap_or(false),
        args.account_id,
    )?;
    plan.template = args.template.map(|source| Template::load(&source, cwd)).transpose()?;
    plan.install = args.install;
    let echo = Echo::for_json(json);
    // Deploying needs a session, so `--deploy` implies `--login`.
    let session =
        if args.login.unwrap_or(false) || deploy { Some(Cloudflare::new(cwd, echo).ensure_login()?) } else { None };
    if let Some(session) = &session {
        plan.account_id = pick_account(session, plan.account_id.as_deref())?;
    }
    let mut report = plan.create()?;
    if deploy {
        let deployed = plan.deploy(echo)?;
        report.url = deployed.url;
        report.secret_created = deployed.secret_created;
        report.provisioned = deployed.provisioned;
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
    pub api: bool,
    pub starter: Starter,
    pub git: bool,
    pub account_id: Option<String>,
    /// Applied right after the app and its starter are created.
    pub template: Option<Template>,
    /// Run `npm install` once the files are written.
    pub install: bool,
}

impl Plan {
    /// Checks everything that could fail before a single file is written.
    pub fn new(
        cwd: &Path,
        name: &str,
        ocre_path: Option<&Path>,
        api: bool,
        starter: Starter,
        git: bool,
        account_id: Option<String>,
    ) -> Result<Self, CliError> {
        check_new_app(cwd, name)?;
        let source = match ocre_path {
            Some(path) => {
                let path = path.canonicalize().map_err(|err| {
                    CliError::new(format!("--ocre-path {}: {err}", path.display()))
                        .hint("pass the directory of the `ocre` crate (crates/ocre in the Ocre repository)")
                })?;
                format!("path = {:?}", path.display().to_string())
            }
            None => format!("git = \"{OCRE_GIT}\""),
        };
        let ocre_dep = if api {
            format!("ocre = {{ {source}, default-features = false }}")
        } else {
            format!("ocre = {{ {source} }}")
        };
        if git && !git_available() {
            return Err(CliError::new("git is not installed").hint("install git, or create the app without `--git`"));
        }
        Ok(Self {
            name: name.to_owned(),
            root: cwd.join(name),
            ocre_dep,
            api,
            starter,
            git,
            account_id,
            template: None,
            install: true,
        })
    }

    pub fn create(&self) -> CliResult {
        let name = &self.name;
        if self.install && !npm_available() {
            return Err(CliError::new("npm not found").hint(format!(
                "install Node.js {NODE_MIN} or newer (it provides npm), or pass --no-install and run `npm install` in the app later"
            )));
        }
        let mut report = Report::new("new");
        let kind_files = if self.api { API_FILES } else { HTML_FILES };
        let test_dep = test_dependency(&self.ocre_dep);
        for (relative, template) in FILES.iter().chain(kind_files) {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().expect("file paths have a parent"))?;
            let mut contents = template
                .replace("__APP_NAME__", name)
                .replace("__OCRE_DEP__", &self.ocre_dep)
                .replace("__OCRE_TEST_DEP__", &test_dep)
                .replace("__DOCS_URL__", DOCS_URL)
                .replace("__CF_VERSION__", CF_VERSION)
                .replace("__WRANGLER_VERSION__", WRANGLER_VERSION)
                .replace("__TYPESCRIPT_VERSION__", TYPESCRIPT_VERSION);
            match (*relative, &self.account_id) {
                (config::FILE, Some(id)) => contents = config::with_account_id(&contents, id),
                ("Cargo.toml", _) if self.api => contents = contents.replace(ASKAMA_DEP, "") + API_METADATA,
                _ => {}
            }
            std::fs::write(&path, contents)?;
            report.created.push(format!("{name}/{relative}"));
        }
        // Local secrets and overrides for `ocre dev`, git-ignored. Deploys create
        // the production secret; MAIL_ADAPTER=log makes `ocre dev` print email instead of sending it.
        let dev_vars = format!("{SECRET_KEY_BASE}={}\nMAIL_ADAPTER=log\n", secret::generate());
        std::fs::write(self.root.join(".dev.vars"), dev_vars)?;
        report.created.push(format!("{name}/.dev.vars"));
        if self.starter == Starter::Blog {
            let fields = ["title:string", "body:text", "published:boolean"].map(String::from);
            let mut project = Project::at(self.root.clone())?;
            let generator = if self.api { "api" } else { "scaffold" };
            project.generate.invocation =
                ["g", generator, "Post"].into_iter().map(String::from).chain(fields.iter().cloned()).collect();
            let generated = if self.api {
                generate::api(&project, "Post", &fields, false)?
            } else {
                generate::scaffold(&project, "Post", &fields, false)?
            };
            report.created.extend(generated.created.into_iter().map(|path| format!("{name}/{path}")));
        }
        if let Some(template) = &self.template {
            let applied = template.apply(&self.root)?;
            report.created.extend(applied.created.into_iter().map(|path| format!("{name}/{path}")));
            report.ran.extend(applied.ran);
        }
        if self.install {
            npm_install(&self.root)?;
            report.ran.push(format!("npm install (cf {CF_VERSION}, wrangler {WRANGLER_VERSION})"));
        }
        if self.git {
            let status = Command::new("git").args(["init", "--quiet"]).current_dir(&self.root).status()?;
            if !status.success() {
                return Err(CliError::new(format!("`git init` failed ({status})")));
            }
        }
        report.next = vec![format!("cd {name}"), "ocre dev".to_owned(), "ocre deploy".to_owned()];
        if !self.install {
            report.next.insert(1, "npm install".to_owned());
        }
        Ok(report)
    }

    /// Builds and deploys the new app; returns its workers.dev URL.
    pub fn deploy(&self, echo: Echo) -> Result<Deployed, CliError> {
        check_wasm_target()?;
        Cloudflare::new(&self.root, echo).deploy(&Project::at(self.root.clone())?)
    }
}

/// The `[dev-dependencies]` line: Ocre again, with its `testing` feature.
fn test_dependency(ocre_dep: &str) -> String {
    format!("{}, features = [\"testing\"] }}", ocre_dep.trim_end_matches('}').trim_end())
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

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|out| out.status.success())
}

fn npm_available() -> bool {
    Command::new("npm").arg("--version").stdin(Stdio::null()).output().is_ok_and(|out| out.status.success())
}

/// Installs the app's pinned cf, wrangler and typescript (writes package-lock.json).
/// `npm` answered `npm --version` in [`Plan::create`] before any file was written.
fn npm_install(root: &Path) -> Result<(), CliError> {
    let output = Command::new("npm")
        .args(["install", "--no-audit", "--no-fund"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(CliError::new(format!(
        "`npm install` failed in {} ({}):\n{}",
        root.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim_end()
    ))
    .hint(format!(
        "the app is created: fix the cause above (network, Node.js {NODE_MIN}+), then run `npm install` in it"
    )))
}

#[cfg(test)]
#[path = "../tests/new.rs"]
mod tests;

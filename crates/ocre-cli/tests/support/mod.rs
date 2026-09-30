//! Test sandbox: a scratch directory, fake `cf`, `wrangler`, `npx`, `npm`,
//! `node` and `tsc` on PATH (tests/support/fake_cf.sh), and helpers to run
//! the real `ocre` binary against them.

#![allow(dead_code)] // each test binary uses a different subset

use std::{
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::Value;

pub const OCRE: &str = env!("CARGO_BIN_EXE_ocre");

/// Names the fake answers to on PATH; `npm install` links the others
/// (cf, wrangler, tsc) into the app's node_modules/.bin.
const FAKES: [&str; 3] = ["npx", "npm", "node"];

/// `wrangler <args>` of the local database fallback, as logged by the fake.
pub fn local_d1(args: &str) -> String {
    format!("wrangler {args} -c .wrangler/ocre-d1.json --persist-to .wrangler/state")
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

pub struct Sandbox {
    root: PathBuf,
    /// Working directory for commands.
    pub work: PathBuf,
    bin: PathBuf,
    state: PathBuf,
    path: OsString,
}

impl Sandbox {
    pub fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("sandbox-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (work, bin, state) = (root.join("work"), root.join("bin"), root.join("state"));
        for dir in [&work, &bin, &state] {
            fs::create_dir_all(dir).unwrap();
        }
        let sandbox = Self {
            path: std::env::join_paths(
                [bin.clone()].into_iter().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
            )
            .unwrap(),
            root,
            work,
            bin,
            state,
        };
        for name in FAKES {
            sandbox.script(name, include_str!("fake_cf.sh"));
        }
        sandbox
    }

    /// Installs an executable script in the sandbox bin directory (first on PATH).
    pub fn script(&self, name: &str, body: &str) {
        let path = self.bin.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Takes a fake off PATH (e.g. `node`, to test its absence).
    pub fn remove_tool(&self, name: &str) {
        fs::remove_file(self.bin.join(name)).unwrap();
    }

    /// PATH is only the sandbox bin directory: no git, rustc, real node...
    pub fn isolate_path(&mut self) {
        self.path = self.bin.clone().into_os_string();
    }

    /// Removes the fakes so the real npm, node, cf and wrangler run.
    pub fn use_real_cloudflare(&self) {
        for name in FAKES {
            self.remove_tool(name);
        }
    }

    pub fn write_state(&self, name: &str, contents: &str) {
        fs::write(self.state.join(name), contents).unwrap();
    }

    /// JSON outputs of the next D1 queries, one per call, in order.
    pub fn queue_execute(&self, outputs: &[&str]) {
        let dir = self.state.join("execute_queue");
        fs::create_dir_all(&dir).unwrap();
        for (index, output) in outputs.iter().enumerate() {
            fs::write(dir.join(format!("{index:03}.json")), output).unwrap();
        }
    }

    /// Makes the fakes behave differently (see fake_cf.sh).
    pub fn set(&self, marker: &str) {
        self.write_state(marker, "");
    }

    /// The remote D1 database `name` exists (id `uuid-<name>`).
    pub fn remote_database(&self, name: &str) {
        self.set(&format!("d1db_{name}"));
    }

    pub fn login_as(&self, accounts: &[(&str, &str)]) {
        let accounts: Vec<Value> = accounts
            .iter()
            .map(|(id, name)| serde_json::json!({ "id": id, "name": name, "type": "standard" }))
            .collect();
        let whoami = serde_json::json!({
            "authenticated": true,
            "authSource": "oauth",
            "tokenValid": true,
            "email": "dev@example.com",
            "accounts": accounts,
        });
        self.write_state("whoami.json", &whoami.to_string());
        self.set("logged_in");
    }

    pub fn accounts_after_login(&self, accounts: &[(&str, &str)]) {
        self.login_as(accounts);
        fs::remove_file(self.state.join("logged_in")).unwrap();
    }

    /// cf, wrangler and npm invocations so far, e.g. `["cf auth whoami", "cf auth login"]`.
    pub fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.state.join("calls.log")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    /// Forgets the calls so far.
    pub fn clear_calls(&self) {
        let _ = fs::remove_file(self.state.join("calls.log"));
    }

    pub fn command(&self, args: &[&str], cwd: &Path) -> Command {
        let mut command = Command::new(OCRE);
        command
            .args(args)
            .current_dir(cwd)
            .env("PATH", &self.path)
            .env("FAKE_CF_STATE", &self.state)
            .env("FAKE_CF_FIXTURES", Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/cf_fixtures"));
        command
    }

    pub fn ocre(&self, args: &[&str], cwd: &Path) -> Output {
        self.command(args, cwd).output().unwrap()
    }

    /// Runs with `--json` and returns (parsed stdout, success).
    pub fn json(&self, args: &[&str], cwd: &Path) -> (Value, bool) {
        // Before a `--`: what follows it goes to another program (`ocre test -- <cargo args>`).
        let mut all = args.to_vec();
        let at = all.iter().position(|arg| *arg == "--").unwrap_or(all.len());
        all.insert(at, "--json");
        let output = self.ocre(&all, cwd);
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value = serde_json::from_str(&stdout)
            .unwrap_or_else(|err| panic!("stdout is not one JSON object ({err}): {stdout}"));
        (value, output.status.success())
    }

    /// Creates `app` in the work dir with the local ocre crate (and the fake
    /// npm packages), returns its root. The calls it made are forgotten.
    pub fn new_app(&self, name: &str, extra: &[&str]) -> PathBuf {
        let ocre = ocre_crate();
        let mut args = vec!["new", name, "--ocre-path", ocre.to_str().unwrap()];
        args.extend_from_slice(extra);
        let (report, ok) = self.json(&args, &self.work);
        assert!(ok, "{report}");
        self.clear_calls();
        self.work.join(name)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// `crates/ocre`, for `--ocre-path`.
pub fn ocre_crate() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ocre").canonicalize().unwrap()
}

pub fn text(output: &Output) -> (String, String) {
    (String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned())
}

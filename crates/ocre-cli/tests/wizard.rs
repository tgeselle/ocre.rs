//! `ocre new` in a real pseudo-terminal: the guided setup, as a human sees it.

mod common;

use std::fs;

use common::{Sandbox, ocre_crate};
use rexpect::{
    process::WaitStatus,
    session::{PtySession, spawn_command},
};

const DOWN: &str = "\x1b[B";
const ENTER: &str = "\r";

fn start(sandbox: &Sandbox, args: &[&str]) -> PtySession {
    let mut command = sandbox.command(args, &sandbox.work);
    command.env("TERM", "xterm-256color");
    spawn_command(command, Some(60_000)).unwrap()
}

/// Waits for exit; returns the remaining output and the exit code.
fn finish(mut session: PtySession) -> (String, i32) {
    let rest = session.exp_eof().unwrap();
    match session.process().wait().unwrap() {
        WaitStatus::Exited(_, code) => (rest, code),
        other => panic!("unexpected exit: {other:?}"),
    }
}

fn press(session: &mut PtySession, keys: &str) {
    session.send(keys).unwrap();
    session.flush().unwrap();
}

#[test]
fn guided_setup_logs_in_and_deploys() {
    let sandbox = Sandbox::new();
    sandbox.accounts_after_login(&[("acc1", "Main")]);
    let ocre = ocre_crate();
    let mut s = start(&sandbox, &["new", "--ocre-path", ocre.to_str().unwrap()]);

    s.exp_string("What is your app called?").unwrap();
    press(&mut s, &format!("Bad_Name{ENTER}"));
    s.exp_string("invalid app name `Bad_Name`").unwrap();
    press(&mut s, &format!("{}blog{ENTER}", "\x7f".repeat(8)));

    s.exp_string("Pick a starter").unwrap();
    press(&mut s, &format!("{DOWN}{ENTER}"));
    s.exp_string("Connect your Cloudflare account?").unwrap();
    press(&mut s, ENTER);
    s.exp_string("Logged in to Cloudflare").unwrap();
    s.exp_string("Initialize a git repository?").unwrap();
    press(&mut s, "y");
    s.exp_string("Deploy it now?").unwrap();
    press(&mut s, ENTER);
    s.exp_string("Your app is live at https://app.example.workers.dev").unwrap();
    let (_, code) = finish(s);

    assert_eq!(code, 0);
    let root = sandbox.work.join("blog");
    assert!(root.join("src/posts.rs").is_file(), "blog starter");
    assert!(root.join(".git").is_dir());
    assert_eq!(
        sandbox.calls(),
        [
            "whoami --json",
            "whoami --json",
            "login",
            "whoami --json",
            "d1 list --json",
            "deploy",
            "d1 migrations apply blog --remote"
        ]
    );
}

#[test]
fn guided_setup_asks_which_account_when_there_are_several() {
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main"), ("acc2", "Side")]);
    let mut s = start(&sandbox, &["new"]);

    s.exp_string("What is your app called?").unwrap();
    press(&mut s, &format!("shop{ENTER}"));
    s.exp_string("Pick a starter").unwrap();
    press(&mut s, ENTER);
    s.exp_string("Logged in to Cloudflare as dev@example.com").unwrap();
    s.exp_string("Which Cloudflare account should host it?").unwrap();
    press(&mut s, &format!("{DOWN}{ENTER}"));
    s.exp_string("Initialize a git repository?").unwrap();
    press(&mut s, "n");
    s.exp_string("Deploy it now?").unwrap();
    press(&mut s, "n");
    s.exp_string("Happy building!").unwrap();
    let (_, code) = finish(s);

    assert_eq!(code, 0);
    let wrangler = fs::read_to_string(sandbox.work.join("shop/wrangler.toml")).unwrap();
    assert!(wrangler.contains("account_id = \"acc2\""), "{wrangler}");
    assert!(!sandbox.work.join("shop/.git").exists());
    assert!(!sandbox.work.join("shop/src/posts.rs").exists(), "empty starter");
}

#[test]
fn guided_setup_can_skip_cloudflare_for_later() {
    let sandbox = Sandbox::new();
    let mut s = start(&sandbox, &["new", "later"]);

    s.exp_string("Pick a starter").unwrap();
    press(&mut s, ENTER);
    s.exp_string("Connect your Cloudflare account?").unwrap();
    press(&mut s, &format!("{DOWN}{ENTER}"));
    s.exp_string("Initialize a git repository?").unwrap();
    press(&mut s, "n");
    s.exp_string("ocre login").unwrap();
    s.exp_string("Happy building!").unwrap();
    let (_, code) = finish(s);

    assert_eq!(code, 0);
    assert_eq!(sandbox.calls(), ["whoami --json"], "no deploy offered without a login");
}

#[test]
fn flags_answer_the_questions_in_a_terminal_too() {
    let sandbox = Sandbox::new();
    sandbox.accounts_after_login(&[("acc1", "Main")]);
    let s = start(&sandbox, &["new", "flagged", "--starter", "empty", "--login", "--no-git", "--deploy"]);
    let (output, code) = finish(s);

    assert_eq!(code, 0, "{output}");
    assert!(output.contains("Your app is live at https://app.example.workers.dev"), "{output}");
    assert!(!output.contains("Pick a starter") && !output.contains("Deploy it now?"));
}

#[test]
fn flags_can_decline_login_and_deploy() {
    let sandbox = Sandbox::new();
    let s = start(&sandbox, &["new", "quiet", "--starter", "blog", "--no-login", "--git"]);
    let (output, code) = finish(s);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("Happy building!"));
    assert!(sandbox.work.join("quiet/.git").is_dir());

    // Logged in with an API token (no email), several accounts, account chosen by flag.
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main"), ("acc2", "Side")]);
    sandbox.write_state(
        "whoami.json",
        r#"{"loggedIn":true,"accounts":[{"id":"acc1","name":"Main"},{"id":"acc2","name":"Side"}]}"#,
    );
    let s = start(&sandbox, &["new", "token", "--starter", "empty", "--no-git", "--no-deploy", "--account-id", "acc1"]);
    let (output, code) = finish(s);
    assert_eq!(code, 0, "{output}");
    assert!(output.contains("Logged in to Cloudflare as an API token"), "{output}");
    assert!(fs::read_to_string(sandbox.work.join("token/wrangler.toml")).unwrap().contains("account_id = \"acc1\""));
}

#[test]
fn a_failing_deploy_shows_the_captured_wrangler_output() {
    let sandbox = Sandbox::new();
    sandbox.login_as(&[("acc1", "Main")]);
    sandbox.set("deploy_fails");
    let s = start(&sandbox, &["new", "broken", "--starter", "empty", "--no-git", "--deploy"]);
    let (output, code) = finish(s);

    assert_eq!(code, 1);
    assert!(output.contains("error: `wrangler deploy` failed"), "{output}");
    assert!(output.contains("stdout before failure") && output.contains("[ERROR] deploy_fails"), "{output}");
}

#[test]
fn escape_cancels_the_setup() {
    let sandbox = Sandbox::new();
    let mut s = start(&sandbox, &["new"]);
    s.exp_string("What is your app called?").unwrap();
    s.send_control('c').unwrap();
    let (output, code) = finish(s);

    assert_eq!(code, 1);
    assert!(output.contains("error: cancelled"), "{output}");
    assert!(fs::read_dir(&sandbox.work).unwrap().next().is_none(), "nothing created");
}

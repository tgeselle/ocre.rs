//! `ocre logs`: the deployed Worker's live logs through the app's
//! `wrangler tail`, run through the real binary against the fake wrangler.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use support::{Sandbox, text};

#[test]
fn logs_tail_the_worker_named_in_cloudflare_config() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let output = sandbox.ocre(&["logs"], &root);
    let (stdout, _) = text(&output);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("Connected to shop, waiting for logs..."), "{stdout}");
    assert!(stdout.contains("\"request_id\":\"8c2f1a0b9d3e4f5a-CDG\""), "{stdout}");
    assert_eq!(sandbox.calls(), ["wrangler tail shop --format pretty"]);

    sandbox.clear_calls();
    let args = ["logs", "--format", "json", "--status", "error", "--status", "canceled", "--search", "checkout"];
    let (report, ok) = sandbox.json(&args, &root);
    assert!(ok, "{report}");
    assert_eq!(report["command"], "logs");
    assert_eq!(
        sandbox.calls(),
        ["wrangler tail shop --format json --status error --status canceled --search checkout"]
    );
}

#[test]
fn logs_failures_name_wranglers_own_login() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("tail_auth_fails");
    let (report, ok) = sandbox.json(&["logs"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "`wrangler tail shop --format pretty` failed (exit status: 1)");
    assert!(
        report["hint"]
            .as_str()
            .unwrap()
            .starts_with("wrangler tail uses wrangler's own login: run `npx wrangler login` in ")
    );

    fs::remove_dir_all(root.join("node_modules")).unwrap();
    let (report, ok) = sandbox.json(&["logs"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("the app's npm packages are not installed"));
}

#[test]
fn logs_refuse_unknown_formats_and_statuses() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    for args in [["logs", "--format", "yaml"], ["logs", "--status", "failed"]] {
        let output = sandbox.ocre(&args, &root);
        assert!(!output.status.success());
        assert!(text(&output).1.contains("invalid value"), "{args:?}");
    }
    assert!(sandbox.calls().is_empty());
}

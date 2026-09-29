//! `ocre secrets list` and `ocre secrets push`, against the fake wrangler.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use support::{Sandbox, text};

#[test]
fn list_merges_local_and_deployed_names() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let dev_vars = fs::read_to_string(root.join(".dev.vars")).unwrap();
    fs::write(root.join(".dev.vars"), format!("{dev_vars}# a comment\n\nGITHUB_CLIENT_ID=\"abc\"\nnot a pair\n"))
        .unwrap();
    sandbox.set("has_secret");
    let (report, ok) = sandbox.json(&["secrets", "list"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["command"], "secrets list");
    assert_eq!(
        report["secrets"],
        serde_json::json!([
            {"name": "GITHUB_CLIENT_ID", "local": true, "deployed": false},
            {"name": "MAIL_ADAPTER", "local": true, "deployed": false},
            {"name": "OTHER", "local": false, "deployed": true},
            {"name": "SECRET_KEY_BASE", "local": true, "deployed": true},
        ])
    );
    assert_eq!(report["next"], serde_json::json!(["ocre secrets push GITHUB_CLIENT_ID --file <production values>"]));

    let (stdout, _) = text(&sandbox.ocre(&["secrets", "list"], &root));
    assert!(stdout.contains("  OTHER                                       deployed\n"), "{stdout}");

    // No Worker yet, and no .dev.vars: nothing deployed, nothing local.
    fs::remove_file(root.join(".dev.vars")).unwrap();
    sandbox.set("secret_list_fails");
    let (report, ok) = sandbox.json(&["secrets", "list"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["secrets"], serde_json::json!([]));
    assert!(report.get("next").is_none());

    fs::create_dir(root.join(".dev.vars")).unwrap();
    let (report, ok) = sandbox.json(&["secrets", "list"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with("cannot read .dev.vars"), "{report}");
}

#[test]
fn push_uploads_named_values_and_deletes_the_file() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    fs::write(root.join(".prod.vars"), "GITHUB_CLIENT_ID='id-1'\nGITHUB_CLIENT_SECRET = s3cret\nUNUSED=x\n").unwrap();
    let (report, ok) =
        sandbox.json(&["secrets", "push", "GITHUB_CLIENT_ID", "GITHUB_CLIENT_SECRET", "--file", ".prod.vars"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["ran"], serde_json::json!(["uploaded GITHUB_CLIENT_ID", "uploaded GITHUB_CLIENT_SECRET"]));
    assert_eq!(
        sandbox.calls().last().unwrap(),
        r#"uploaded {"GITHUB_CLIENT_ID":"id-1","GITHUB_CLIENT_SECRET":"s3cret"}"#
    );
    assert!(!root.join(".wrangler/ocre-secrets-push.json").exists(), "values do not stay on disk");

    sandbox.set("secret_bulk_fails");
    let (report, ok) = sandbox.json(&["secrets", "push", "UNUSED", "--file", ".prod.vars"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().contains("secret bulk"), "{report}");
    assert!(!root.join(".wrangler/ocre-secrets-push.json").exists(), "deleted after a failure too");
}

#[test]
fn push_refuses_missing_names_and_development_values() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["secrets", "push"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "name the secrets to upload");

    let (report, ok) = sandbox.json(&["secrets", "push", "SECRET_KEY_BASE"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "SECRET_KEY_BASE in .dev.vars is a development value");
    assert!(report["hint"].as_str().unwrap().contains("--file"));

    let (report, ok) = sandbox.json(&["secrets", "push", "NOPE"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "NOPE is not set in .dev.vars");
    assert!(sandbox.calls().iter().all(|call| !call.starts_with("uploaded")), "nothing uploaded");
}

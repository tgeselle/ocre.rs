//! `ocre secrets list` and `ocre secrets push`, against the fake cf.

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
    assert_eq!(sandbox.calls(), ["cf workers secrets list --worker shop"]);

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
    // One bulk call; the body wraps self-named secret_text entries in `secrets` (tests/support/cf_fixtures).
    let calls = sandbox.calls();
    assert_eq!(calls[0], "cf workers secrets bulk --worker shop --file .wrangler/ocre-secrets-push.json");
    let body: serde_json::Value = serde_json::from_str(calls[1].strip_prefix("uploaded ").unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"secrets": {
            "GITHUB_CLIENT_ID": {"name": "GITHUB_CLIENT_ID", "type": "secret_text", "text": "id-1"},
            "GITHUB_CLIENT_SECRET": {"name": "GITHUB_CLIENT_SECRET", "type": "secret_text", "text": "s3cret"},
        }})
    );
    assert_eq!(calls.len(), 2);
    assert!(!root.join(".wrangler/ocre-secrets-push.json").exists(), "values do not stay on disk");
    // The fake creates secrets only for the body the real API accepts: the
    // uploaded secrets are now deployed.
    let (report, _) = sandbox.json(&["secrets", "list"], &root);
    let deployed: Vec<&str> = report["secrets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|secret| secret["deployed"] == true)
        .map(|secret| secret["name"].as_str().unwrap())
        .collect();
    assert!(deployed.contains(&"GITHUB_CLIENT_ID") && deployed.contains(&"GITHUB_CLIENT_SECRET"), "{report}");

    sandbox.set("secret_bulk_fails");
    let (report, ok) = sandbox.json(&["secrets", "push", "UNUSED", "--file", ".prod.vars"], &root);
    assert!(!ok);
    let error = report["error"].as_str().unwrap();
    assert!(
        error.starts_with("`cf workers secrets bulk --worker shop --file .wrangler/ocre-secrets-push.json` failed"),
        "{report}"
    );
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

#[test]
fn a_redeploy_keeps_the_pushed_secrets() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let deployed = |sandbox: &Sandbox| {
        let (report, ok) = sandbox.json(&["secrets", "list"], &root);
        assert!(ok, "{report}");
        let secrets = report["secrets"].as_array().unwrap();
        secrets
            .iter()
            .filter(|s| s["deployed"] == true)
            .map(|s| s["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok && report["secret_created"] == true, "{report}");
    fs::write(root.join(".prod.vars"), format!("{}A=1\nB=2\n", fs::read_to_string(root.join(".prod.vars")).unwrap()))
        .unwrap();
    let (report, ok) = sandbox.json(&["secrets", "push", "A", "B", "--file", ".prod.vars"], &root);
    assert!(ok, "{report}");
    assert_eq!(deployed(&sandbox), ["A", "B", "OTHER", "SECRET_KEY_BASE"]);

    // cf drops every secret on a deploy without --secrets-file: Ocre always passes one.
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok && report.get("secret_created").is_none(), "{report}");
    assert_eq!(deployed(&sandbox), ["A", "B", "OTHER", "SECRET_KEY_BASE"]);
    assert_eq!(fs::read_to_string(sandbox.work.join("../state/uploaded_secrets")).unwrap(), "{}");
}

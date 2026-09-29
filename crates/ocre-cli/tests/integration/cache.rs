//! `ocre g cache` and the KV namespaces `ocre deploy` creates, run through
//! the real binary against the fake wrangler.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use serde_json::json;
use support::{Sandbox, text};

#[test]
fn cache_generator_adds_the_kv_binding_once() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "cache"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], json!(["wrangler.toml"]));
    assert_eq!(report["next"][2], "ocre deploy (creates the KV namespace)");
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    assert!(wrangler.ends_with("# uses a local one. Free plan: 100,000 reads and 1,000 writes a day, 1 GB.\n[[kv_namespaces]]\nbinding = \"CACHE\"\n"), "{wrangler}");
    assert!(wrangler.parse::<toml::Table>().is_ok());

    let (report, ok) = sandbox.json(&["g", "cache"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "wrangler.toml already has the `CACHE` KV binding");
    assert!(report["hint"].as_str().unwrap().contains("ocre::cache::fetch"), "{report}");
    assert_eq!(fs::read_to_string(root.join("wrangler.toml")).unwrap(), wrangler, "nothing written");
}

#[test]
fn deploy_creates_or_links_kv_namespaces_and_writes_their_ids() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("has_secret");
    let (report, ok) = sandbox.json(&["g", "cache"], &root);
    assert!(ok, "{report}");

    let output = sandbox.ocre(&["deploy"], &root);
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains("Created KV namespace shop-cache (id written to wrangler.toml) on Cloudflare\n"),
        "{stdout}"
    );
    assert_eq!(sandbox.calls()[..3], ["kv namespace list", "kv namespace create shop-cache", "kv namespace list"]);
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    assert!(wrangler.ends_with("[[kv_namespaces]]\nbinding = \"CACHE\"\nid = \"id-shop-cache\"\n"), "{wrangler}");

    // Linked: later deploys do not look it up again.
    let calls = sandbox.calls().len();
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok && report.get("provisioned").is_none(), "{report}");
    assert!(!sandbox.calls()[calls..].iter().any(|call| call.starts_with("kv ")));

    // An existing namespace with the expected title is reused, not created.
    fs::write(root.join("wrangler.toml"), format!("{wrangler}\n[[kv_namespaces]]\nbinding = \"RATE_LIMITS\"\n"))
        .unwrap();
    sandbox.write_state("kvns_shop-rate-limits", "existing");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok && report.get("provisioned").is_none(), "{report}");
    let wrangler = fs::read_to_string(root.join("wrangler.toml")).unwrap();
    assert!(wrangler.ends_with("binding = \"RATE_LIMITS\"\nid = \"existing\"\n"), "{wrangler}");
}

#[test]
fn kv_failures_stop_the_deploy_with_a_hint() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("has_secret");
    let (report, ok) = sandbox.json(&["g", "cache"], &root);
    assert!(ok, "{report}");

    sandbox.set("kv_list_fails");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "`wrangler kv namespace list` failed: ✘ [ERROR] kv_list_fails");
    assert!(report["hint"].as_str().unwrap().contains("ocre login"), "{report}");
    fs::remove_file(sandbox.work.join("../state/kv_list_fails")).unwrap();

    sandbox.set("kv_list_garbage");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert!(
        report["error"].as_str().unwrap().starts_with("unexpected `wrangler kv namespace list` output"),
        "{report}"
    );
    fs::remove_file(sandbox.work.join("../state/kv_list_garbage")).unwrap();

    sandbox.set("kv_create_fails");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "`wrangler kv namespace create shop-cache` failed (exit status: 1)");
    fs::remove_file(sandbox.work.join("../state/kv_create_fails")).unwrap();

    sandbox.set("kv_create_unlisted");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "KV namespace shop-cache was created but is not listed");
    assert!(!sandbox.calls().iter().any(|call| call.starts_with("deploy")), "never deployed without its namespace");
    assert!(!fs::read_to_string(root.join("wrangler.toml")).unwrap().contains("id = "));
}

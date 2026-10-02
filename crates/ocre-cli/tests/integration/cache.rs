//! `ocre g cache` and the KV namespaces `ocre deploy` creates, run through
//! the real binary against the fake cf.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use serde_json::json;
use support::{Sandbox, text};

const LIST_PAGE_1: &str = "cf kv namespaces list --per-page 100 --page 1";

fn config(root: &std::path::Path) -> String {
    fs::read_to_string(root.join("cloudflare.config.ts")).unwrap()
}

/// An app with the `CACHE` binding, whose database and SECRET_KEY_BASE exist.
fn cached_app(sandbox: &Sandbox) -> std::path::PathBuf {
    let root = sandbox.new_app("shop", &[]);
    sandbox.set("has_secret");
    sandbox.remote_database("shop");
    let (report, ok) = sandbox.json(&["g", "cache"], &root);
    assert!(ok, "{report}");
    sandbox.clear_calls();
    root
}

#[test]
fn cache_generator_adds_the_kv_binding_once() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "cache"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["updated"], json!(["cloudflare.config.ts"]));
    assert_eq!(report["next"][2], "ocre deploy (creates the KV namespace)");
    let written = config(&root);
    assert!(written.contains("\t\t\tCACHE: bindings.kv(),\n"), "{written}");

    let (report, ok) = sandbox.json(&["g", "cache"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "cloudflare.config.ts already has the `CACHE` binding");
    assert!(report["hint"].as_str().unwrap().contains("ocre::cache::fetch"), "{report}");
    assert_eq!(config(&root), written, "nothing written");

    // Without the `// ocre:env` marker: named, nothing written.
    let other = sandbox.new_app("other", &[]);
    let without = config(&other).replace("// ocre:env", "");
    fs::write(other.join("cloudflare.config.ts"), &without).unwrap();
    let (report, ok) = sandbox.json(&["g", "cache"], &other);
    assert!(!ok);
    assert_eq!(report["error"], "cloudflare.config.ts is missing the `// ocre:env` marker");
    assert!(report["hint"].as_str().unwrap().contains("inside `worker.env: { ... }`"), "{report}");
    assert_eq!(config(&other), without);
}

#[test]
fn deploy_creates_or_links_kv_namespaces_and_writes_their_ids() {
    let sandbox = Sandbox::new();
    let root = cached_app(&sandbox);

    let output = sandbox.ocre(&["deploy"], &root);
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains("Created KV namespace shop-cache (id written to cloudflare.config.ts) on Cloudflare\n"),
        "{stdout}"
    );
    assert_eq!(
        sandbox.calls(),
        [
            "cf d1 list --name shop",
            LIST_PAGE_1,
            "cf kv namespaces create --title shop-cache",
            "cf workers secrets list --worker shop",
            "cf d1 migrations apply uuid-shop",
            "cf deploy --secrets-file .wrangler/ocre-secrets.json",
            "secrets file ok",
            "build --release",
        ]
    );
    let written = config(&root);
    assert!(written.contains("\tCACHE: bindings.kv({ id: \"id-shop-cache\" }),\n"), "{written}");

    // Linked: later deploys do not look it up again.
    sandbox.clear_calls();
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok && report.get("provisioned").is_none(), "{report}");
    assert!(!sandbox.calls().iter().any(|call| call.starts_with("cf kv ")), "{:?}", sandbox.calls());

    // An existing namespace with the expected title is reused, not created.
    let with_limits = written.replace("// ocre:env", "RATE_LIMITS: bindings.kv({}),\n\t\t\t// ocre:env");
    fs::write(root.join("cloudflare.config.ts"), with_limits).unwrap();
    sandbox.write_state("kvns_shop-rate-limits", "existing");
    sandbox.clear_calls();
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok && report.get("provisioned").is_none(), "{report}");
    assert!(!sandbox.calls().iter().any(|call| call.starts_with("cf kv namespaces create")), "{:?}", sandbox.calls());
    let written = config(&root);
    assert!(written.contains("RATE_LIMITS: bindings.kv({ id: \"existing\" }),"), "{written}");
    assert!(written.contains("CACHE: bindings.kv({ id: \"id-shop-cache\" }),"), "{written}");
}

#[test]
fn kv_namespaces_are_found_on_later_pages() {
    let sandbox = Sandbox::new();
    let root = cached_app(&sandbox);
    let others: Vec<_> =
        (0..100).map(|i| json!({ "id": format!("other-{i}"), "title": format!("other-{i}") })).collect();
    sandbox.write_state("kv_page_1.json", &json!(others).to_string());
    sandbox.write_state("kv_page_2.json", &json!([{ "id": "paged", "title": "shop-cache" }]).to_string());

    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert!(report.get("provisioned").is_none(), "linked, not created: {report}");
    let calls = sandbox.calls();
    assert_eq!(calls[1..3], [LIST_PAGE_1, "cf kv namespaces list --per-page 100 --page 2"], "{calls:?}");
    assert!(!calls.iter().any(|call| call.contains("--page 3") || call.starts_with("cf kv namespaces create")));
    assert!(config(&root).contains("CACHE: bindings.kv({ id: \"paged\" }),"));
}

#[test]
fn kv_failures_stop_the_deploy_with_a_hint() {
    let sandbox = Sandbox::new();
    let root = cached_app(&sandbox);
    let pristine = config(&root);
    let unset = |marker: &str| fs::remove_file(sandbox.work.join("../state").join(marker)).unwrap();

    sandbox.set("kv_list_fails");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(report["error"], format!("`{LIST_PAGE_1}` failed: ┌ Error\n│ kv_list_fails\n└"));
    assert!(report["hint"].as_str().unwrap().contains("ocre login"), "{report}");
    unset("kv_list_fails");

    sandbox.set("kv_list_garbage");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert!(report["error"].as_str().unwrap().starts_with(&format!("unexpected `{LIST_PAGE_1}` output")), "{report}");
    unset("kv_list_garbage");

    sandbox.set("kv_create_fails");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "`cf kv namespaces create --title shop-cache` failed: ┌ Error\n│ kv_create_fails\n└");
    unset("kv_create_fails");

    // Created, but cf answered without the id: the next deploy links it by title.
    sandbox.set("kv_create_no_id");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(!ok);
    assert_eq!(
        report["error"],
        "`cf kv namespaces create --title shop-cache` returned no id: {\"supports_url_encoding\":true,\"title\":\"shop-cache\"}"
    );
    assert!(report["hint"].as_str().unwrap().starts_with("run `ocre deploy` again"), "{report}");
    assert!(!sandbox.calls().iter().any(|call| call.starts_with("cf deploy")), "never deployed without its namespace");
    assert_eq!(config(&root), pristine, "no id written");
    let (report, ok) = sandbox.json(&["deploy"], &root);
    assert!(ok, "{report}");
    assert!(config(&root).contains("CACHE: bindings.kv({ id: \"id-shop-cache\" }),"));
}

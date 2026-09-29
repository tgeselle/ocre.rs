//! `ocre g locale`, `ocre i18n missing` and the locale check of `ocre dev`
//! and `ocre deploy`, run through the real binary.

#[path = "../support/mod.rs"]
mod support;

use std::fs;

use serde_json::json;
use support::{Sandbox, text};

fn read(root: &std::path::Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap()
}

#[test]
fn first_locales_wire_the_catalog_and_later_ones_extend_it() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "scaffold", "Comment", "body:text"], &root);
    assert!(ok, "{report}");
    let (report, ok) = sandbox.json(&["g", "locale", "en", "fr"], &root);
    assert!(ok, "{report}");
    assert_eq!(report["created"], json!(["locales/en.yml", "locales/fr.yml"]));
    assert_eq!(report["updated"], json!(["src/lib.rs"]));
    assert_eq!(report["next"][1], "ocre i18n missing");
    let lib = read(&root, "src/lib.rs");
    // Last in the chain: it also wraps the routes merged before it.
    let chain =
        "        // ocre:routes\n        .merge(comments::routes())\n        .layer(ocre::i18n::layer(&LOCALES))\n}";
    assert!(lib.contains(chain), "{lib}");
    assert!(lib.ends_with("static LOCALES: ocre::i18n::Locales = ocre::locales!(\"en\", \"fr\");\n"), "{lib}");
    assert!(read(&root, "locales/en.yml").ends_with("en:\n  app:\n    welcome: \"Welcome\"\n"));
    assert!(read(&root, "locales/fr.yml").contains("Translate every key of locales/en.yml"));

    let (report, ok) = sandbox.json(&["g", "locale", "pt-BR", "zh-Hant"], &root);
    assert!(ok, "{report}");
    let lib = read(&root, "src/lib.rs");
    assert!(lib.contains("ocre::locales!(\"en\", \"fr\", \"pt-BR\", \"zh-Hant\");"), "{lib}");
    assert_eq!(lib.matches(".layer(ocre::i18n::layer(&LOCALES))").count(), 1);
    assert!(read(&root, "locales/pt-BR.yml").ends_with("pt-BR:\n"));

    // Routes generated later stay inside the layer.
    let (report, ok) = sandbox.json(&["g", "scaffold", "Post", "title:string"], &root);
    assert!(ok, "{report}");
    let lib = read(&root, "src/lib.rs");
    assert!(
        lib.contains(
            "// ocre:routes\n        .merge(posts::routes())\n        .merge(comments::routes())\n        \
             .layer(ocre::i18n::layer(&LOCALES))"
        ),
        "{lib}"
    );
    let (report, ok) = sandbox.json(&["i18n", "missing"], &root);
    assert!(!ok, "fr, pt-BR and zh-Hant lack app.welcome");
    assert_eq!(
        report["error"],
        "3 locale problems:\n  locales/fr.yml: missing app.welcome\n  locales/pt-BR.yml: missing app.welcome\n  \
         locales/zh-Hant.yml: missing app.welcome"
    );
}

#[test]
fn locale_errors_name_the_fix_and_write_nothing() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    for code in ["FR", "e", "en-x", "english"] {
        let (report, ok) = sandbox.json(&["g", "locale", code], &root);
        assert!(!ok);
        assert_eq!(report["error"], format!("invalid locale code `{code}`"));
        assert!(report["hint"].as_str().unwrap().contains("pt-BR"), "{report}");
    }
    let (report, ok) = sandbox.json(&["g", "locale", "en", "fr", "fr"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "locale `fr` is already declared in src/lib.rs");
    assert!(!root.join("locales").exists(), "nothing written");

    let (report, ok) = sandbox.json(&["g", "locale", "en"], &root);
    assert!(ok, "{report}");
    let (report, ok) = sandbox.json(&["g", "locale", "en"], &root);
    assert!(!ok);
    assert_eq!(report["hint"], "edit locales/en.yml; `ocre i18n missing` lists keys to translate");

    fs::write(root.join("src/lib.rs"), "static LOCALES: ocre::i18n::Locales = ocre::locales!();\n").unwrap();
    let (report, ok) = sandbox.json(&["g", "locale", "fr"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "ocre::locales!() in src/lib.rs lists no locale");

    fs::write(root.join("src/lib.rs"), "fn main() {}\n").unwrap();
    let (report, ok) = sandbox.json(&["g", "locale", "fr"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "src/lib.rs is missing the `// ocre:routes` marker");
    assert!(!root.join("locales/fr.yml").exists());
}

const EN: &str =
    "en:\n  posts:\n    title: Posts\n    count:\n      one: \"%{count} post\"\n      other: \"%{count} posts\"\n";

#[test]
fn missing_lists_what_each_locale_lacks() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["i18n", "missing"], &root);
    assert!(!ok);
    assert_eq!(report["error"], "src/lib.rs declares no locales (ocre::locales!(...) not found)");
    assert!(report["hint"].as_str().unwrap().contains("ocre g locale en"), "{report}");

    let (report, ok) = sandbox.json(&["g", "locale", "en", "fr", "ru"], &root);
    assert!(ok, "{report}");
    fs::write(root.join("locales/en.yml"), EN).unwrap();
    fs::write(root.join("locales/fr.yml"), "fr:\n  posts:\n    title: Articles\n").unwrap();
    fs::write(root.join("locales/ru.yml"), "ru:\n  posts:\n    title: \"Посты\"\n    count:\n      one: x\n").unwrap();
    fs::write(root.join("locales/de.yml"), "de:\n").unwrap();
    let (report, ok) = sandbox.json(&["i18n", "missing"], &root);
    assert!(!ok);
    assert_eq!(
        report["error"],
        "6 locale problems:\n  locales/de.yml: not declared; add \"de\" to ocre::locales!(...) in src/lib.rs\n  \
         locales/fr.yml: missing posts.count.one\n  locales/fr.yml: missing posts.count.other\n  \
         locales/ru.yml: missing posts.count.few\n  locales/ru.yml: missing posts.count.many\n  \
         locales/ru.yml: missing posts.count.other"
    );
    assert!(report["hint"].as_str().unwrap().contains("same path as in locales/en.yml"), "{report}");

    fs::remove_file(root.join("locales/de.yml")).unwrap();
    fs::write(root.join("locales/fr.yml"), EN.replace("en:", "fr:")).unwrap();
    let ru = "ru:\n  posts:\n    title: x\n    count:\n      one: x\n      few: x\n      many: x\n      other: x\n";
    fs::write(root.join("locales/ru.yml"), ru).unwrap();
    let output = sandbox.ocre(&["i18n", "missing"], &root);
    let (stdout, _) = text(&output);
    assert!(output.status.success(), "{stdout}");
    assert_eq!(stdout, "  every locale (en, fr, ru) has every key of locales/en.yml\n");

    fs::remove_dir_all(root.join("locales")).unwrap();
    let (report, ok) = sandbox.json(&["i18n", "missing"], &root);
    assert!(!ok);
    assert!(
        report["error"].as_str().unwrap().contains("locales/en.yml: declared in src/lib.rs but missing"),
        "{report}"
    );
}

#[test]
fn dev_and_deploy_refuse_locale_files_the_worker_cannot_load() {
    let sandbox = Sandbox::new();
    let root = sandbox.new_app("shop", &[]);
    let (report, ok) = sandbox.json(&["g", "locale", "en", "fr"], &root);
    assert!(ok, "{report}");
    fs::write(root.join("locales/fr.yml"), "fr:\n  title: %{count} posts\n").unwrap();
    for command in [&["dev"][..], &["deploy"]] {
        let (report, ok) = sandbox.json(command, &root);
        assert!(!ok);
        assert_eq!(
            report["error"],
            "invalid locale files:\n  locales/fr.yml:2: quote values starting with `%`, e.g. \"%{count} posts\""
        );
        assert!(report["hint"].as_str().unwrap().contains("ocre i18n missing"), "{report}");
    }
    assert!(sandbox.calls().is_empty(), "wrangler never ran");

    // Missing keys are not errors for dev.
    fs::write(root.join("locales/fr.yml"), "fr:\n").unwrap();
    let (report, ok) = sandbox.json(&["dev"], &root);
    assert!(ok, "{report}");
}

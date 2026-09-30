use super::*;

const APP: &str = include_str!("../templates/new/cloudflare.config.ts");

fn app() -> Config {
    Config::parse(APP.replace("__APP_NAME__", "demo")).unwrap()
}

fn parse(text: &str) -> Config {
    Config::parse(text.to_owned()).unwrap_or_else(|err| panic!("{}", err.message))
}

fn error(text: &str) -> CliError {
    Config::parse(text.to_owned()).unwrap_err()
}

#[test]
fn reads_the_generated_file() {
    let config = app();
    assert_eq!(config.worker_name().unwrap(), "demo");
    assert_eq!(config.compatibility_date.as_deref(), Some("2026-09-01"));
    assert_eq!(config.database_name().unwrap(), "demo");
    // Commented-out entries (MAIL_ADAPTER, EMAIL) are not bindings.
    let keys: Vec<_> = config.env.iter().map(|call| call.key.as_deref().unwrap()).collect();
    assert_eq!(keys, ["DB", "MAIL_FROM"]);
    assert_eq!(config.vars().collect::<Vec<_>>(), [("MAIL_FROM", Some("demo <noreply@example.com>"))]);
    assert!(config.triggers.is_empty() && config.exports.is_empty());
    assert!(config.queue_names().unwrap().is_empty() && config.bucket_names().unwrap().is_empty());
    assert!(config.crons().unwrap().is_empty() && config.kv_without_id().is_empty());
}

#[test]
fn inserts_entries_after_the_markers_with_their_indentation() {
    let text = app().insert(ENV_MARKER, "// Cache.\nCACHE: bindings.kv(),").unwrap();
    assert!(text.contains("\t\t\t// ocre:env\n\t\t\t// Cache.\n\t\t\tCACHE: bindings.kv(),\n\t\t},"), "{text}");
    let config = parse(&text);
    assert_eq!(config.kv_without_id(), ["CACHE"]);
    // Inserting the same entry again changes nothing.
    assert_eq!(config.insert(ENV_MARKER, "// Cache.\nCACHE: bindings.kv(),").unwrap(), text);

    let text = config.insert(TRIGGERS_MARKER, "triggers.scheduled({ schedule: \"0 3 * * *\" }),").unwrap();
    let text =
        parse(&text).insert(EXPORTS_MARKER, "OcreChannel: exports.durableObject({ storage: \"sqlite\" }),").unwrap();
    let config = parse(&text);
    assert_eq!(config.crons().unwrap(), ["0 3 * * *"]);
    assert_eq!(config.export("OcreChannel").unwrap().field("storage"), Some("sqlite"));
    assert!(config.export("Other").is_none());
}

#[test]
fn a_missing_marker_names_where_it_goes() {
    let config = parse("export default defineConfig({ worker: { name: \"a\", env: {} } });");
    for (marker, place) in
        [(ENV_MARKER, "worker.env: {"), (TRIGGERS_MARKER, "worker.triggers: ["), (EXPORTS_MARKER, "worker.exports: {")]
    {
        let err = config.insert(marker, "X: bindings.kv(),").unwrap_err();
        assert_eq!(err.message, format!("cloudflare.config.ts is missing the `{marker}` marker"));
        assert!(err.hint.unwrap().contains(place));
    }
}

#[test]
fn reads_human_formatted_entries() {
    let config = parse(
        r#"import { bindings, defineConfig, triggers } from "cf/config";
/* A block comment with bindings.kv() and a } brace. */
export default defineConfig({
	accountId: "123",
	worker: {
		"name": 'shop', // a comment with "quotes" and ( parens
		env: {
			DB: bindings . d1 ( {
				name: "shop", // trailing comment
				id: "uuid",
			} ),
			/* JOBS: bindings.queue({ name: "commented" }), */
			JOBS: bindings.queue({ name: "shop-jobs" }),
			NOTE: bindings.text("a \"quoted\" (value) with bindings.kv() inside"),
			...spread,
			ALIAS: bindingsAlias.kv(),
			OTHER: someFunction({ name: "x" }),
			MORE: bindings.kv() || fallback,
			NAKED: bindings,
			NOCALL: bindings.kv,
			EMPTY: bindings.(),
		},
		triggers: [
			triggers.queue({
				name: "shop-jobs",
				deadLetterQueue: "shop-jobs-failed",
				maxBatchSize: 10,
			}),
			triggers.queue({ name: "shop-jobs" }),
			triggers.scheduled({ schedule: "*/5 * * * *" }),
			somethingElse(),
		],
	},
});
"#,
    );
    assert_eq!(config.worker_name().unwrap(), "shop");
    assert_eq!(config.database_name().unwrap(), "shop");
    let keys: Vec<_> = config.env.iter().map(|call| call.key.as_deref().unwrap()).collect();
    assert_eq!(keys, ["DB", "JOBS", "NOTE"]);
    assert_eq!(config.binding("NOTE").unwrap().text(), Some("a \"quoted\" (value) with bindings.kv() inside"));
    assert_eq!(config.queue_names().unwrap(), ["shop-jobs", "shop-jobs-failed"]);
    assert_eq!(config.crons().unwrap(), ["*/5 * * * *"]);
    assert_eq!(config.binding("DB").unwrap().kind, "d1");
}

#[test]
fn non_literal_values_fail_only_when_needed() {
    let config = parse(
        "export default defineConfig({ worker: { name: `${app}`, env: { DB: bindings.d1({ name: `${app}` }), \
         CACHE: bindings.kv(options), JOBS: bindings.queue(queue), STORAGE: bindings.r2({ name: bucket }), \
         MAIL_FROM: bindings.text(sender) }, triggers: [triggers.queue(consumer), triggers.scheduled(cron)] } });",
    );
    assert_eq!(config.vars().collect::<Vec<_>>(), [("MAIL_FROM", None)]);
    let err = config.worker_name().unwrap_err();
    assert!(err.hint.unwrap().contains("name: \"<app-name>\""));
    let err = config.database_name().unwrap_err();
    assert_eq!(err.message, "cloudflare.config.ts defines `DB` in a form Ocre cannot read");
    assert_eq!(err.hint.unwrap(), "write it as a literal: `DB: bindings.d1({ name: \"<app-name>\" }),`");
    assert!(config.queue_names().unwrap_err().message.contains("`JOBS`"));
    assert!(config.bucket_names().unwrap_err().message.contains("`STORAGE`"));
    assert!(config.crons().unwrap_err().hint.unwrap().contains("triggers.scheduled"));
    assert!(config.set_kv_id("CACHE", "id").unwrap_err().message.contains("`CACHE`"));
    // A non-literal KV binding counts as having no id.
    assert_eq!(config.kv_without_id(), ["CACHE"]);

    let consumer_only = parse("export default defineConfig({ worker: { triggers: [triggers.queue(consumer)] } });");
    assert!(consumer_only.queue_names().unwrap_err().hint.unwrap().contains("deadLetterQueue"));
}

#[test]
fn database_errors_show_the_canonical_form() {
    let missing = parse("export default defineConfig({ worker: { name: \"a\", env: {} } });");
    let err = missing.database_name().unwrap_err();
    assert_eq!(err.message, "cloudflare.config.ts has no D1 database bound to `DB`");
    assert_eq!(err.hint.unwrap(), "add `DB: bindings.d1({ name: \"a\" }),` inside `worker.env`");
    let wrong_kind = parse("export default defineConfig({ worker: { name: \"a\", env: { DB: bindings.kv() } } });");
    assert!(wrong_kind.database_name().unwrap_err().message.contains("cannot read"));
    assert!(wrong_kind.binding_field("DB", "name", "DB: ...").is_err());
}

#[test]
fn the_structure_must_be_there() {
    for text in [
        "",
        "export default {}",
        "export default defineConfig(config);",
        "export default defineConfig({ name: \"a\" });",
        "export default defineConfig({ worker: config });",
        "export default defineConfig({ worker: { name: \"a\" }",
        "const x = mydefineConfig({ worker: {} });",
    ] {
        let err = error(text);
        assert!(err.message.contains("export default defineConfig"), "{text}");
    }
    // Sections of the wrong shape are ignored.
    let config = parse("export default defineConfig({ worker: { env: [], triggers: {}, exports: 1 } });");
    assert!(config.env.is_empty() && config.triggers.is_empty() && config.exports.is_empty());
}

#[test]
fn duplicate_keys_are_an_error() {
    let err = error("export default defineConfig({ worker: { env: { A: bindings.kv(), A: bindings.kv() } } });");
    assert_eq!(err.message, "cloudflare.config.ts defines `A` twice");
    assert_eq!(err.hint.unwrap(), "keep one `A: bindings.kv(...)` entry");
}

#[test]
fn kv_ids_are_written_into_the_call() {
    let text = "export default defineConfig({ worker: { env: {\n\t\tA: bindings.kv(),\n\t\tB: bindings.kv({ }),\n\t\t\
                C: bindings.kv({ preview: true }),\n\t\tD: bindings.kv({ id: \"old\" }),\n\t\tE: bindings.d1({ name: \"x\" }),\n} } });";
    let config = parse(text);
    assert_eq!(config.kv_without_id(), ["A", "B", "C"]);
    let text = config.set_kv_id("A", "id-a").unwrap();
    assert!(text.contains("A: bindings.kv({ id: \"id-a\" }),"), "{text}");
    let text = parse(&text).set_kv_id("B", "id-b").unwrap();
    assert!(text.contains("B: bindings.kv({ id: \"id-b\" }),"), "{text}");
    let config = parse(&parse(&text).set_kv_id("C", "id-c").unwrap());
    assert_eq!(config.binding("C").unwrap().field("id"), Some("id-c"));
    assert_eq!(config.binding("C").unwrap().values.as_ref().unwrap()[0]["preview"], true);
    assert!(config.kv_without_id().is_empty());
    for binding in ["E", "missing"] {
        let err = config.set_kv_id(binding, "x").unwrap_err();
        assert_eq!(err.hint.unwrap(), format!("write it as a literal: `{binding}: bindings.kv(),`"));
    }
}

#[test]
fn account_ids_go_before_the_worker() {
    let text = with_account_id(&APP.replace("__APP_NAME__", "demo"), "abc");
    assert!(text.contains("export default defineConfig({\n\taccountId: \"abc\",\n\tworker: {"), "{text}");
    assert_eq!(parse(&text).worker_name().unwrap(), "demo");
}

#[test]
fn reads_the_assets_directory_of_the_build_config() {
    let root = std::env::temp_dir().join(format!("ocre-config-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    assert_eq!(assets_directory(&root), None);
    std::fs::write(root.join(BUILD_FILE), include_str!("../templates/new/wrangler.config.ts")).unwrap();
    assert_eq!(assets_directory(&root).as_deref(), Some("public"));
    std::fs::write(root.join(BUILD_FILE), "export default defineWranglerConfig({ build: {} });").unwrap();
    assert_eq!(assets_directory(&root), None);
    std::fs::write(root.join(BUILD_FILE), "export default somethingElse;").unwrap();
    assert_eq!(assets_directory(&root), None);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn masking_keeps_offsets_and_skips_strings_and_comments() {
    let text = "a /* x\n} */ \"b\\\"(\" 'c' `d${e}` // f(\ng";
    let masked = mask(text);
    assert_eq!(masked.len(), text.len());
    let masked = String::from_utf8(masked).unwrap();
    assert!(!masked.contains(['(', '}', 'x', 'b', 'f']), "{masked}");
    assert!(masked.starts_with("a ") && masked.ends_with("\ng"));
    assert_eq!(mask("/* unterminated").len(), 15);
    assert_eq!(mask("\"unterminated \\").len(), 15);
}

#[test]
fn domains_are_read_and_written_as_a_list_of_strings() {
    let config = parse("export default defineConfig({\n  worker: {\n    name: \"a\",\n  },\n});\n");
    assert!(config.domains().unwrap().is_empty());
    let added = config.with_domains(&["www.example.com".to_owned()]).unwrap();
    assert!(added.contains("    name: \"a\",\n    // Custom domains"), "{added}");
    assert!(added.contains("    domains: [\"www.example.com\"],\n  },"), "{added}");

    let config = parse(&added);
    assert_eq!(config.domains().unwrap(), ["www.example.com"]);
    let both = ["www.example.com".to_owned(), "example.com".to_owned()];
    let replaced = config.with_domains(&both).unwrap();
    assert!(replaced.contains("domains: [\"www.example.com\", \"example.com\"],"), "{replaced}");

    let config = parse("export default defineConfig({ worker: { name: \"a\", domains: list } });");
    assert_eq!(config.domains().unwrap_err().message, "cloudflare.config.ts has a `domains` entry Ocre cannot read");
    assert!(config.with_domains(&both).is_err());
    assert!(parse("export default defineConfig({ worker: {} });").with_domains(&both).is_err());
}

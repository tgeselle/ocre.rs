use std::{fs, path::PathBuf};

use serde_json::{Value, json};

use super::*;

/// A scratch app root with `files` (path, contents) written in it.
fn app(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("ocre-fixtures-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for (path, contents) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    root
}

fn sql(name: &str, files: &[(&str, &str)]) -> String {
    let root = app(name, files);
    let sql = to_sql(&root, FIXTURES).unwrap();
    let _ = fs::remove_dir_all(root);
    sql
}

fn error(name: &str, files: &[(&str, &str)]) -> CliError {
    let root = app(name, files);
    let err = to_sql(&root, FIXTURES).unwrap_err();
    fs::remove_dir_all(root).unwrap();
    err
}

const MIGRATION: (&str, &str) = (
    "migrations/0001_create_posts.sql",
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);\n\
     CREATE TABLE posts (id INTEGER PRIMARY KEY, author_id INTEGER REFERENCES users(id), title TEXT);\n",
);

#[test]
fn identify_is_rails_crc32_modulo() {
    // zlib.crc32(b"david") == 2274809787; 2274809787 % (2**30 - 1) == 127326141.
    assert_eq!(crc32(b"david"), 2_274_809_787);
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
    assert_eq!(identify("david"), 127_326_141);
    assert_eq!(identify("grace"), 370_882_803);
}

#[test]
fn a_missing_or_empty_directory_gives_no_sql() {
    assert_eq!(sql("missing", &[]), "");
    assert_eq!(sql("other-files", &[("db/fixtures/README.txt", "notes"), ("db/fixtures/noext", "")]), "");
}

#[test]
fn rows_become_inserts_after_deleting_every_fixture_table() {
    let users = "# users\nada:\n  name: Ada\ngrace:\n  id: 7\n  name: Grace\n";
    let posts = "DEFAULTS: &defaults\n  published: true\n  rating: 4.5\n  views: 3\n\
                 hello:\n  <<: *defaults\n  title: \"$LABEL world\"\n  author: ada\n  views: 10\n\
                 bye:\n  <<: *defaults\n  title: It's $LABEL\n  author: grace\n  published: false\n  body: null\n";
    let out = sql("rows", &[MIGRATION, ("db/fixtures/users.yml", users), ("db/fixtures/posts.yaml", posts)]);
    let ada = identify("ada");
    let (hello, bye) = (identify("hello"), identify("bye"));
    assert_eq!(
        out,
        format!(
            "PRAGMA defer_foreign_keys = on;\n\
             DELETE FROM \"posts\";\n\
             DELETE FROM \"users\";\n\
             INSERT INTO \"posts\" (\"id\", \"title\", \"author_id\", \"views\", \"published\", \"rating\") \
             VALUES ({hello}, 'hello world', {ada}, 10, 1, 4.5);\n\
             INSERT INTO \"posts\" (\"id\", \"title\", \"author_id\", \"published\", \"body\", \"rating\", \"views\") \
             VALUES ({bye}, 'It''s bye', 7, 0, NULL, 4.5, 3);\n\
             INSERT INTO \"users\" (\"id\", \"name\") VALUES ({ada}, 'Ada');\n\
             INSERT INTO \"users\" (\"id\", \"name\") VALUES (7, 'Grace');\n"
        )
    );
}

#[test]
fn merges_lists_nested_merges_and_json_files() {
    let posts = "base: &base\n  a: 1\nmore: &more\n  <<: *base\n  b: 2\n\
                 row:\n  <<: [*more, {a: 9, c: 3}]\n  d: 4\nempty:\n";
    let tags =
        r#"{"t1": {"id": "uuid-1", "tags": ["x", 1, 2.5, "1e999", true, null, {"k": "v"}], "meta": {"n": .inf}}}"#;
    let out = sql("merge", &[("db/fixtures/posts.yml", posts), ("db/fixtures/tags.json", tags)]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[3], format!("INSERT INTO \"posts\" (\"id\", \"a\") VALUES ({}, 1);", identify("base")));
    assert_eq!(lines[4], format!("INSERT INTO \"posts\" (\"id\", \"b\", \"a\") VALUES ({}, 2, 1);", identify("more")));
    assert_eq!(
        lines[5],
        format!("INSERT INTO \"posts\" (\"id\", \"d\", \"b\", \"a\", \"c\") VALUES ({}, 4, 2, 1, 3);", identify("row"))
    );
    assert_eq!(lines[6], format!("INSERT INTO \"posts\" (\"id\") VALUES ({});", identify("empty")));
    assert_eq!(
        lines[7],
        "INSERT INTO \"tags\" (\"id\", \"tags\", \"meta\") VALUES ('uuid-1', \
         '[\"x\",1,2.5,\"1e999\",true,null,{\"k\":\"v\"}]', '{\"n\":\".inf\"}');"
    );
}

#[test]
fn empty_files_still_empty_their_table() {
    let out = sql("empty", &[("db/fixtures/posts.yml", "# no rows\n"), ("db/fixtures/tags.yml", "~\n")]);
    assert_eq!(out, "PRAGMA defer_foreign_keys = on;\nDELETE FROM \"posts\";\nDELETE FROM \"tags\";\n");
}

#[test]
fn associations_prefer_the_plural_table_then_a_unique_label() {
    let files = [
        MIGRATION,
        ("db/fixtures/authors.yml", "ada:\n  id: 1\n"),
        ("db/fixtures/users.yml", "ada:\n  id: 2\n"),
        ("db/fixtures/posts.yml", "p:\n  author: ada\n"),
    ];
    let expected = format!("INSERT INTO \"posts\" (\"id\", \"author_id\") VALUES ({}, 1);\n", identify("p"));
    assert!(sql("plural", &files).contains(&expected));

    let files = [
        MIGRATION,
        ("db/fixtures/admins.yml", "ada:\n  id: 1\n"),
        ("db/fixtures/users.yml", "ada:\n  id: 2\n"),
        ("db/fixtures/posts.yml", "p:\n  author: ada\n"),
    ];
    let err = error("ambiguous", &files);
    assert_eq!(err.message, "db/fixtures/posts.yml: `p`: `ada` labels rows of several tables, none of them `authors`");
    assert_eq!(err.hint.unwrap(), "set `author_id` to the id instead");
}

fn assert_error(name: &str, files: &[(&str, &str)], message: &str, hint: &str) {
    let err = error(name, files);
    assert_eq!((err.message.as_str(), err.hint.as_deref()), (message, Some(hint)));
}

#[test]
fn errors_name_the_file_and_label_with_a_hint() {
    let posts = |yaml: &'static str| [MIGRATION, ("db/fixtures/posts.yml", yaml)];
    assert_error(
        "unknown-label",
        &posts("p:\n  author: nobody\n"),
        "db/fixtures/posts.yml: `p`: no fixture labelled `nobody` for `author`",
        "add a `nobody:` row to a fixture file, or set `author_id` to an id",
    );
    assert_error(
        "not-a-label",
        &posts("p:\n  author: 3\n"),
        "db/fixtures/posts.yml: `p`: `author` names a fixture label (`author_id` is a column)",
        "write `author: <label>`, or set `author_id` directly",
    );
    assert_error(
        "syntax",
        &posts("p: [\n"),
        "db/fixtures/posts.yml: invalid YAML: while parsing a node, did not find expected node content at byte 5 line 2 column 1",
        "a fixture file maps labels to rows of `column: value` (YAML or JSON)",
    );
    assert_error(
        "top",
        &posts("- a\n"),
        "db/fixtures/posts.yml: expected a mapping of labels to rows",
        "write one `label:` per row, its columns indented below it",
    );
    assert_error(
        "label",
        &posts("1:\n  a: 1\n"),
        "db/fixtures/posts.yml: labels must be strings, found Integer(1)",
        "quote the label, e.g. `\"1\":`",
    );
    assert_error(
        "row",
        &posts("p: 3\n"),
        "db/fixtures/posts.yml: `p`: expected a mapping of `column: value`",
        "indent the row's columns below its label",
    );
    assert_error(
        "column",
        &posts("p:\n  1: a\n"),
        "db/fixtures/posts.yml: `p`: column names must be strings, found Integer(1)",
        "quote the column name",
    );
    assert_error(
        "merge",
        &posts("p:\n  <<: 3\n"),
        "db/fixtures/posts.yml: `p`: `<<` merges a mapping or a list of mappings",
        "merge an anchored row, e.g. `<<: *defaults`",
    );
    assert_error(
        "bad-id",
        &posts("p:\n  id: !!int x\n"),
        "db/fixtures/posts.yml: `p`: invalid value",
        "check the value's `!!tag`, or quote it to store it as text",
    );
    assert_error(
        "bad-value",
        &posts("p:\n  a: !!float x\n"),
        "db/fixtures/posts.yml: `p`: invalid value",
        "check the value's `!!tag`, or quote it to store it as text",
    );
    assert_error(
        "bad-nested",
        &posts("p:\n  a: [!!int x]\n"),
        "db/fixtures/posts.yml: `p`: invalid value",
        "check the value's `!!tag`, or quote it to store it as text",
    );
    assert_error(
        "json-key",
        &posts("p:\n  a: {1: x}\n"),
        "db/fixtures/posts.yml: `p`: JSON keys must be strings, found Integer(1)",
        "quote the key",
    );
}

#[test]
fn to_yaml_writes_fixtures_to_sql_reads_back() {
    let rows: Vec<Vec<(String, Value)>> = vec![
        vec![
            ("id".into(), json!(1)),
            ("title".into(), json!("It's \"quoted\"\nnext: line")),
            ("null".into(), Value::Null),
        ],
        vec![("id".into(), json!("uuid-2")), ("1st col".into(), json!(2.5))],
        vec![("name".into(), json!("no id"))],
    ];
    let rows: Vec<&[(String, Value)]> = rows.iter().map(Vec::as_slice).collect();
    let yaml = to_yaml("posts", &rows, "Dumped.");
    assert_eq!(
        yaml,
        "# Dumped.\nposts_1:\n  id: 1\n  title: \"It's \\\"quoted\\\"\\nnext: line\"\n  \"null\": null\n\
         \"posts_uuid-2\":\n  id: \"uuid-2\"\n  \"1st col\": 2.5\nposts_3:\n  name: \"no id\"\n"
    );
    let out = sql("round-trip", &[("db/fixtures/posts.yml", &yaml)]);
    assert!(out.contains("VALUES (1, 'It''s \"quoted\"\nnext: line', NULL);\n"), "{out}");
    assert!(out.contains("(\"id\", \"1st col\") VALUES ('uuid-2', 2.5);\n"), "{out}");
    assert!(out.contains(&format!("(\"id\", \"name\") VALUES ({}, 'no id');\n", identify("posts_3"))), "{out}");
}

#[test]
fn foreign_keys_come_from_the_migrations() {
    let root = app(
        "keys",
        &[("migrations/0001_create_posts.sql", "CREATE TABLE posts (author_id INT);"), ("migrations/.gitkeep", "")],
    );
    assert_eq!(foreign_keys(&root).unwrap(), HashSet::from(["author_id".to_owned()]));
    fs::remove_dir_all(root.join("migrations")).unwrap();
    assert!(foreign_keys(&root).unwrap().is_empty(), "no migrations directory");
}

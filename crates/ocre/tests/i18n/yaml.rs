use super::*;

fn parse(text: &str) -> Result<Vec<(String, String)>, ParseError> {
    parse_locale("fr", text).map(|entries| entries.into_iter().map(|(k, v)| (k, v.into_owned())).collect())
}

fn error(text: &str) -> (usize, String) {
    let err = parse(text).unwrap_err();
    (err.line, err.message)
}

#[test]
fn reads_nested_mappings_of_strings() {
    let text = "\u{feff}---\n# Français\nfr:\n  posts:\n    title: Articles   # comment\n\n    created: \"Créé \\\"ok\\\"\\n\\t\\\\ \\/ \\u00e9\"  # c\r\n    quote: 'l''article'\n    plain: 'simple' # c\n  count: 3\n";
    assert_eq!(
        parse(text).unwrap(),
        [
            ("posts.title".to_owned(), "Articles".to_owned()),
            ("posts.created".to_owned(), "Créé \"ok\"\n\t\\ / é".to_owned()),
            ("posts.quote".to_owned(), "l'article".to_owned()),
            ("posts.plain".to_owned(), "simple".to_owned()),
            ("count".to_owned(), "3".to_owned()),
        ]
    );
}

#[test]
fn unescaped_values_borrow_the_file() {
    let entries = parse_locale("fr", "fr:\n  a: \"x\"\n  b: 'y'\n  c: z\n").unwrap();
    assert!(entries.iter().all(|(_, value)| matches!(value, Cow::Borrowed(_))));
}

#[test]
fn an_empty_locale_is_valid() {
    assert_eq!(parse("fr:\n").unwrap(), []);
    assert_eq!(parse("fr: # to translate\n").unwrap(), []);
}

#[test]
fn root_errors() {
    assert_eq!(error(""), (1, "the file must start with `fr:` on its own line".to_owned()));
    assert_eq!(error("# only a comment\n"), (1, "the file must start with `fr:` on its own line".to_owned()));
    assert_eq!(error("en:\n  a: b\n"), (1, "the file must start with `fr:` on its own line".to_owned()));
    assert_eq!(error("fr: text\n"), (1, "the file must start with `fr:` on its own line".to_owned()));
    assert_eq!(error("  fr:\n"), (1, "`fr:` must start the line".to_owned()));
    assert_eq!(error("fr:\n  a: b\nx: y\n"), (3, "only one root key, `fr:`; indent `x` under it".to_owned()));
}

#[test]
fn structure_errors() {
    assert_eq!(error("fr:\n\ta: b\n").1, "indent with spaces, not tabs");
    assert_eq!(error("fr:\n  - a\n").1, "lists are not supported: use one key per string");
    assert_eq!(error("fr:\n  -\n").1, "lists are not supported: use one key per string");
    assert_eq!(error("fr:\n  just text\n"), (2, "expected `key: value` or `key:`".to_owned()));
    assert_eq!(error("fr:\n  a.b: c\n").1, "expected `key: value` or `key:`");
    assert_eq!(error("fr:\n  a:b\n").1, "expected `key: value` or `key:`");
    assert_eq!(error("fr:\n  : b\n").1, "expected `key: value` or `key:`");
    assert_eq!(
        error("fr:\n  a:\n    b: c\n   d: e\n"),
        (4, "inconsistent indentation: siblings of this key are indented 4 spaces".to_owned())
    );
    assert_eq!(
        error("fr:\n  a: x\n    b: y\n").1,
        "inconsistent indentation: siblings of this key are indented 2 spaces"
    );
    assert_eq!(error("fr:\n  a: x\n  a: y\n"), (3, "duplicate key `a`".to_owned()));
    assert_eq!(error("fr:\n  a: x\n  a:\n    b: y\n").1, "duplicate key `a`");
    assert_eq!(
        error("fr:\n  a:\n  b: c\n"),
        (2, "`a` has no value: write `a: \"text\"`, or indent keys under it".to_owned())
    );
    assert_eq!(error("fr:\n  b: c\n  a:\n").0, 3, "an empty mapping at the end of the file");
}

#[test]
fn value_errors() {
    let message = |value: &str| error(&format!("fr:\n  a: {value}\n")).1;
    assert!(message("|").starts_with("block scalars"));
    assert!(message(">").starts_with("block scalars"));
    assert_eq!(message("%{count} posts"), "quote values starting with `%`, e.g. \"%{count} posts\"");
    assert_eq!(message("[a]"), "quote values starting with `[`, e.g. \"[a]\"");
    assert_eq!(message("Note: this"), "quote values containing `: ` or ending with `:`, e.g. \"Note: this\"");
    assert_eq!(message("Note:"), "quote values containing `: ` or ending with `:`, e.g. \"Note:\"");
    assert_eq!(message("Yes"), "YAML does not read `Yes` as text: quote it, \"Yes\"");
    assert_eq!(message("~"), "YAML does not read `~` as text: quote it, \"~\"");
    assert_eq!(message("\"open"), "close the double quote on the same line");
    assert_eq!(message("'open"), "close the single quote on the same line");
    assert_eq!(message("\"a\" b"), "unexpected `b` after the closing quote");
    assert_eq!(message("'a' b"), "unexpected `b` after the closing quote");
    assert_eq!(message("\"\\x\""), "unknown escape `\\x`: use \\n, \\t, \\\", \\\\ or \\uXXXX");
    assert_eq!(message("\"\\"), "unknown escape `\\`: use \\n, \\t, \\\", \\\\ or \\uXXXX");
    assert_eq!(message("\"\\u12\""), "`\\u12\"` is not a character: use \\u and 4 hex digits");
    assert_eq!(message("\"\\ud800\""), "`\\ud800` is not a character: use \\u and 4 hex digits");
}

use serde::Deserialize;

use super::*;

#[derive(Debug, Deserialize, PartialEq)]
struct New {
    #[serde(default, deserialize_with = "optional")]
    pages: Option<i64>,
    #[serde(default, deserialize_with = "optional")]
    note: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Changes {
    #[serde(default, deserialize_with = "patch")]
    pages: Option<Option<i64>>,
}

#[test]
fn optional_reads_forms() {
    let parse = |query: &str| serde_urlencoded::from_str::<New>(query);
    assert_eq!(parse("pages=12&note=hi").unwrap(), New { pages: Some(12), note: Some("hi".into()) });
    assert_eq!(parse("pages=&note=").unwrap(), New { pages: None, note: None });
    assert_eq!(parse("").unwrap(), New { pages: None, note: None });
    assert!(parse("pages=many").is_err());
}

#[test]
fn optional_reads_json() {
    let parse = |json: &str| serde_json::from_str::<New>(json).unwrap();
    assert_eq!(parse(r#"{"pages": 3, "note": "x"}"#), New { pages: Some(3), note: Some("x".into()) });
    assert_eq!(parse(r#"{"pages": null, "note": null}"#), New { pages: None, note: None });
    assert_eq!(parse(r#"{"pages": "7"}"#), New { pages: Some(7), note: None });
}

#[test]
fn patch_tells_missing_from_cleared() {
    let parse = |json: &str| serde_json::from_str::<Changes>(json).unwrap();
    assert_eq!(parse("{}"), Changes { pages: None });
    assert_eq!(parse(r#"{"pages": null}"#), Changes { pages: Some(None) });
    assert_eq!(parse(r#"{"pages": 4}"#), Changes { pages: Some(Some(4)) });
    assert_eq!(serde_urlencoded::from_str::<Changes>("pages=").unwrap(), Changes { pages: Some(None) });
}

#[derive(Debug, Deserialize, PartialEq)]
struct JsonChanges {
    #[serde(default, deserialize_with = "patch_json")]
    metadata: Option<Option<serde_json::Value>>,
}

#[test]
fn patch_json_keeps_strings_as_json_strings() {
    let parse = |json: &str| serde_json::from_str::<JsonChanges>(json).unwrap();
    assert_eq!(parse("{}"), JsonChanges { metadata: None });
    assert_eq!(parse(r#"{"metadata": null}"#), JsonChanges { metadata: Some(None) });
    assert_eq!(parse(r#"{"metadata": "{}"}"#), JsonChanges { metadata: Some(Some("{}".into())) });
    assert_eq!(
        parse(r#"{"metadata": {"a": [1]}}"#),
        JsonChanges { metadata: Some(Some(serde_json::json!({"a": [1]}))) }
    );
}

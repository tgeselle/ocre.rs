use serde::Serialize;
use serde_json::json;

use super::*;

#[derive(Serialize)]
struct Track {
    title: String,
    plays: i64,
}

fn text(stmt: &Statement) -> String {
    match &stmt.params[0].0 {
        crate::sql::Value::Text(text) => text.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn rows_go_in_one_json_parameter() {
    let tracks = [Track { title: "A".into(), plays: 1 }, Track { title: "B".into(), plays: 2 }];
    let insert = insert("tracks", &["title", "plays"], &tracks).unwrap();
    assert_eq!(text(&insert), r#"[{"plays":1,"title":"A"},{"plays":2,"title":"B"}]"#);
    let upserted = upsert("tracks", "title", &["title", "plays"], &tracks).unwrap();
    assert_eq!(
        upserted.sql,
        "INSERT INTO tracks (title, plays) SELECT value ->> '$.title', value ->> '$.plays' FROM json_each(?1) \
         WHERE true ON CONFLICT (title) DO UPDATE SET plays = excluded.plays"
    );
    let only_key = upsert("tags", "name", &["name"], &[json!({"name": "rust"})]).unwrap();
    assert!(only_key.sql.ends_with("ON CONFLICT (name) DO NOTHING"), "{}", only_key.sql);
    let touched = update("tracks", "id", &["plays"], &[json!({"id": 1, "plays": 3})], true).unwrap();
    assert!(
        touched.sql.contains("plays = row.value ->> '$.plays', updated_at = datetime('now') FROM"),
        "{}",
        touched.sql
    );
    let update = update("tracks", "id", &["plays"], &[json!({"id": 1, "plays": 3})], false).unwrap();
    assert_eq!(
        update.sql,
        "UPDATE tracks SET plays = row.value ->> '$.plays' FROM json_each(?1) AS row WHERE tracks.id = row.value ->> '$.id'"
    );
}

#[test]
fn names_rows_and_keys_are_checked() {
    let message = |err: Error| err.to_string();
    let rows = [json!({"a": 1})];
    assert!(message(insert("t; DROP", &["a"], &rows).unwrap_err()).contains("`t; DROP` is not a plain SQL identifier"));
    assert!(message(insert("t", &["a b"], &rows).unwrap_err()).contains("`a b` is not a plain SQL identifier"));
    assert!(message(insert("t", &[], &rows).unwrap_err()).contains("no columns to write"));
    assert!(message(insert("t", &["a"], &[1, 2]).unwrap_err()).contains("must serialize to a JSON object"));
    assert!(message(upsert("t", "id", &["a"], &rows).unwrap_err()).contains("the key `id` must be one of the columns"));
    assert!(
        message(update("t", "1d", &["a"], &rows, true).unwrap_err()).contains("`1d` is not a plain SQL identifier")
    );
    let tuple_keys = [std::collections::BTreeMap::from([((1, 2), 3)])];
    assert!(
        message(insert("t", &["a"], &tuple_keys).unwrap_err()).contains("rows do not serialize: key must be a string")
    );
    assert!(insert("_t2", &["a_1"], &rows).is_ok());
}

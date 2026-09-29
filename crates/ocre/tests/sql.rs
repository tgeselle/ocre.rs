use serde::Deserialize;

use super::*;
use crate::params;

fn text(s: &str) -> Param {
    Param(Value::Text(s.to_owned()))
}

fn number(n: f64) -> Param {
    Param(Value::Number(n))
}

#[test]
fn strings_bind_as_text() {
    let owned = String::from("b");
    assert_eq!(params!["a", owned.clone(), &owned], vec![text("a"), text("b"), text("b")]);
}

#[test]
fn booleans_bind_as_integers() {
    assert_eq!(params![true, false], vec![number(1.0), number(0.0)]);
}

#[test]
fn small_integers_and_floats_bind_as_numbers() {
    assert_eq!(
        params![-1i8, 2i16, 3i32, 4u8, 5u16, 6u32, 1.5f64],
        vec![number(-1.0), number(2.0), number(3.0), number(4.0), number(5.0), number(6.0), number(1.5)]
    );
}

#[test]
fn i64_is_a_number_up_to_the_js_safe_limit_then_text() {
    assert_eq!(MAX_SAFE_INTEGER.into_param(), number(9_007_199_254_740_991.0));
    assert_eq!((-MAX_SAFE_INTEGER).into_param(), number(-9_007_199_254_740_991.0));
    assert_eq!((MAX_SAFE_INTEGER + 1).into_param(), text("9007199254740992"));
    assert_eq!((-MAX_SAFE_INTEGER - 1).into_param(), text("-9007199254740992"));
    assert_eq!(i64::MAX.into_param(), text("9223372036854775807"));
}

#[test]
fn options_bind_null_or_the_inner_value() {
    assert_eq!(
        params![None::<i32>, Some("x"), Param(Value::Null)],
        vec![Param(Value::Null), text("x"), Param(Value::Null)]
    );
}

#[test]
fn empty_params_macro() {
    let empty: Vec<Param> = params![];
    assert!(empty.is_empty());
}

#[derive(Deserialize)]
struct Row {
    #[serde(deserialize_with = "bool_from_sql")]
    flag: bool,
}

fn row(json: &str) -> Result<bool, serde_json::Error> {
    serde_json::from_str::<Row>(json).map(|r| r.flag)
}

#[test]
fn bool_from_sql_accepts_booleans_and_numbers() {
    assert!(row(r#"{"flag": true}"#).unwrap());
    assert!(!row(r#"{"flag": false}"#).unwrap());
    assert!(row(r#"{"flag": 1}"#).unwrap());
    assert!(!row(r#"{"flag": 0}"#).unwrap());
    assert!(row(r#"{"flag": -1}"#).unwrap());
    assert!(row(r#"{"flag": 1.0}"#).unwrap());
    assert!(!row(r#"{"flag": 0.0}"#).unwrap());
}

#[test]
fn bool_from_sql_rejects_other_types() {
    let err = row(r#"{"flag": "yes"}"#).unwrap_err();
    assert!(err.to_string().contains("a boolean or the integer 0 or 1"), "{err}");
}

#[test]
fn statements_carry_sql_and_params() {
    let statement = Statement::new("DELETE FROM posts WHERE id = ?1", params![3]);
    assert_eq!(statement.sql, "DELETE FROM posts WHERE id = ?1");
    assert_eq!(statement.params, vec![number(3.0)]);
}

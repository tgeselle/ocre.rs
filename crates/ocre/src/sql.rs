//! Query parameters and SQLite value conversions. Pure Rust: no JavaScript
//! values are created until a statement is bound (see `runtime::d1`).

/// A value bound to a `?N` placeholder. Build with [`params!`](crate::params).
#[derive(Debug, Clone, PartialEq)]
pub struct Param(pub(crate) Value);

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Number(f64),
    Text(String),
}

/// Types that can be bound as query parameters.
pub trait IntoParam {
    fn into_param(self) -> Param;
}

impl IntoParam for Param {
    fn into_param(self) -> Param {
        self
    }
}

impl IntoParam for &str {
    fn into_param(self) -> Param {
        Param(Value::Text(self.to_owned()))
    }
}

impl IntoParam for String {
    fn into_param(self) -> Param {
        Param(Value::Text(self))
    }
}

impl IntoParam for &String {
    fn into_param(self) -> Param {
        Param(Value::Text(self.clone()))
    }
}

/// SQLite has no boolean type: `true`/`false` are stored as INTEGER 1/0.
/// Read them back with `#[serde(deserialize_with = "ocre::bool_from_sql")]`.
impl IntoParam for bool {
    fn into_param(self) -> Param {
        Param(Value::Number(if self { 1.0 } else { 0.0 }))
    }
}

impl IntoParam for f64 {
    fn into_param(self) -> Param {
        Param(Value::Number(self))
    }
}

macro_rules! lossless_number_param {
    ($($ty:ty),*) => {$(
        impl IntoParam for $ty {
            fn into_param(self) -> Param {
                Param(Value::Number(f64::from(self)))
            }
        }
    )*};
}

lossless_number_param!(i8, i16, i32, u8, u16, u32);

/// Largest integer a JavaScript number (and so D1) represents exactly: 2^53 - 1.
/// D1 returns INTEGER columns as JavaScript numbers, so rows holding larger
/// values cannot be read back into `i64`: validate input against this bound.
pub const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// Integers within ±(2^53 - 1) are bound as numbers. Larger values are bound
/// as decimal text, exact for comparisons in `WHERE` clauses.
impl IntoParam for i64 {
    fn into_param(self) -> Param {
        if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&self) {
            Param(Value::Number(self as f64))
        } else {
            Param(Value::Text(self.to_string()))
        }
    }
}

impl<T: IntoParam> IntoParam for Option<T> {
    fn into_param(self) -> Param {
        self.map_or(Param(Value::Null), IntoParam::into_param)
    }
}

/// Deserializes a SQLite boolean column (INTEGER 0/1) into `bool`:
/// `#[serde(deserialize_with = "ocre::bool_from_sql")]`.
pub fn bool_from_sql<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    struct Visitor;

    impl serde::de::Visitor<'_> for Visitor {
        type Value = bool;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a boolean or the integer 0 or 1")
        }

        fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<bool, E> {
            Ok(v)
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<bool, E> {
            Ok(v != 0)
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<bool, E> {
            Ok(v != 0)
        }

        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<bool, E> {
            Ok(v != 0.0)
        }
    }

    deserializer.deserialize_any(Visitor)
}

#[cfg(test)]
mod tests {
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
}

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

/// One SQL statement with its parameters, for [`Db::batch`](crate::Db::batch).
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub sql: String,
    pub params: Vec<Param>,
}

impl Statement {
    pub fn new(sql: impl Into<String>, params: Vec<Param>) -> Self {
        Self { sql: sql.into(), params }
    }
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
#[path = "../tests/sql.rs"]
mod tests;

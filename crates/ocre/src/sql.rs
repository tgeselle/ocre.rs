//! Query parameters and SQLite value conversions. Pure Rust: no JavaScript
//! values are created until a statement is bound (see `runtime::d1`).

/// A value bound to a `?N` placeholder of a D1 query.
///
/// Build a list with [`params!`](crate::params), which calls
/// [`IntoParam::into_param`] on each value, in placeholder order: the first
/// value binds `?1`. A `Param` is a SQLite `NULL`, number or text; no
/// JavaScript value exists until the statement is bound, so building one is
/// pure Rust and compares with `==`.
///
/// # Examples
///
/// ```
/// use ocre::{IntoParam, params};
///
/// let title = String::from("Hello");
/// let params = params![&title, 42, true, None::<i64>];
/// assert_eq!(params.len(), 4);
/// assert_eq!(params[0], "Hello".into_param());
/// assert_eq!(params[2], 1_i32.into_param()); // booleans are INTEGER 1/0
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Param(pub(crate) Value);

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Number(f64),
    Text(String),
}

/// One SQL statement with its parameters, for [`Db::batch`](crate::Db::batch).
///
/// # Examples
///
/// ```
/// use ocre::{Statement, params};
///
/// let stmt = Statement::new("DELETE FROM posts WHERE id = ?1", params![7]);
/// assert_eq!(stmt.sql, "DELETE FROM posts WHERE id = ?1");
/// assert_eq!(stmt.params, params![7]);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    /// SQL text with `?1, ?2...` placeholders.
    pub sql: String,
    /// Values for the placeholders, in order (build with [`params!`](crate::params)).
    pub params: Vec<Param>,
}

impl Statement {
    /// Pairs `sql` with its `params`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Statement, params};
    ///
    /// let stmt = Statement::new("UPDATE posts SET title = ?1 WHERE id = ?2", params!["New", 3]);
    /// assert_eq!(stmt.params.len(), 2);
    /// ```
    pub fn new(sql: impl Into<String>, params: Vec<Param>) -> Self {
        Self { sql: sql.into(), params }
    }
}

/// Types that can be bound as D1 query parameters.
///
/// Implemented for text (`&str`, `String`, `&String`), `bool` (INTEGER 1/0),
/// `f64`, integers up to 32 bits, `i64` (see [`MAX_SAFE_INTEGER`]),
/// [`serde_json::Value`] (stored as compact JSON text), `Option<T>` (`None` is
/// `NULL`) and [`Param`] itself. [`params!`](crate::params) calls it on each
/// value; implement it for your own types (e.g. an enum stored as text) by
/// delegating to one of these.
///
/// # Examples
///
/// ```
/// use ocre::{IntoParam, Param, params};
///
/// enum Status {
///     Draft,
///     Published,
/// }
///
/// impl IntoParam for Status {
///     fn into_param(self) -> Param {
///         match self {
///             Status::Draft => "draft",
///             Status::Published => "published",
///         }
///         .into_param()
///     }
/// }
///
/// assert_eq!(params![Status::Published], params!["published"]);
/// # let _ = Status::Draft;
/// ```
pub trait IntoParam {
    /// Converts the value into a bound parameter.
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
///
/// D1 returns INTEGER columns as JavaScript numbers, so rows holding larger
/// values cannot be read back into `i64`: validate input against this bound
/// with [`Validator::safe_integer`](crate::Validator::safe_integer). Binding a
/// larger `i64` sends it as decimal text instead of a rounded number.
///
/// # Examples
///
/// ```
/// use ocre::{IntoParam, MAX_SAFE_INTEGER};
///
/// assert_eq!(MAX_SAFE_INTEGER, 9_007_199_254_740_991);
/// let too_big = MAX_SAFE_INTEGER + 1;
/// assert_eq!(too_big.into_param(), too_big.to_string().into_param());
/// ```
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

/// JSON values are stored as their compact text in a `TEXT` column (the
/// generators add `CHECK (json_valid(<column>))`). Read them back with
/// `#[serde(deserialize_with = "ocre::json_from_sql")]`.
impl IntoParam for serde_json::Value {
    fn into_param(self) -> Param {
        Param(Value::Text(self.to_string()))
    }
}

/// Deserializes a SQLite boolean column (INTEGER 0/1) into `bool`.
///
/// Use it as `#[serde(deserialize_with = "ocre::bool_from_sql")]`. Any
/// non-zero number is `true`; real booleans are accepted too, so the same
/// struct also reads JSON bodies and cached rows.
///
/// # Errors
///
/// Fails with the deserializer's error for anything but a boolean or a number
/// ("expected a boolean or the integer 0 or 1"); [`Db`](crate::Db) queries
/// report it as [`Error::Internal`](crate::Error::Internal).
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Post {
///     #[serde(deserialize_with = "ocre::bool_from_sql")]
///     published: bool,
/// }
///
/// let post: Post = serde_json::from_str(r#"{"published": 1}"#).unwrap();
/// assert!(post.published);
/// let post: Post = serde_json::from_str(r#"{"published": false}"#).unwrap();
/// assert!(!post.published);
/// assert!(serde_json::from_str::<Post>(r#"{"published": "yes"}"#).is_err());
/// ```
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

/// Deserializes a JSON column (the JSON text D1 returns) into [`serde_json::Value`].
///
/// Use it as `#[serde(deserialize_with = "ocre::json_from_sql")]` on a
/// `serde_json::Value` field. A string is parsed as JSON text; a value that is
/// not a string (e.g. a row cached as JSON) is taken as is.
///
/// # Errors
///
/// Fails with the deserializer's error when the text is not valid JSON;
/// [`Db`](crate::Db) queries report it as [`Error::Internal`](crate::Error::Internal).
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Post {
///     #[serde(deserialize_with = "ocre::json_from_sql")]
///     metadata: serde_json::Value,
/// }
///
/// // As D1 returns it: JSON text in a TEXT column.
/// let post: Post = serde_json::from_str(r#"{"metadata": "{\"tags\":[\"rust\"]}"}"#).unwrap();
/// assert_eq!(post.metadata["tags"][0], "rust");
/// // Already parsed (a cached row): kept as is.
/// let post: Post = serde_json::from_str(r#"{"metadata": {"tags": []}}"#).unwrap();
/// assert_eq!(post.metadata, serde_json::json!({"tags": []}));
/// ```
pub fn json_from_sql<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<serde_json::Value, D::Error> {
    parse_json_text(serde::Deserialize::deserialize(deserializer)?)
}

/// Like [`json_from_sql`] for a `NULL`-able JSON column: `NULL` is `None`.
///
/// Use it as `#[serde(default, deserialize_with = "ocre::optional_json_from_sql")]`
/// on an `Option<serde_json::Value>` field (`default` also accepts a missing key).
///
/// # Errors
///
/// Fails with the deserializer's error when the text is not valid JSON;
/// [`Db`](crate::Db) queries report it as [`Error::Internal`](crate::Error::Internal).
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Post {
///     #[serde(default, deserialize_with = "ocre::optional_json_from_sql")]
///     metadata: Option<serde_json::Value>,
/// }
///
/// let post: Post = serde_json::from_str(r#"{"metadata": null}"#).unwrap();
/// assert_eq!(post.metadata, None);
/// let post: Post = serde_json::from_str(r#"{"metadata": "[1, 2]"}"#).unwrap();
/// assert_eq!(post.metadata, Some(serde_json::json!([1, 2])));
/// ```
pub fn optional_json_from_sql<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    <Option<serde_json::Value> as serde::Deserialize>::deserialize(deserializer)?.map(parse_json_text).transpose()
}

fn parse_json_text<E: serde::de::Error>(value: serde_json::Value) -> Result<serde_json::Value, E> {
    match value {
        serde_json::Value::String(text) => serde_json::from_str(&text).map_err(E::custom),
        other => Ok(other),
    }
}

#[cfg(test)]
#[path = "../tests/sql.rs"]
mod tests;

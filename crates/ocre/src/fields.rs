//! Serde helpers for optional (`NULL`-able) fields in forms and JSON bodies.
//!
//! HTML forms send every field as a string, and an empty input means "no
//! value". JSON bodies send `null` or a typed value. These helpers accept both.

use std::{fmt::Display, str::FromStr};

use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
#[serde(untagged)]
enum Raw<T> {
    Text(String),
    Value(T),
}

fn parse<T, E>(raw: Option<Raw<T>>) -> Result<Option<T>, E>
where
    T: FromStr,
    T::Err: Display,
    E: serde::de::Error,
{
    match raw {
        None => Ok(None),
        Some(Raw::Value(value)) => Ok(Some(value)),
        Some(Raw::Text(text)) if text.trim().is_empty() => Ok(None),
        Some(Raw::Text(text)) => text.parse().map(Some).map_err(E::custom),
    }
}

/// Deserializes an optional field where `null`, a missing value or an empty string is `None`.
///
/// Use on `Option<T>` fields with
/// `#[serde(default, deserialize_with = "ocre::optional")]` (`default` makes a
/// missing key `None`). A typed value (`42`) is taken as is; a blank string
/// (only whitespace) is `None`; any other string (`"42"`, as HTML forms send)
/// is parsed with `T::from_str`.
///
/// # Errors
///
/// Fails with the deserializer's error, carrying `T::Err`'s message, when a
/// non-empty string does not parse. Behind [`Json`](crate::Json) that is a JSON 400.
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Book {
///     #[serde(default, deserialize_with = "ocre::optional")]
///     pages: Option<i64>,
/// }
///
/// let parse = |json| serde_json::from_str::<Book>(json).map(|book| book.pages);
/// assert_eq!(parse(r#"{"pages": 320}"#).unwrap(), Some(320));
/// assert_eq!(parse(r#"{"pages": "320"}"#).unwrap(), Some(320));
/// assert_eq!(parse(r#"{"pages": ""}"#).unwrap(), None);
/// assert_eq!(parse(r#"{"pages": null}"#).unwrap(), None);
/// assert_eq!(parse("{}").unwrap(), None);
/// assert!(parse(r#"{"pages": "many"}"#).is_err());
/// ```
pub fn optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + FromStr,
    T::Err: Display,
{
    parse(Option::<Raw<T>>::deserialize(deserializer)?)
}

/// Deserializes a field of a partial update (PATCH) into keep / clear / set.
///
/// Use on `Option<Option<T>>` fields with
/// `#[serde(default, deserialize_with = "ocre::patch")]`: a missing field is
/// `None` (keep the value, thanks to `default`), `null` or an empty string is
/// `Some(None)` (clear it), a value is `Some(Some(value))`. Strings are parsed
/// with `T::from_str`, as in [`optional`].
///
/// # Errors
///
/// Fails with the deserializer's error, carrying `T::Err`'s message, when a
/// non-empty string does not parse.
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct BookChanges {
///     #[serde(default, deserialize_with = "ocre::patch")]
///     pages: Option<Option<i64>>,
/// }
///
/// let parse = |json| serde_json::from_str::<BookChanges>(json).unwrap().pages;
/// assert_eq!(parse("{}"), None); // keep
/// assert_eq!(parse(r#"{"pages": null}"#), Some(None)); // clear
/// assert_eq!(parse(r#"{"pages": ""}"#), Some(None)); // clear (HTML form)
/// assert_eq!(parse(r#"{"pages": 12}"#), Some(Some(12))); // set
/// ```
pub fn patch<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + FromStr,
    T::Err: Display,
{
    parse::<T, D::Error>(Option::<Raw<T>>::deserialize(deserializer)?).map(Some)
}

/// Deserializes an optional JSON field of a partial update into keep / clear / set.
///
/// [`patch`] for `Option<Option<serde_json::Value>>` fields: a missing field
/// is `None`, `null` is `Some(None)`, any other JSON value (strings included,
/// taken as JSON strings, not parsed) is `Some(Some(value))`. Use with
/// `#[serde(default, deserialize_with = "ocre::patch_json")]`.
///
/// # Errors
///
/// Only the deserializer's own errors (malformed input); every JSON value is accepted.
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
/// use serde_json::json;
///
/// #[derive(Deserialize)]
/// struct PostChanges {
///     #[serde(default, deserialize_with = "ocre::patch_json")]
///     metadata: Option<Option<serde_json::Value>>,
/// }
///
/// let parse = |json| serde_json::from_str::<PostChanges>(json).unwrap().metadata;
/// assert_eq!(parse("{}"), None);
/// assert_eq!(parse(r#"{"metadata": null}"#), Some(None));
/// assert_eq!(parse(r#"{"metadata": {"a": 1}}"#), Some(Some(json!({"a": 1}))));
/// assert_eq!(parse(r#"{"metadata": "{}"}"#), Some(Some(json!("{}"))));
/// ```
pub fn patch_json<'de, D>(deserializer: D) -> Result<Option<Option<serde_json::Value>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<serde_json::Value>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
#[path = "../tests/fields.rs"]
mod tests;

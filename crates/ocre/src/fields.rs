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

/// For `Option<T>` fields: `null`, a missing value or an empty string is
/// `None`. Use with `#[serde(default, deserialize_with = "ocre::optional")]`.
pub fn optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + FromStr,
    T::Err: Display,
{
    parse(Option::<Raw<T>>::deserialize(deserializer)?)
}

/// For `Option<Option<T>>` fields of partial updates (PATCH): a missing field
/// is `None` (keep the value, thanks to `#[serde(default)]`), `null` or an
/// empty string is `Some(None)` (clear it), a value is `Some(Some(value))`.
/// Use with `#[serde(default, deserialize_with = "ocre::patch")]`.
pub fn patch<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + FromStr,
    T::Err: Display,
{
    parse::<T, D::Error>(Option::<Raw<T>>::deserialize(deserializer)?).map(Some)
}

#[cfg(test)]
mod tests;

//! Typed app configuration from Worker variables and secrets, and the environment (development or production).
//!
//! An Ocre app has no config files at run time: its settings are the
//! plain-text variables of cloudflare.config.ts (`KEY: bindings.text("...")`)
//! and its secrets (`ocre secrets push`), with `.dev.vars` overriding both
//! in `ocre dev`. Declare them once as a struct and read it with
//! [`Ctx::config`](crate::Ctx::config) (Loco's `settings`, Rails'
//! `config.x` and credentials):
//!
//! ```no_run
//! use axum::extract::State;
//! use ocre::{Ctx, Result};
//! use serde::Deserialize;
//!
//! /// Field `stripe_secret_key` reads `STRIPE_SECRET_KEY`.
//! #[derive(Deserialize)]
//! struct Settings {
//!     stripe_secret_key: String,
//!     #[serde(default = "default_page_size")]
//!     page_size: u32,
//!     support_email: Option<String>,
//!     beta_features: Vec<String>, // "search,exports"
//! }
//!
//! fn default_page_size() -> u32 {
//!     25
//! }
//!
//! async fn checkout(State(ctx): State<Ctx>) -> Result<String> {
//!     let settings: Settings = ctx.config()?;
//!     Ok(format!("{} per page", settings.page_size))
//! }
//! # let _ = checkout;
//! ```
//!
//! Each field reads the variable or secret named like the field in upper
//! case (`#[serde(rename = "...")]` names it exactly, before upper-casing).
//! Values are text, converted to the field's type: numbers, `bool`
//! (`true`/`false`/`1`/`0`), `char`, `String`, `Vec<_>` from a
//! comma-separated list, and unit enums by name. `Option` fields and
//! `#[serde(default)]` fields may be missing; a missing required field is
//! [`Error::Internal`] naming the variable and the
//! two places to set it.
//!
//! # Free plan
//!
//! Reading costs no request or binding operation: one environment lookup
//! per field (microseconds of CPU). The free plan allows 64 variables and
//! secrets per Worker, 5 KB each (September 2026).

use serde::de::{self, DeserializeOwned, IntoDeserializer, Visitor};

use crate::Error;

/// Where the Worker runs, from its build profile (Rails' `Rails.env`, Loco's environment).
///
/// `ocre dev` and `cargo test` build with debug assertions
/// ([`Development`](Self::Development)); `ocre deploy` builds in release mode
/// ([`Production`](Self::Production)). Rails' `test` environment is
/// `Development` here: tests run the debug build natively. Values that
/// differ per environment go in `.dev.vars` (development) and
/// cloudflare.config.ts or secrets (production), read with
/// [`Ctx::config`](crate::Ctx::config).
///
/// # Examples
///
/// ```
/// use ocre::config::Environment;
///
/// let env = Environment::current();
/// assert_eq!(env.is_development(), cfg!(debug_assertions));
/// assert_eq!(Environment::Production.as_str(), "production");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    /// Debug build: `ocre dev`, `cargo test`, `ocre test`.
    Development,
    /// Release build: `ocre deploy`.
    Production,
}

impl Environment {
    /// The environment of this build.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(ocre::config::Environment::current().is_development() || cfg!(not(debug_assertions)));
    /// ```
    pub fn current() -> Self {
        if cfg!(debug_assertions) { Self::Development } else { Self::Production }
    }

    /// `development` or `production`, as sent to error reporters.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::config::Environment::Development.as_str(), "development");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Production => "production",
        }
    }

    /// Whether this is [`Development`](Self::Development).
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(!ocre::config::Environment::Production.is_development());
    /// ```
    pub fn is_development(self) -> bool {
        self == Self::Development
    }
}

/// Reads `T` from the given variables: what [`Ctx::config`](crate::Ctx::config) does with the Worker's environment.
///
/// Use it in unit tests of code that takes the settings struct.
///
/// # Errors
///
/// [`Error::Internal`] when a required variable is missing or does not
/// convert to its field's type; the message names the variable and the fix.
///
/// # Examples
///
/// ```
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize, PartialEq)]
/// struct Settings {
///     page_size: u32,
///     admins: Vec<String>,
///     debug_toolbar: Option<bool>,
/// }
///
/// let settings: Settings = ocre::config::from_vars([("PAGE_SIZE", "20"), ("ADMINS", "ada, grace")]).unwrap();
/// assert_eq!(settings, Settings { page_size: 20, admins: vec!["ada".into(), "grace".into()], debug_toolbar: None });
///
/// let err = ocre::config::from_vars::<Settings>([("PAGE_SIZE", "many")]).unwrap_err();
/// assert!(err.to_string().contains("`PAGE_SIZE`"));
/// ```
pub fn from_vars<'a, T: DeserializeOwned>(vars: impl IntoIterator<Item = (&'a str, &'a str)>) -> crate::Result<T> {
    let vars: Vec<(&str, &str)> = vars.into_iter().collect();
    from_lookup(&|name| vars.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned()))
}

/// Reads `T` with `lookup` (variable, then secret, by name); used by `Ctx::config`.
pub(crate) fn from_lookup<T: DeserializeOwned>(lookup: &dyn Fn(&str) -> Option<String>) -> crate::Result<T> {
    T::deserialize(Vars { lookup }).map_err(|err| Error::internal(err.0))
}

/// The text of a missing-variable error, naming both places to set it.
fn missing(name: &str) -> String {
    format!(
        "Worker variable or secret `{name}` is missing. Fix: add `{name}: bindings.text(\"...\"),` to worker.env in cloudflare.config.ts, or for a secret run `ocre secrets push {name} --file .prod.vars`; in `ocre dev`, add `{name}=...` to .dev.vars"
    )
}

/// A config error, turned into [`Error::Internal`].
#[derive(Debug)]
struct ConfigError(String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

impl de::Error for ConfigError {
    fn custom<M: std::fmt::Display>(message: M) -> Self {
        Self(message.to_string())
    }

    fn missing_field(field: &'static str) -> Self {
        Self(missing(&field.to_ascii_uppercase()))
    }
}

/// The whole environment: only structs can be read from it.
struct Vars<'a> {
    lookup: &'a dyn Fn(&str) -> Option<String>,
}

impl<'de> de::Deserializer<'de> for Vars<'_> {
    type Error = ConfigError;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, ConfigError> {
        Err(ConfigError("ocre::config reads a struct with named fields, one per variable".to_owned()))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ConfigError> {
        let entries: Vec<(&'static str, String)> = fields
            .iter()
            .filter_map(|field| {
                let name = field.to_ascii_uppercase();
                (self.lookup)(&name).map(|value| (*field, value))
            })
            .collect();
        visitor.visit_map(Fields { entries: entries.into_iter(), value: None })
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

struct Fields {
    entries: std::vec::IntoIter<(&'static str, String)>,
    value: Option<(&'static str, String)>,
}

impl<'de> de::MapAccess<'de> for Fields {
    type Error = ConfigError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, ConfigError> {
        let Some((field, value)) = self.entries.next() else { return Ok(None) };
        self.value = Some((field, value));
        seed.deserialize(field.into_deserializer()).map(Some)
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, ConfigError> {
        let (field, value) = self.value.take().expect("serde asks for a value after its key");
        let name = field.to_ascii_uppercase();
        seed.deserialize(Text { text: &value, name: &name })
    }
}

/// One variable's text, converted to the type the field asks for.
struct Text<'a> {
    text: &'a str,
    name: &'a str,
}

impl Text<'_> {
    fn parse<T: std::str::FromStr>(&self, kind: &str) -> Result<T, ConfigError> {
        self.text.trim().parse().map_err(|_| {
            ConfigError(format!(
                "Worker variable `{}` is `{}`, not {kind}. Fix: set it to {kind} in cloudflare.config.ts, the secret, or .dev.vars",
                self.name, self.text
            ))
        })
    }
}

macro_rules! parse_as {
    ($($method:ident => $visit:ident, $kind:literal;)*) => {
        $(fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ConfigError> {
            visitor.$visit(self.parse($kind)?)
        })*
    };
}

impl<'de> de::Deserializer<'de> for Text<'_> {
    type Error = ConfigError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ConfigError> {
        visitor.visit_string(self.text.to_owned())
    }

    parse_as! {
        deserialize_i8 => visit_i8, "an integer";
        deserialize_i16 => visit_i16, "an integer";
        deserialize_i32 => visit_i32, "an integer";
        deserialize_i64 => visit_i64, "an integer";
        deserialize_u8 => visit_u8, "a positive integer";
        deserialize_u16 => visit_u16, "a positive integer";
        deserialize_u32 => visit_u32, "a positive integer";
        deserialize_u64 => visit_u64, "a positive integer";
        deserialize_f32 => visit_f32, "a number";
        deserialize_f64 => visit_f64, "a number";
        deserialize_char => visit_char, "a single character";
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ConfigError> {
        match self.text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => visitor.visit_bool(true),
            "false" | "0" => visitor.visit_bool(false),
            _ => visitor.visit_bool(self.parse::<bool>("`true` or `false`")?),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ConfigError> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, ConfigError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ConfigError> {
        let name = self.name;
        let items = self.text.split(',').map(str::trim).filter(|item| !item.is_empty());
        let items: Vec<Text<'_>> = items.map(|text| Text { text, name }).collect();
        visitor.visit_seq(de::value::SeqDeserializer::new(items.into_iter()))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ConfigError> {
        visitor.visit_enum(self.text.trim().into_deserializer())
    }

    serde::forward_to_deserialize_any! {
        i128 u128 str string bytes byte_buf unit unit_struct tuple
        tuple_struct map struct identifier ignored_any
    }
}

impl<'de, 'a> IntoDeserializer<'de, ConfigError> for Text<'a> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

#[cfg(test)]
#[path = "../tests/config.rs"]
mod tests;

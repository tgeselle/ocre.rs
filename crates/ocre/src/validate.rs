//! Field validations with Rails-style messages.
//!
//! ```ignore
//! let mut v = Validator::new();
//! v.required("title", &input.title);
//! v.max_length("title", &input.title, 100);
//! v.range("pages", input.pages, 1..=10_000);
//! v.finish()?; // Error::Invalid with every failed field
//! ```

use std::{fmt, ops::RangeInclusive, str::FromStr};

use serde::Serialize;

use crate::{Error, MAX_SAFE_INTEGER, names::humanize};

/// One failed validation: the field name plus the message without it, as in Rails' `errors`.
///
/// [`Error::Invalid`] carries a list of them; JSON answers group the messages
/// by field, HTML pages show [`full_message`](Self::full_message). Serializes
/// as `{"field": "title", "message": "can't be blank"}`; `Display` writes the
/// full message.
///
/// # Examples
///
/// ```
/// use ocre::FieldError;
///
/// let error = FieldError::new("published_at", "is not a valid date");
/// assert_eq!(error.full_message(), "Published at is not a valid date");
/// assert_eq!(error.to_string(), error.full_message());
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldError {
    /// Field name as in the form or JSON body (`"title"`, `"author_id"`).
    pub field: String,
    /// Message without the field name (`"can't be blank"`).
    pub message: String,
}

impl FieldError {
    /// Builds an error for `field` with `message` (without the field name).
    ///
    /// # Examples
    ///
    /// ```
    /// let error = ocre::FieldError::new("title", "can't be blank");
    /// assert_eq!((error.field.as_str(), error.message.as_str()), ("title", "can't be blank"));
    /// ```
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self { field: field.into(), message: message.into() }
    }

    /// The message prefixed with the humanized field name: `"Title can't be blank"`.
    ///
    /// Like Rails' `full_messages`: underscores become spaces, a trailing
    /// `_id` is dropped and the first letter is capitalized.
    ///
    /// # Examples
    ///
    /// ```
    /// let error = ocre::FieldError::new("title", "can't be blank");
    /// assert_eq!(error.full_message(), "Title can't be blank");
    /// ```
    pub fn full_message(&self) -> String {
        format!("{} {}", humanize(&self.field), self.message)
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.full_message())
    }
}

/// Collects field errors with Rails-style messages; [`finish`](Self::finish) fails with all of them.
///
/// Every check adds at most one error per failed rule and returns `&mut Self`,
/// so checks chain. Nothing stops at the first failure: the user sees every
/// problem at once, as [`Error::Invalid`] (422). Pure Rust: no binding call,
/// no D1 rows. Uniqueness and foreign keys need the database; generated
/// models check them with [`Db::exists`](crate::Db::exists) and
/// [`check`](Self::check).
///
/// # Examples
///
/// ```
/// use ocre::{Error, Validator};
///
/// let mut v = Validator::new();
/// v.required("title", "  ").max_length("title", "  ", 100).range("pages", 0, 1..=10_000);
/// let Err(Error::Invalid(errors)) = v.finish() else { panic!("expected errors") };
/// let messages: Vec<String> = errors.iter().map(|e| e.full_message()).collect();
/// assert_eq!(messages, ["Title can't be blank", "Pages must be greater than or equal to 1"]);
/// ```
#[derive(Debug, Default)]
pub struct Validator {
    errors: Vec<FieldError>,
}

impl Validator {
    /// Creates a validator with no errors.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(ocre::Validator::new().is_valid());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `message` for `field` when `failed` is true: the building block for custom rules.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.check("ends_at", 10 < 5, "must be after the start").check("title", true, "is reserved");
    /// assert!(!v.is_valid());
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Title is reserved");
    /// ```
    pub fn check(&mut self, field: &str, failed: bool, message: impl Into<String>) -> &mut Self {
        if failed {
            self.errors.push(FieldError::new(field, message));
        }
        self
    }

    /// Checks that `value` is not empty after trimming whitespace ("can't be blank").
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.required("title", "Hello");
    /// assert!(v.is_valid());
    /// v.required("body", " \n");
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Body can't be blank");
    /// ```
    pub fn required(&mut self, field: &str, value: &str) -> &mut Self {
        self.check(field, value.trim().is_empty(), "can't be blank")
    }

    /// Checks that `value` has at most `max` characters (Unicode scalar values, not bytes).
    ///
    /// Message: "is too long (maximum is N characters)".
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.max_length("title", "été", 3);
    /// assert!(v.is_valid());
    /// v.max_length("title", "summer", 3);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Title is too long (maximum is 3 characters)");
    /// ```
    pub fn max_length(&mut self, field: &str, value: &str, max: usize) -> &mut Self {
        let too_long = value.chars().count() > max;
        self.check(field, too_long, format!("is too long (maximum is {max} characters)"))
    }

    /// Checks that `value` has at least `min` characters (Unicode scalar values, not bytes).
    ///
    /// Message: "is too short (minimum is N characters)".
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.min_length("password", "hunter2", 12);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Password is too short (minimum is 12 characters)");
    /// ```
    pub fn min_length(&mut self, field: &str, value: &str, min: usize) -> &mut Self {
        let too_short = value.chars().count() < min;
        self.check(field, too_short, format!("is too short (minimum is {min} characters)"))
    }

    /// Checks that `value` is within `range`, bounds included.
    ///
    /// Messages: "must be greater than or equal to MIN" or "must be less than
    /// or equal to MAX".
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.range("rating", 3, 1..=5).range("price", 12.5, 0.0..=10.0);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Price must be less than or equal to 10");
    /// ```
    pub fn range<T: PartialOrd + fmt::Display>(
        &mut self,
        field: &str,
        value: T,
        range: RangeInclusive<T>,
    ) -> &mut Self {
        let (below, above) = (value < *range.start(), value > *range.end());
        self.bounds(field, below, above, range.start(), range.end())
    }

    fn bounds(
        &mut self,
        field: &str,
        below: bool,
        above: bool,
        min: &dyn fmt::Display,
        max: &dyn fmt::Display,
    ) -> &mut Self {
        self.check(field, below, format!("must be greater than or equal to {min}"));
        self.check(field, above, format!("must be less than or equal to {max}"))
    }

    /// Checks that `value` is an integer D1 can store and return exactly (±2^53 - 1).
    ///
    /// See [`MAX_SAFE_INTEGER`]; same messages as [`range`](Self::range).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.safe_integer("views", ocre::MAX_SAFE_INTEGER);
    /// assert!(v.is_valid());
    /// v.safe_integer("views", i64::MAX);
    /// assert!(!v.is_valid());
    /// ```
    pub fn safe_integer(&mut self, field: &str, value: i64) -> &mut Self {
        self.range(field, value, -MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER)
    }

    /// Checks that `value` is one of `allowed` ("is not included in the list").
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.inclusion("status", "draft", &["draft", "published"]);
    /// assert!(v.is_valid());
    /// v.inclusion("status", "archived", &["draft", "published"]);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Status is not included in the list");
    /// ```
    pub fn inclusion(&mut self, field: &str, value: &str, allowed: &[&str]) -> &mut Self {
        self.check(field, !allowed.contains(&value), "is not included in the list")
    }

    /// Checks that `value` looks like an e-mail address ("is invalid").
    ///
    /// One `@`, text on both sides, a dot in the domain, no spaces or `<>,`.
    /// Delivery is the only real check. [`mail::send`](crate::mail::send)
    /// applies the same rule.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.email("email", "ada@example.com");
    /// assert!(v.is_valid());
    /// v.email("email", "ada@localhost");
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Email is invalid");
    /// ```
    pub fn email(&mut self, field: &str, value: &str) -> &mut Self {
        self.check(field, !is_email(value), "is invalid")
    }

    /// Parses a required number typed as text (HTML forms), adding "is not a number" when it does not parse.
    ///
    /// Surrounding whitespace is ignored. Returns the parsed value, or `None`
    /// after adding the error, so parsing and validation happen in one pass.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// assert_eq!(v.number::<i64>("pages", " 42 "), Some(42));
    /// assert_eq!(v.number::<i64>("year", ""), None);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Year is not a number");
    /// ```
    pub fn number<T: FromStr>(&mut self, field: &str, text: &str) -> Option<T> {
        let parsed = text.trim().parse().ok();
        self.check(field, parsed.is_none(), "is not a number");
        parsed
    }

    /// Parses an optional number typed as text: blank text is `None` without an error.
    ///
    /// Otherwise like [`number`](Self::number).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// assert_eq!(v.optional_number::<u32>("pages", "  "), None);
    /// assert!(v.is_valid());
    /// assert_eq!(v.optional_number::<u32>("pages", "-1"), None);
    /// assert!(!v.is_valid());
    /// ```
    pub fn optional_number<T: FromStr>(&mut self, field: &str, text: &str) -> Option<T> {
        if text.trim().is_empty() { None } else { self.number(field, text) }
    }

    /// Parses a required JSON value typed as text (a `<textarea>`), adding "is not valid JSON" when it does not parse.
    ///
    /// Returns the parsed value, or `None` after adding the error.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// assert_eq!(v.json("metadata", r#"{"a": 1}"#), Some(serde_json::json!({"a": 1})));
    /// assert_eq!(v.json("metadata", "{a: 1}"), None);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Metadata is not valid JSON");
    /// ```
    pub fn json(&mut self, field: &str, text: &str) -> Option<serde_json::Value> {
        let parsed = serde_json::from_str(text).ok();
        self.check(field, parsed.is_none(), "is not valid JSON");
        parsed
    }

    /// Parses an optional JSON value typed as text: blank text is `None` without an error.
    ///
    /// Otherwise like [`json`](Self::json).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// assert_eq!(v.optional_json("metadata", ""), None);
    /// assert!(v.is_valid());
    /// ```
    pub fn optional_json(&mut self, field: &str, text: &str) -> Option<serde_json::Value> {
        if text.trim().is_empty() { None } else { self.json(field, text) }
    }

    /// Checks that `value` is a real calendar date written `YYYY-MM-DD` ("is not a valid date").
    ///
    /// Month lengths and leap years are checked.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.date("born_on", "2024-02-29");
    /// assert!(v.is_valid());
    /// v.date("born_on", "2023-02-29");
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Born on is not a valid date");
    /// ```
    pub fn date(&mut self, field: &str, value: &str) -> &mut Self {
        self.check(field, !is_date(value), "is not a valid date")
    }

    /// Checks that `value` is `YYYY-MM-DD HH:MM[:SS]`, with a space or `T` (HTML `datetime-local`).
    ///
    /// Message: "is not a valid date and time". No time zone is accepted.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.datetime("starts_at", "2026-09-29T18:30").datetime("ends_at", "2026-09-29 19:00:00");
    /// assert!(v.is_valid());
    /// v.datetime("starts_at", "2026-09-29");
    /// assert!(!v.is_valid());
    /// ```
    pub fn datetime(&mut self, field: &str, value: &str) -> &mut Self {
        let valid = value.len() >= 16
            && value.is_char_boundary(10)
            && is_date(&value[..10])
            && matches!(value.as_bytes()[10], b' ' | b'T')
            && is_time(&value[11..]);
        self.check(field, !valid, "is not a valid date and time")
    }

    /// Adds the errors collected by `other`, e.g. a model's `validate()` after parsing a form.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut model = ocre::Validator::new();
    /// model.required("title", "");
    /// let mut form = ocre::Validator::new();
    /// form.merge(model);
    /// assert!(!form.is_valid());
    /// ```
    pub fn merge(&mut self, mut other: Validator) -> &mut Self {
        self.errors.append(&mut other.errors);
        self
    }

    /// Whether no error has been collected so far.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// assert!(v.is_valid());
    /// v.required("title", "");
    /// assert!(!v.is_valid());
    /// ```
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns `Ok(())` when every check passed, and takes the collected errors otherwise.
    ///
    /// The validator is empty afterwards, so it can be reused.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] (422) with every collected [`FieldError`], in the
    /// order the checks ran.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::{Error, FieldError, Validator};
    ///
    /// let mut v = Validator::new();
    /// v.required("title", "");
    /// let Err(Error::Invalid(errors)) = v.finish() else { panic!("expected errors") };
    /// assert_eq!(errors, [FieldError::new("title", "can't be blank")]);
    /// assert!(v.finish().is_ok());
    /// ```
    pub fn finish(&mut self) -> Result<(), Error> {
        if self.errors.is_empty() { Ok(()) } else { Err(Error::Invalid(std::mem::take(&mut self.errors))) }
    }
}

/// The address rule behind [`Validator::email`] and outgoing mail.
pub(crate) fn is_email(value: &str) -> bool {
    let forbidden = |c: char| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | ',');
    !value.contains(forbidden)
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        })
}

fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `YYYY-MM-DD`, checking month lengths and leap years.
fn is_date(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    let [year, month, day] = parts[..] else { return false };
    let (Some(year), Some(month), Some(day)) = (digits(year), digits(month), digits(day)) else { return false };
    if parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

/// `HH:MM` or `HH:MM:SS`.
fn is_time(value: &str) -> bool {
    let parts: Vec<&str> = value.split(':').collect();
    let limits = [23, 59, 59];
    (2..=3).contains(&parts.len())
        && parts.iter().zip(limits).all(|(part, max)| part.len() == 2 && digits(part).is_some_and(|n| n <= max))
}

#[cfg(test)]
#[path = "../tests/validate.rs"]
mod tests;

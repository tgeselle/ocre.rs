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

/// One failed check: `field` plus the message without the field name
/// (`"can't be blank"`), as in Rails' `errors`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self { field: field.into(), message: message.into() }
    }

    /// `"Title can't be blank"`.
    pub fn full_message(&self) -> String {
        format!("{} {}", humanize(&self.field), self.message)
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.full_message())
    }
}

/// Collects field errors; [`finish`](Self::finish) fails with all of them.
#[derive(Debug, Default)]
pub struct Validator {
    errors: Vec<FieldError>,
}

impl Validator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an error when `failed` is true.
    pub fn check(&mut self, field: &str, failed: bool, message: impl Into<String>) -> &mut Self {
        if failed {
            self.errors.push(FieldError::new(field, message));
        }
        self
    }

    /// Not empty after trimming whitespace.
    pub fn required(&mut self, field: &str, value: &str) -> &mut Self {
        self.check(field, value.trim().is_empty(), "can't be blank")
    }

    /// At most `max` characters.
    pub fn max_length(&mut self, field: &str, value: &str, max: usize) -> &mut Self {
        let too_long = value.chars().count() > max;
        self.check(field, too_long, format!("is too long (maximum is {max} characters)"))
    }

    /// At least `min` characters.
    pub fn min_length(&mut self, field: &str, value: &str, min: usize) -> &mut Self {
        let too_short = value.chars().count() < min;
        self.check(field, too_short, format!("is too short (minimum is {min} characters)"))
    }

    /// Within `range`, inclusive.
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

    /// Integers D1 can store and return exactly (±2^53 - 1).
    pub fn safe_integer(&mut self, field: &str, value: i64) -> &mut Self {
        self.range(field, value, -MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER)
    }

    /// One of `allowed`.
    pub fn inclusion(&mut self, field: &str, value: &str, allowed: &[&str]) -> &mut Self {
        self.check(field, !allowed.contains(&value), "is not included in the list")
    }

    /// Looks like an e-mail address: one `@`, text on both sides, a dot in
    /// the domain. Delivery is the only real check.
    pub fn email(&mut self, field: &str, value: &str) -> &mut Self {
        let valid = value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
        self.check(field, !valid, "is invalid")
    }

    /// Parses a required number typed as text (HTML forms). Adds
    /// "is not a number" and returns `None` when it does not parse.
    pub fn number<T: FromStr>(&mut self, field: &str, text: &str) -> Option<T> {
        let parsed = text.trim().parse().ok();
        self.check(field, parsed.is_none(), "is not a number");
        parsed
    }

    /// Like [`number`](Self::number) for optional fields: empty text is `None`
    /// without an error.
    pub fn optional_number<T: FromStr>(&mut self, field: &str, text: &str) -> Option<T> {
        if text.trim().is_empty() { None } else { self.number(field, text) }
    }

    /// `YYYY-MM-DD` with a real calendar date.
    pub fn date(&mut self, field: &str, value: &str) -> &mut Self {
        self.check(field, !is_date(value), "is not a valid date")
    }

    /// `YYYY-MM-DD HH:MM[:SS]`, with a space or `T` (HTML `datetime-local`).
    pub fn datetime(&mut self, field: &str, value: &str) -> &mut Self {
        let valid = value.len() >= 16
            && value.is_char_boundary(10)
            && is_date(&value[..10])
            && matches!(value.as_bytes()[10], b' ' | b'T')
            && is_time(&value[11..]);
        self.check(field, !valid, "is not a valid date and time")
    }

    /// Adds the errors collected by `other` (e.g. a model's `validate()` after
    /// parsing a form).
    pub fn merge(&mut self, mut other: Validator) -> &mut Self {
        self.errors.append(&mut other.errors);
        self
    }

    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// `Ok(())`, or [`Error::Invalid`] with every collected error.
    pub fn finish(&mut self) -> Result<(), Error> {
        if self.errors.is_empty() { Ok(()) } else { Err(Error::Invalid(std::mem::take(&mut self.errors))) }
    }
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
mod tests;

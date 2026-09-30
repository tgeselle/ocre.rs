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
#[derive(Debug, Clone, Serialize)]
pub struct FieldError {
    /// Field name as in the form or JSON body (`"title"`, `"author_id"`).
    pub field: String,
    /// Message without the field name (`"can't be blank"`).
    pub message: String,
    /// Rails' translation key of the check (`blank`, `too_long`...), for
    /// [`I18n::error_message`](crate::i18n::I18n::error_message).
    #[serde(skip)]
    key: Option<&'static str>,
    /// `%{count}` of the message (a bound), or the confirmed field of `confirmation`.
    #[serde(skip)]
    detail: Option<String>,
}

/// Compares the field and the message only, as they are what users see.
impl PartialEq for FieldError {
    fn eq(&self, other: &Self) -> bool {
        self.field == other.field && self.message == other.message
    }
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
        Self { field: field.into(), message: message.into(), key: None, detail: None }
    }

    /// The Rails translation key of the check that failed (`"blank"`, `"too_long"`...), if any.
    ///
    /// Set by each [`Validator`] check; `None` for [`FieldError::new`],
    /// [`Validator::check`] and after [`Validator::message`].
    /// [`I18n::error_message`](crate::i18n::I18n::error_message) looks it up
    /// under `errors.messages.<key>` (and more specific keys).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.required("title", "").max_length("body", "long text", 3);
    /// let keys: Vec<_> = v.errors().iter().map(|e| e.key()).collect();
    /// assert_eq!(keys, [Some("blank"), Some("too_long")]);
    /// assert_eq!(ocre::FieldError::new("title", "is odd").key(), None);
    /// ```
    pub fn key(&self) -> Option<&'static str> {
        self.key
    }

    /// `%{count}` for the message, or the confirmed field of `confirmation`.
    pub(crate) fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
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
    /// Whether the last check failed, for [`message`](Self::message).
    last_failed: bool,
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
        self.last_failed = failed;
        if failed {
            self.errors.push(FieldError::new(field, message));
        }
        self
    }

    /// [`check`](Self::check) recording Rails' translation `key` and the `%{count}` (or confirmed field) `detail`.
    fn fail(
        &mut self,
        field: &str,
        failed: bool,
        key: &'static str,
        detail: Option<&dyn fmt::Display>,
        message: impl Into<String>,
    ) -> &mut Self {
        self.check(field, failed, message);
        if failed && let Some(error) = self.errors.last_mut() {
            error.key = Some(key);
            error.detail = detail.map(ToString::to_string);
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
        self.fail(field, value.trim().is_empty(), "blank", None, "can't be blank")
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
        self.fail(field, too_long, "too_long", Some(&max), format!("is too long (maximum is {max} characters)"))
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
        self.fail(field, too_short, "too_short", Some(&min), format!("is too short (minimum is {min} characters)"))
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
        if below {
            self.fail(
                field,
                true,
                "greater_than_or_equal_to",
                Some(min),
                format!("must be greater than or equal to {min}"),
            )
        } else {
            self.fail(field, above, "less_than_or_equal_to", Some(max), format!("must be less than or equal to {max}"))
        }
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
        self.fail(field, !allowed.contains(&value), "inclusion", None, "is not included in the list")
    }

    /// Checks that `value` is not one of `forbidden` ("is reserved"), like Rails' `exclusion`.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.exclusion("username", "ada", &["admin", "root"]);
    /// assert!(v.is_valid());
    /// v.exclusion("username", "admin", &["admin", "root"]);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Username is reserved");
    /// ```
    pub fn exclusion(&mut self, field: &str, value: &str, forbidden: &[&str]) -> &mut Self {
        self.fail(field, forbidden.contains(&value), "exclusion", None, "is reserved")
    }

    /// Checks that `value` has exactly `length` characters (Unicode scalar
    /// values): "is the wrong length (should be N characters)". For a range
    /// (Rails' `in:`), chain [`min_length`](Self::min_length) and
    /// [`max_length`](Self::max_length).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.length("zip", "75001", 5);
    /// assert!(v.is_valid());
    /// v.length("zip", "7500", 5);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Zip is the wrong length (should be 5 characters)");
    /// ```
    pub fn length(&mut self, field: &str, value: &str, length: usize) -> &mut Self {
        let wrong = value.chars().count() != length;
        let message = format!("is the wrong length (should be {length} characters)");
        self.fail(field, wrong, "wrong_length", Some(&length), message)
    }

    /// Checks that `value > than` ("must be greater than N"), like Rails' `comparison`.
    ///
    /// Works for numbers and for `YYYY-MM-DD` dates or datetimes as text,
    /// which compare in time order.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.greater_than("ends_on", "2026-10-02", "2026-10-01").greater_than("quantity", 0, 0);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Quantity must be greater than 0");
    /// ```
    pub fn greater_than<T: PartialOrd + fmt::Display>(&mut self, field: &str, value: T, than: T) -> &mut Self {
        self.fail(field, value <= than, "greater_than", Some(&than), format!("must be greater than {than}"))
    }

    /// Checks that `value >= min` ("must be greater than or equal to N").
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.greater_than_or_equal_to("age", 17, 18);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Age must be greater than or equal to 18");
    /// ```
    pub fn greater_than_or_equal_to<T: PartialOrd + fmt::Display>(
        &mut self,
        field: &str,
        value: T,
        min: T,
    ) -> &mut Self {
        let message = format!("must be greater than or equal to {min}");
        self.fail(field, value < min, "greater_than_or_equal_to", Some(&min), message)
    }

    /// Checks that `value < than` ("must be less than N").
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.less_than("discount", 1.0, 1.0);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Discount must be less than 1");
    /// ```
    pub fn less_than<T: PartialOrd + fmt::Display>(&mut self, field: &str, value: T, than: T) -> &mut Self {
        self.fail(field, value >= than, "less_than", Some(&than), format!("must be less than {than}"))
    }

    /// Checks that `value <= max` ("must be less than or equal to N").
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.less_than_or_equal_to("seats", 9, 8);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Seats must be less than or equal to 8");
    /// ```
    pub fn less_than_or_equal_to<T: PartialOrd + fmt::Display>(&mut self, field: &str, value: T, max: T) -> &mut Self {
        self.fail(
            field,
            value > max,
            "less_than_or_equal_to",
            Some(&max),
            format!("must be less than or equal to {max}"),
        )
    }

    /// Checks that `value != other` ("must be other than N").
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.other_than("parent_id", 4, 4);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Parent must be other than 4");
    /// ```
    pub fn other_than<T: PartialEq + fmt::Display>(&mut self, field: &str, value: T, other: T) -> &mut Self {
        self.fail(field, value == other, "other_than", Some(&other), format!("must be other than {other}"))
    }

    /// Checks that `confirmation` equals `value`, like Rails' `confirmation`:
    /// the error goes on `<field>_confirmation` ("doesn't match Password").
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.confirmation("password", "s3cret-pass", "s3cret-pass");
    /// assert!(v.is_valid());
    /// v.confirmation("password", "s3cret-pass", "typo");
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Password confirmation doesn't match Password");
    /// ```
    pub fn confirmation(&mut self, field: &str, value: &str, confirmation: &str) -> &mut Self {
        let message = format!("doesn't match {}", humanize(field));
        self.fail(&format!("{field}_confirmation"), value != confirmation, "confirmation", Some(&field), message)
    }

    /// Checks that a checkbox was ticked ("must be accepted"), like Rails' `acceptance`.
    ///
    /// The value is not stored: take it as a `bool` in the form or JSON
    /// struct (`#[serde(default)]`, since an unticked checkbox sends nothing).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.acceptance("terms_of_service", false);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Terms of service must be accepted");
    /// ```
    pub fn acceptance(&mut self, field: &str, accepted: bool) -> &mut Self {
        self.fail(field, !accepted, "accepted", None, "must be accepted")
    }

    /// Checks that `value` is blank: empty or only whitespace ("must be blank"), like Rails' `absence`.
    ///
    /// For an `Option`, check `value.is_some()` with [`check`](Self::check).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.absence("nickname", " ");
    /// assert!(v.is_valid());
    /// v.absence("nickname", "bot");
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Nickname must be blank");
    /// ```
    pub fn absence(&mut self, field: &str, value: &str) -> &mut Self {
        self.fail(field, !value.trim().is_empty(), "present", None, "must be blank")
    }

    /// Checks that every character of `value` passes `allowed` ("is invalid"):
    /// Rails' `format`, without regular expressions (no regex engine in the
    /// WebAssembly binary). Combine with [`min_length`](Self::min_length) to
    /// refuse an empty value, and with [`check`](Self::check) for position
    /// rules (`value.starts_with(..)`).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// let slug = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
    /// v.format("slug", "hello-2026", slug);
    /// assert!(v.is_valid());
    /// v.format("slug", "Hello World", slug);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Slug is invalid");
    /// ```
    pub fn format(&mut self, field: &str, value: &str, allowed: impl Fn(char) -> bool) -> &mut Self {
        let invalid = !value.chars().all(allowed);
        self.fail(field, invalid, "invalid", None, "is invalid")
    }

    /// Replaces the message of the check just before, if it failed (Rails' `message:` option).
    ///
    /// Only the last check counts: call it right after the check it changes.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.required("title", "Dune").message("needs a title");
    /// v.required("body", "").message("write something first");
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Body write something first");
    /// ```
    pub fn message(&mut self, message: impl Into<String>) -> &mut Self {
        if self.last_failed
            && let Some(error) = self.errors.last_mut()
        {
            error.message = message.into();
            error.key = None;
            error.detail = None;
        }
        self
    }

    /// The errors collected so far, in the order the checks ran (Rails' `errors`).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.required("title", "").required("body", "");
    /// let fields: Vec<&str> = v.errors().iter().map(|e| e.field.as_str()).collect();
    /// assert_eq!(fields, ["title", "body"]);
    /// ```
    pub fn errors(&self) -> &[FieldError] {
        &self.errors
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
        self.fail(field, !is_email(value), "invalid", None, "is invalid")
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
        self.fail(field, parsed.is_none(), "not_a_number", None, "is not a number");
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

    /// Parses form text into one of an enum's values, like
    /// [`number`](Self::number): `None` plus "is not included in the list"
    /// when `T::from_str` refuses it. Generated scaffolds use it for `enum`
    /// fields.
    ///
    /// # Examples
    ///
    /// ```
    /// #[derive(Debug, PartialEq)]
    /// enum Status {
    ///     Draft,
    /// }
    ///
    /// impl std::str::FromStr for Status {
    ///     type Err = ();
    ///     fn from_str(text: &str) -> Result<Self, ()> {
    ///         if text == "draft" { Ok(Status::Draft) } else { Err(()) }
    ///     }
    /// }
    ///
    /// let mut v = ocre::Validator::new();
    /// assert_eq!(v.one_of::<Status>("status", "draft"), Some(Status::Draft));
    /// assert_eq!(v.one_of::<Status>("status", "archived"), None);
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Status is not included in the list");
    /// ```
    pub fn one_of<T: FromStr>(&mut self, field: &str, text: &str) -> Option<T> {
        let parsed = text.parse().ok();
        self.fail(field, parsed.is_none(), "inclusion", None, "is not included in the list");
        parsed
    }

    /// Like [`one_of`](Self::one_of) for an optional field: blank text is `None` without error.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// assert_eq!(v.optional_one_of::<bool>("flag", " "), None);
    /// assert_eq!(v.optional_one_of::<bool>("flag", "true"), Some(true));
    /// assert!(v.is_valid());
    /// ```
    pub fn optional_one_of<T: FromStr>(&mut self, field: &str, text: &str) -> Option<T> {
        if text.trim().is_empty() { None } else { self.one_of(field, text) }
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
        self.fail(field, parsed.is_none(), "not_json", None, "is not valid JSON");
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
        self.fail(field, !is_date(value), "not_a_date", None, "is not a valid date")
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
        self.fail(field, !valid, "not_a_datetime", None, "is not a valid date and time")
    }

    /// Checks that `value` is a time of day written `HH:MM` or `HH:MM:SS` (HTML `<input type="time">`).
    ///
    /// Message: "is not a valid time". No time zone is accepted.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.time("opens_at", "09:30").time("closes_at", "18:00:00");
    /// assert!(v.is_valid());
    /// v.time("opens_at", "24:00");
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Opens at is not a valid time");
    /// ```
    pub fn time(&mut self, field: &str, value: &str) -> &mut Self {
        self.fail(field, !is_time(value), "not_a_time", None, "is not a valid time")
    }

    /// Checks that `value` is a UUID in its hyphenated form, any case ("is not a valid UUID").
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.uuid("token", "67e55044-10b1-426f-9247-bb680e5fe0c8");
    /// assert!(v.is_valid());
    /// v.uuid("token", "67e55044");
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Token is not a valid UUID");
    /// ```
    pub fn uuid(&mut self, field: &str, value: &str) -> &mut Self {
        let groups: Vec<&str> = value.split('-').collect();
        let valid = groups.len() == 5
            && groups
                .iter()
                .zip([8, 4, 4, 4, 12])
                .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()));
        self.fail(field, !valid, "not_a_uuid", None, "is not a valid UUID")
    }

    /// Checks that `value` is an exact decimal number such as `-12.50` ("is not a decimal number").
    ///
    /// An optional sign, digits, then optionally a dot and digits: no
    /// exponent, no spaces. Decimal columns are stored as this text, so money
    /// keeps every digit (a `REAL` would round `0.1 + 0.2`).
    ///
    /// # Examples
    ///
    /// ```
    /// let mut v = ocre::Validator::new();
    /// v.decimal("price", "19.99").decimal("balance", "-3");
    /// assert!(v.is_valid());
    /// v.decimal("price", "1e3");
    /// assert_eq!(v.finish().unwrap_err().to_string(), "invalid: Price is not a decimal number");
    /// ```
    pub fn decimal(&mut self, field: &str, value: &str) -> &mut Self {
        let unsigned = value.strip_prefix(['-', '+']).unwrap_or(value);
        let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, "0"));
        let valid = [whole, fraction].iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
        self.fail(field, !valid, "not_a_decimal", None, "is not a decimal number")
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

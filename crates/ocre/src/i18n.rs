//! Translations, like Rails' `I18n.t`: YAML files in `locales/` compiled
//! into the Worker, `%{name}` interpolation, CLDR plurals, fallback to the
//! default locale and locale selection per request.
//!
//! ```yaml
//! # locales/fr.yml
//! fr:
//!   posts:
//!     created: "Article créé."
//!     greeting: "Bonjour %{name} !"
//!     count:
//!       one: "%{count} article"
//!       other: "%{count} articles"
//! ```
//!
//! ```ignore
//! // src/lib.rs: the first code is the default locale.
//! static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr");
//!
//! fn routes() -> Router<Ctx> {
//!     Router::new()
//!         .route("/", get(home))
//!         .layer(ocre::i18n::layer(&LOCALES))
//! }
//!
//! async fn home(i18n: I18n) -> String {
//!     i18n.t("posts.count").count(3).to_string() // "3 articles" for French visitors
//! }
//! ```
//!
//! The files are parsed once per Worker instance, on the first translation.
//! [`I18n`] picks the locale from a `{locale}` path segment, then the
//! `locale` cookie, then `Accept-Language`, then the default locale.
//! A key missing from the request's locale falls back to the default locale
//! in release builds (`ocre deploy`); debug builds (`ocre dev`) show
//! `translation missing: fr.posts.created` instead, so gaps are visible.
//! `ocre i18n missing` lists them.

mod plural;
mod yaml;

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::LazyLock,
};

use axum::{
    Extension,
    extract::{FromRequestParts, RawPathParams},
    http::{HeaderMap, header, request::Parts},
};

use crate::{Error, Result};
use plural::{Category, Rule};

/// Cookie that remembers a visitor's chosen locale (see [`I18n::cookie`]).
pub const LOCALE_COOKIE: &str = "locale";

/// Name of the path parameter [`I18n`] reads: `.nest("/{locale}", pages())`.
pub const LOCALE_PARAM: &str = "locale";

/// The app's translations, declared once in `src/lib.rs` with
/// [`locales!`](crate::locales) and parsed on first use.
///
/// ```ignore
/// static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr");
/// ```
pub type Locales = LazyLock<Catalog>;

/// Declares the app's locales: `ocre::locales!("en", "fr")` compiles
/// `locales/en.yml` and `locales/fr.yml` (paths from the app root) into the
/// binary. The first code is the default locale, used for fallbacks. A
/// missing file is a compile error.
///
/// ```ignore
/// static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr");
/// ```
#[macro_export]
macro_rules! locales {
    ($($code:literal),+ $(,)?) => {
        ::std::sync::LazyLock::new(|| {
            $crate::i18n::Catalog::load(&[$((
                $code,
                ::std::include_str!(::std::concat!(::std::env!("CARGO_MANIFEST_DIR"), "/locales/", $code, ".yml")),
            )),+])
        })
    };
}

/// Makes the catalog available to the [`I18n`] extractor. Add it last in
/// `routes()`, after the `// ocre:routes` marker, so it covers every route:
///
/// ```ignore
/// Router::new()
///     .route("/", get(home))
///     // ocre:routes
///     .layer(ocre::i18n::layer(&LOCALES))
/// ```
pub fn layer(locales: &'static Locales) -> Extension<&'static Catalog> {
    Extension(LazyLock::force(locales))
}

/// A translated string (text or plural forms).
#[derive(Debug, Clone, PartialEq)]
enum Value<'t> {
    Text(Cow<'t, str>),
    /// Indexed like [`Category::ALL`].
    Plural([Option<Cow<'t, str>>; 6]),
}

/// One parsed locale file.
#[derive(Debug)]
struct Table {
    code: &'static str,
    rule: Rule,
    values: BTreeMap<String, Value<'static>>,
}

impl Table {
    /// Tables live in a `static` [`Locales`], so their strings are `'static`.
    fn get(&'static self, key: &str, count: Option<i64>) -> Option<&'static str> {
        let text = match (self.values.get(key)?, count) {
            (Value::Text(text), _) => text,
            (Value::Plural(forms), Some(0)) if forms[Category::Zero as usize].is_some() => {
                forms[Category::Zero as usize].as_ref()?
            }
            (Value::Plural(forms), Some(n)) => {
                forms[self.rule.category(n) as usize].as_ref().or(forms[Category::Other as usize].as_ref())?
            }
            (Value::Plural(forms), None) => forms[Category::Other as usize].as_ref()?,
        };
        Some(text)
    }
}

/// All locales of an app, built by [`locales!`](crate::locales). Files that
/// fail to parse are empty (so every key falls back) and listed in
/// [`errors`](Self::errors), which are also logged.
///
/// ```
/// use std::sync::LazyLock;
/// use ocre::i18n::{Catalog, Locales};
///
/// static LOCALES: Locales = LazyLock::new(|| {
///     Catalog::load(&[("en", "en:\n  hello: \"Hello %{name}\"\n"), ("fr", "fr:\n  hello: \"Bonjour %{name}\"\n")])
/// });
///
/// assert_eq!(LOCALES.default_locale(), "en");
/// assert_eq!(LOCALES.codes().collect::<Vec<_>>(), ["en", "fr"]);
/// assert_eq!(LOCALES.locale("fr").t("hello").arg("name", "Ada").to_string(), "Bonjour Ada");
/// ```
#[derive(Debug)]
pub struct Catalog {
    tables: Vec<Table>,
    errors: Vec<String>,
}

impl Catalog {
    /// Parses `(code, file contents)` pairs; the first is the default locale.
    ///
    /// ```
    /// let catalog = ocre::i18n::Catalog::load(&[("en", "en:\n  title: Posts\n")]);
    /// assert!(catalog.errors().is_empty());
    /// ```
    pub fn load(sources: &[(&'static str, &'static str)]) -> Self {
        let mut tables = Vec::with_capacity(sources.len().max(1));
        let mut errors = Vec::new();
        for (code, text) in sources {
            let values = match yaml::parse_locale(code, text) {
                Ok(entries) => group(entries),
                Err(err) => {
                    errors.push(format!("locales/{code}.yml line {}: {}", err.line, err.message));
                    BTreeMap::new()
                }
            };
            tables.push(Table { code, rule: Rule::for_locale(code), values });
        }
        if tables.is_empty() {
            errors.push("no locales: declare them with ocre::locales!(\"en\")".to_owned());
            tables.push(Table { code: "en", rule: Rule::One, values: BTreeMap::new() });
        }
        for error in &errors {
            crate::error::log_internal(error);
        }
        Self { tables, errors }
    }

    /// Parse errors, one line each (`locales/fr.yml line 3: ...`).
    ///
    /// ```
    /// let catalog = ocre::i18n::Catalog::load(&[("en", "en:\n  title: [Posts]\n")]);
    /// assert_eq!(catalog.errors().len(), 1);
    /// ```
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// The default locale: the first code given to [`locales!`](crate::locales).
    ///
    /// ```
    /// let catalog = ocre::i18n::Catalog::load(&[("fr", "fr:\n"), ("en", "en:\n")]);
    /// assert_eq!(catalog.default_locale(), "fr");
    /// ```
    pub fn default_locale(&self) -> &'static str {
        self.tables[0].code
    }

    /// Every locale code, default first; for language switchers.
    ///
    /// ```
    /// let catalog = ocre::i18n::Catalog::load(&[("en", "en:\n"), ("pt-BR", "pt-BR:\n")]);
    /// assert_eq!(catalog.codes().collect::<Vec<_>>(), ["en", "pt-BR"]);
    /// ```
    pub fn codes(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tables.iter().map(|table| table.code)
    }

    /// Translations in `code` (case-insensitive), or in the default locale
    /// when the app has no such locale. For code outside requests: mailers,
    /// jobs.
    ///
    /// ```ignore
    /// let subject = LOCALES.locale(&user.locale).t("mailers.welcome.subject").to_string();
    /// ```
    pub fn locale(&'static self, code: &str) -> I18n {
        I18n { catalog: self, index: self.find(code).unwrap_or(0) }
    }

    fn find(&self, code: &str) -> Option<usize> {
        self.tables.iter().position(|table| table.code.eq_ignore_ascii_case(code))
    }

    /// Best locale for an `Accept-Language` header: by quality, exact code
    /// first, then the same language (`fr-CH` matches `fr`, `pt` matches `pt-BR`).
    fn negotiate(&self, accept_language: &str) -> Option<usize> {
        let mut ranges: Vec<(&str, f32)> = accept_language
            .split(',')
            .filter_map(|part| {
                let mut pieces = part.split(';');
                let tag = pieces.next().unwrap_or_default().trim();
                let quality = pieces
                    .find_map(|param| param.trim().strip_prefix("q="))
                    .map_or(1.0, |q| q.trim().parse::<f32>().unwrap_or(0.0));
                (!tag.is_empty() && tag != "*" && quality > 0.0).then_some((tag, quality))
            })
            .collect();
        ranges.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranges.iter().find_map(|(tag, _)| {
            self.find(tag).or_else(|| {
                let language = primary(tag);
                self.tables.iter().position(|table| primary(table.code).eq_ignore_ascii_case(language))
            })
        })
    }

    /// The text for `key` in locale `index`: the locale's own, else (unless
    /// `strict`) the default locale's.
    fn lookup(&'static self, index: usize, key: &str, count: Option<i64>, strict: bool) -> Option<&'static str> {
        let own = self.tables[index].get(key, count);
        if own.is_some() || strict || index == 0 {
            return own;
        }
        self.tables[0].get(key, count)
    }
}

/// `fr` of `fr-CH`.
fn primary(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

/// Turns `posts.count.one`/`posts.count.other` into one plural value. A
/// mapping is plural when every child is a CLDR category name (`zero`,
/// `one`, `two`, `few`, `many`, `other`).
fn group<'t>(entries: Vec<(String, Cow<'t, str>)>) -> BTreeMap<String, Value<'t>> {
    let flat: BTreeMap<String, Cow<'t, str>> = entries.into_iter().collect();
    let candidates: BTreeSet<&str> = flat
        .keys()
        .filter_map(|key| key.rsplit_once('.'))
        .filter(|(_, last)| Category::from_name(last).is_some())
        .map(|(parent, _)| parent)
        .collect();
    let plural: BTreeSet<String> = candidates
        .into_iter()
        .filter(|parent| {
            let prefix = format!("{parent}.");
            flat.range(prefix.clone()..)
                .take_while(|(key, _)| key.starts_with(&prefix))
                .all(|(key, _)| Category::from_name(&key[prefix.len()..]).is_some())
        })
        .map(str::to_owned)
        .collect();
    let mut values = BTreeMap::new();
    for (key, text) in flat {
        match key.rsplit_once('.').filter(|(parent, _)| plural.contains(*parent)) {
            Some((parent, last)) => {
                let category = Category::from_name(last).expect("plural children are categories");
                let entry = values.entry(parent.to_owned()).or_insert_with(|| Value::Plural(Default::default()));
                if let Value::Plural(forms) = entry {
                    forms[category as usize] = Some(text);
                }
            }
            None => {
                values.insert(key, Value::Text(text));
            }
        }
    }
    values
}

/// A problem found by [`check`].
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// The file is not valid locale YAML; nothing in it is used.
    Invalid { locale: String, line: usize, message: String },
    /// A key of the default locale (or a plural form this language needs)
    /// is missing from `locale`.
    Missing { locale: String, key: String },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { locale, line, message } => write!(f, "locales/{locale}.yml:{line}: {message}"),
            Self::Missing { locale, key } => write!(f, "locales/{locale}.yml: missing {key}"),
        }
    }
}

/// Checks locale files the way [`Catalog::load`] reads them: syntax, then
/// every key of the default (first) locale in every other one, with the
/// plural forms each language needs (Russian needs `few` and `many`,
/// Japanese only `other`). `ocre i18n missing` prints the result.
///
/// ```
/// use ocre::i18n::{Problem, check};
///
/// let problems = check(&[
///     ("en", "en:\n  title: Posts\n  count:\n    one: \"%{count} post\"\n    other: \"%{count} posts\"\n"),
///     ("fr", "fr:\n  count:\n    one: \"%{count} article\"\n    other: \"%{count} articles\"\n"),
/// ]);
/// assert_eq!(problems, [Problem::Missing { locale: "fr".into(), key: "title".into() }]);
/// ```
pub fn check(sources: &[(&str, &str)]) -> Vec<Problem> {
    let mut problems = Vec::new();
    let mut parsed = Vec::with_capacity(sources.len());
    for (code, text) in sources {
        match yaml::parse_locale(code, text) {
            Ok(entries) => parsed.push(Some(entries)),
            Err(err) => {
                problems.push(Problem::Invalid { locale: (*code).to_owned(), line: err.line, message: err.message });
                parsed.push(None);
            }
        }
    }
    let Some(Some(default)) = parsed.first() else {
        return problems;
    };
    let default = group(default.clone());
    for ((code, _), entries) in sources.iter().zip(&parsed).skip(1) {
        let Some(entries) = entries else { continue };
        let present: BTreeSet<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
        for (key, value) in &default {
            let required: Vec<String> = match value {
                Value::Text(_) => vec![key.clone()],
                Value::Plural(_) if present.contains(key.as_str()) => vec![],
                Value::Plural(_) => Rule::for_locale(code)
                    .categories()
                    .iter()
                    .map(|category| format!("{key}.{}", category.name()))
                    .collect(),
            };
            for key in required.into_iter().filter(|key| !present.contains(key.as_str())) {
                problems.push(Problem::Missing { locale: (*code).to_owned(), key });
            }
        }
    }
    problems
}

/// Extractor: translations in the request's locale. The locale comes from
/// the `{locale}` path segment (routes nested with `.nest("/{locale}", ..)`;
/// an unknown code is a 404), else the `locale` cookie, else
/// `Accept-Language`, else the default locale. Needs [`layer`].
///
/// ```ignore
/// async fn show(i18n: I18n) -> Result<Html<String>> {
///     render(&ShowView { i18n })       // template: {{ i18n.t("posts.title") }}
/// }
/// ```
#[derive(Clone, Copy)]
pub struct I18n {
    catalog: &'static Catalog,
    index: usize,
}

impl fmt::Debug for I18n {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("I18n").field("locale", &self.locale()).finish()
    }
}

impl I18n {
    /// The locale code, e.g. for `<html lang="{{ i18n.locale() }}">`.
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n")]));
    /// assert_eq!(LOCALES.locale("en").locale(), "en");
    /// ```
    pub fn locale(&self) -> &'static str {
        self.catalog.tables[self.index].code
    }

    /// Every locale of the app, default first; for language switchers.
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n"), ("fr", "fr:\n")]));
    /// assert_eq!(LOCALES.locale("fr").codes().collect::<Vec<_>>(), ["en", "fr"]);
    /// ```
    pub fn codes(&self) -> impl Iterator<Item = &'static str> + 'static {
        self.catalog.codes()
    }

    /// The translation of a dotted key. It is a [`Display`](fmt::Display)
    /// value: askama writes it (escaped) into the page without an extra
    /// string, `.to_string()` gives a `String`.
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("en", "en:\n  posts:\n    created: Post was successfully created.\n")])
    /// });
    /// let i18n = LOCALES.locale("en");
    /// assert_eq!(i18n.t("posts.created").to_string(), "Post was successfully created.");
    /// ```
    pub fn t<'a>(&self, key: &'a str) -> Translation<'a> {
        Translation { i18n: *self, key, count: None, args: Vec::new() }
    }

    /// `Set-Cookie` value remembering this locale for a year; send it when
    /// the visitor picks a language:
    ///
    /// ```ignore
    /// async fn switch(Form(form): Form<LocaleForm>) -> impl IntoResponse {
    ///     let cookie = LOCALES.locale(&form.locale).cookie(); // unknown codes give the default
    ///     ([(header::SET_COOKIE, cookie)], Redirect::to("/"))
    /// }
    /// ```
    pub fn cookie(&self) -> String {
        format!("{LOCALE_COOKIE}={}; Path=/; Max-Age=31536000; SameSite=Lax", self.locale())
    }

    /// The extractor's logic: the catalog put there by [`layer`], then [`select`](Self::select).
    fn from_parts(catalog: Option<&'static Catalog>, path: Option<&str>, headers: &HeaderMap) -> Result<Self> {
        let catalog = catalog.ok_or_else(|| {
            Error::internal(
                "the I18n extractor needs the catalog. Fix: add `.layer(ocre::i18n::layer(&LOCALES))` at the end of \
                 routes() in src/lib.rs (`ocre g locale` does it)",
            )
        })?;
        Self::select(catalog, path, headers)
    }

    /// The locale of a request: path segment, cookie, `Accept-Language`, default.
    fn select(catalog: &'static Catalog, path: Option<&str>, headers: &HeaderMap) -> Result<Self> {
        if let Some(code) = path {
            return catalog.find(code).map(|index| Self { catalog, index }).ok_or(Error::NotFound);
        }
        let cookie = headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .filter_map(|pair| pair.trim().strip_prefix(LOCALE_COOKIE)?.strip_prefix('='))
            .find_map(|code| catalog.find(code));
        let index = cookie
            .or_else(|| {
                let accept = headers.get(header::ACCEPT_LANGUAGE)?.to_str().ok()?;
                catalog.negotiate(accept)
            })
            .unwrap_or(0);
        Ok(Self { catalog, index })
    }
}

/// Failures render as HTML pages in full-stack apps and as JSON in API-only apps.
impl<S: Send + Sync> FromRequestParts<S> for I18n {
    type Rejection = crate::session::Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let params = RawPathParams::from_request_parts(parts, state).await.ok();
        let path = params.as_ref().and_then(|params| params.iter().find(|(name, _)| *name == LOCALE_PARAM));
        Self::from_parts(
            parts.extensions.get::<&'static Catalog>().copied(),
            path.map(|(_, code)| code),
            &parts.headers,
        )
        .map_err(crate::session::reject)
    }
}

/// A translation being built by [`I18n::t`]; add `%{name}` values with
/// [`arg`](Self::arg) and the plural count with [`count`](Self::count).
/// A key missing everywhere displays as `translation missing: fr.posts.title`.
///
/// ```
/// # use std::sync::LazyLock;
/// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
///     ocre::i18n::Catalog::load(&[(
///         "en",
///         "en:\n  inbox:\n    zero: \"No messages, %{name}\"\n    one: \"One message\"\n    other: \"%{count} messages\"\n",
///     )])
/// });
/// let i18n = LOCALES.locale("en");
/// assert_eq!(i18n.t("inbox").count(0).arg("name", "Ada").to_string(), "No messages, Ada");
/// assert_eq!(i18n.t("inbox").count(1).to_string(), "One message");
/// assert_eq!(i18n.t("inbox").count(12).to_string(), "12 messages");
/// assert_eq!(i18n.t("nope").to_string(), "translation missing: en.nope");
/// ```
#[derive(Debug, Clone)]
pub struct Translation<'a> {
    i18n: I18n,
    key: &'a str,
    count: Option<i64>,
    args: Vec<(&'a str, String)>,
}

impl<'a> Translation<'a> {
    /// Value for `%{name}`.
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n  hi: \"Hi %{name}\"\n")]));
    /// assert_eq!(LOCALES.locale("en").t("hi").arg("name", "Ada").to_string(), "Hi Ada");
    /// ```
    pub fn arg(mut self, name: &'a str, value: impl fmt::Display) -> Self {
        self.args.push((name, value.to_string()));
        self
    }

    /// Picks the plural form for `n` (by the locale's CLDR rule; `zero` when
    /// given and `n` is 0) and sets `%{count}`.
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("fr", "fr:\n  posts:\n    one: \"%{count} article\"\n    other: \"%{count} articles\"\n")])
    /// });
    /// assert_eq!(LOCALES.locale("fr").t("posts").count(0).to_string(), "0 article");
    /// assert_eq!(LOCALES.locale("fr").t("posts").count(2_usize).to_string(), "2 articles");
    /// ```
    pub fn count(mut self, n: impl Count) -> Self {
        self.count = Some(n.to_count());
        self
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, strict: bool) -> fmt::Result {
        let I18n { catalog, index } = self.i18n;
        let Some(text) = catalog.lookup(index, self.key, self.count, strict) else {
            return write!(f, "translation missing: {}.{}", self.i18n.locale(), self.key);
        };
        let mut rest = text;
        while let Some(start) = rest.find("%{") {
            let Some(len) = rest[start..].find('}') else { break };
            f.write_str(&rest[..start])?;
            let name = &rest[start + 2..start + len];
            match self.args.iter().find(|(arg, _)| *arg == name) {
                Some((_, value)) => f.write_str(value)?,
                None => match self.count {
                    Some(count) if name == "count" => write!(f, "{count}")?,
                    _ => f.write_str(&rest[start..=start + len])?,
                },
            }
            rest = &rest[start + len + 1..];
        }
        f.write_str(rest)
    }
}

/// Numbers [`Translation::count`] accepts: every integer type, and
/// references to them (askama passes template fields by reference).
/// Values beyond `i64` count as `i64::MAX`.
///
/// ```
/// use ocre::i18n::Count;
/// assert_eq!(3_usize.to_count(), 3);
/// assert_eq!((&-2_i32).to_count(), -2);
/// assert_eq!(u64::MAX.to_count(), i64::MAX);
/// ```
pub trait Count {
    /// The number as `i64`.
    fn to_count(&self) -> i64;
}

macro_rules! count_via_try_from {
    ($($ty:ty),*) => {$(
        impl Count for $ty {
            fn to_count(&self) -> i64 {
                i64::try_from(*self).unwrap_or(i64::MAX)
            }
        }
    )*};
}

count_via_try_from!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

impl<T: Count + ?Sized> Count for &T {
    fn to_count(&self) -> i64 {
        (**self).to_count()
    }
}

/// Debug builds (`ocre dev`) show missing translations instead of falling back.
impl fmt::Display for Translation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, cfg!(debug_assertions))
    }
}

#[cfg(test)]
#[path = "../tests/i18n.rs"]
mod tests;

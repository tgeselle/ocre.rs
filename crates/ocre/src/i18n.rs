//! Translations: `locales/*.yml`, `%{name}` interpolation, plurals, locale per request.
//!
//! Works like Rails' `I18n.t`: YAML files in `locales/` compiled into the
//! Worker, `%{name}` interpolation, CLDR plurals, fallback to the default
//! locale and locale selection per request.
//!
//! ```yaml
//! # locales/fr.yml (a YAML subset: nested keys and strings)
//! fr:
//!   posts:
//!     created: "Article créé."
//!     greeting: "Bonjour %{name} !"
//!     count:
//!       one: "%{count} article"     # CLDR categories: zero, one, two, few, many, other
//!       other: "%{count} articles"
//! ```
//!
//! An app declares its locales once in `src/lib.rs` (`ocre g locale en fr`
//! writes it) and adds [`layer`] at the end of
//! `routes()`; handlers then take the [`I18n`] extractor:
//!
//! ```ignore
//! // Not compiled here: `locales!` includes `locales/en.yml` and `locales/fr.yml` from the app's root.
//! static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr");
//! ```
//!
//! ```
//! use std::sync::LazyLock;
//! use axum::{Router, routing::get};
//! use ocre::{Ctx, i18n::{Catalog, I18n, Locales}};
//!
//! // `ocre::locales!("en", "fr")` expands to this, with the files' contents.
//! static LOCALES: Locales = LazyLock::new(|| {
//!     Catalog::load(&[
//!         ("en", "en:\n  posts:\n    count:\n      one: \"%{count} post\"\n      other: \"%{count} posts\"\n"),
//!         ("fr", "fr:\n  posts:\n    count:\n      one: \"%{count} article\"\n      other: \"%{count} articles\"\n"),
//!     ])
//! });
//!
//! async fn home(i18n: I18n) -> String {
//!     i18n.t("posts.count").count(3).to_string() // "3 articles" for French visitors
//! }
//!
//! fn routes() -> Router<Ctx> {
//!     Router::new()
//!         .route("/", get(home))
//!         // ocre:routes
//!         .layer(ocre::i18n::layer(&LOCALES))
//! }
//! # let _ = routes;
//!
//! // Outside requests (mailers, jobs):
//! assert_eq!(LOCALES.locale("fr").t("posts.count").count(3).to_string(), "3 articles");
//! ```
//!
//! In templates, translations are [`Display`](std::fmt::Display) values that
//! askama writes (escaped) straight into the page:
//! `<p>{{ i18n.t("posts.greeting").arg("name", user.name) }}</p>`. Never mark
//! them `|safe`; for a translation containing markup, end with
//! [`.html()`](Translation::html), which escapes the values only.
//!
//! Beyond `t`: [`I18n::l`] formats dates and times, [`I18n::number`] and
//! [`I18n::currency`] numbers, [`I18n::model_name`] and [`I18n::attribute`]
//! name models and fields, [`I18n::error_message`] translates validation
//! errors, and [`I18n::time_ago_in_words`] durations. Their texts come from
//! the app's files first, then from built-in translations (compiled in, no
//! parsing) for English, French, German, Spanish, Italian, Portuguese and
//! Dutch, under the rails-i18n keys (`date.formats.short`,
//! `errors.messages.blank`...).
//!
//! Each file is parsed once per Worker instance, the first time a key of its
//! locale is looked up (a request in French parses `fr`, and the default
//! locale's file only for a key `fr` lacks), not every declared locale, by a
//! small parser for this YAML subset; lookups are a `BTreeMap` search.
//! No D1, KV or other billed resource is used. [`I18n`] picks the locale from
//! a `{locale}` path segment, then the `locale` cookie, then
//! `Accept-Language`, then the default locale. A key missing from the
//! request's locale falls back to the default locale in release builds
//! (`ocre deploy`); debug builds (`ocre dev`) show
//! `translation missing: fr.posts.created` instead, so gaps are visible.
//! `ocre i18n missing` lists them (see [`check`]).

mod builtin;
mod format;
mod model;
mod plural;
mod yaml;

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{LazyLock, OnceLock},
};

use axum::{
    Extension, Router,
    extract::{FromRequestParts, RawPathParams, Request},
    http::{HeaderMap, HeaderValue, Method, header, request::Parts},
    middleware::{Next, from_fn},
    response::{IntoResponse, Redirect, Response},
};

use crate::{Error, Result};
use plural::{Category, Rule};

/// Name of the cookie that remembers a visitor's chosen locale.
///
/// [`I18n`] reads it after the path segment; [`I18n::cookie`] builds its
/// `Set-Cookie` value.
pub const LOCALE_COOKIE: &str = "locale";

/// Name of the path parameter [`I18n`] reads the locale from.
///
/// Nest localized routes under it: `.nest("/{locale}", pages())`. An unknown
/// code in the path is a 404.
pub const LOCALE_PARAM: &str = "locale";

/// The app's translations: a [`Catalog`] parsed on first use, declared once as a `static`.
///
/// Build it with [`locales!`](crate::locales), which compiles the files into
/// the binary; the first access in a Worker instance parses them, once, and
/// later accesses reuse the parsed tables.
///
/// # Examples
///
/// ```ignore
/// // Not compiled here: `locales!` includes `locales/*.yml` from the app's root.
/// static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr");
/// ```
///
/// Without files, e.g. in tests:
///
/// ```
/// use std::sync::LazyLock;
/// static LOCALES: ocre::i18n::Locales =
///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n  title: Posts\n")]));
/// assert_eq!(LOCALES.locale("en").t("title").to_string(), "Posts");
/// ```
pub type Locales = LazyLock<Catalog>;

/// Declares the app's locales as a [`Locales`] static, compiling `locales/<code>.yml` into the binary.
///
/// `ocre::locales!("en", "fr")` includes `locales/en.yml` and
/// `locales/fr.yml` (paths from the app root, `CARGO_MANIFEST_DIR`) with
/// `include_str!` and hands them to [`Catalog::load`](crate::i18n::Catalog::load)
/// on first use. The first code is the default locale, used for fallbacks. A
/// missing file is a compile error; a file that does not parse is logged and
/// listed in [`Catalog::errors`](crate::i18n::Catalog::errors) (`ocre dev` and
/// `ocre deploy` refuse it earlier).
///
/// Options follow the codes after a `;`: `unprefixed_default` serves the
/// default locale's pages without a prefix ([`Catalog::unprefixed_default`](crate::i18n::Catalog::unprefixed_default)).
///
/// # Examples
///
/// ```ignore
/// // Not compiled here: needs `locales/en.yml` and `locales/fr.yml` next to the app's Cargo.toml.
/// static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr");
/// // `/pricing` in English, `/fr/pricing` in French:
/// static LOCALES: ocre::i18n::Locales = ocre::locales!("en", "fr"; unprefixed_default);
/// ```
#[macro_export]
macro_rules! locales {
    ($($code:literal),+ $(,)? $(; $option:ident)*) => {
        ::std::sync::LazyLock::new(|| {
            $crate::i18n::Catalog::load(&[$((
                $code,
                ::std::include_str!(::std::concat!(::std::env!("CARGO_MANIFEST_DIR"), "/locales/", $code, ".yml")),
            )),+])
            $(.$option())*
        })
    };
}

/// Makes the catalog available to the [`I18n`] extractor, as an axum layer.
///
/// Forces `locales` (the files themselves are parsed on first use) and returns
/// an [`Extension`] holding it. Add it last in `routes()`, after the
/// `// ocre:routes` marker, so it covers every route; without it, [`I18n`]
/// fails with a 500 whose log line names this fix.
///
/// # Examples
///
/// ```
/// use std::sync::LazyLock;
/// use axum::{Router, routing::get};
/// use ocre::{Ctx, i18n::{Catalog, I18n, Locales}};
///
/// static LOCALES: Locales = LazyLock::new(|| Catalog::load(&[("en", "en:\n  title: Posts\n")]));
///
/// async fn home(i18n: I18n) -> String {
///     i18n.t("title").to_string()
/// }
///
/// let _app: Router<Ctx> = Router::new()
///     .route("/", get(home))
///     // ocre:routes
///     .layer(ocre::i18n::layer(&LOCALES));
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

/// One locale file, parsed on first use: a Worker instance only pays for
/// the locales its requests use (the default one and the visitor's).
#[derive(Debug)]
struct Table {
    code: &'static str,
    rule: Rule,
    source: &'static str,
    parsed: OnceLock<Parsed>,
}

/// A parsed file: its values (empty when it does not parse) and the parse error.
#[derive(Debug)]
struct Parsed {
    values: BTreeMap<String, Value<'static>>,
    error: Option<String>,
}

impl Table {
    fn new(code: &'static str, source: &'static str) -> Self {
        Self { code, rule: Rule::for_locale(code), source, parsed: OnceLock::new() }
    }

    /// Parses the file the first time; a parse error is logged once.
    fn parsed(&self) -> &Parsed {
        self.parsed.get_or_init(|| match yaml::parse_locale(self.code, self.source) {
            Ok(entries) => Parsed { values: group(entries), error: None },
            Err(err) => {
                let error = format!("locales/{}.yml line {}: {}", self.code, err.line, err.message);
                crate::error::log_internal(&error);
                Parsed { values: BTreeMap::new(), error: Some(error) }
            }
        })
    }

    fn values(&self) -> &BTreeMap<String, Value<'static>> {
        &self.parsed().values
    }

    /// Tables live in a `static` [`Locales`], so their strings are `'static`.
    fn get(&'static self, key: &str, count: Option<i64>) -> Option<&'static str> {
        let text = match (self.values().get(key)?, count) {
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

const NO_LOCALES: &str = "no locales: declare them with ocre::locales!(\"en\")";

/// All locales of an app: one table of dotted keys per locale code, the first being the default.
///
/// Built by [`locales!`](crate::locales) inside a [`Locales`] static. Each
/// file is parsed the first time one of its keys is looked up, so adding
/// locales costs a Worker instance nothing until a request uses them. Files
/// that fail to parse are empty (so every key falls back to the default
/// locale), logged to the Worker logs when first used and listed in
/// [`errors`](Self::errors). [`locale`](Self::locale) gives translations
/// outside requests; in handlers, the [`I18n`] extractor picks the locale.
///
/// # Examples
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
    /// No locale was declared.
    undeclared: bool,
    errors: OnceLock<Vec<String>>,
    /// The default locale's URLs carry no prefix (`/pricing`, `/fr/pricing`).
    unprefixed_default: bool,
}

impl Catalog {
    /// Builds a catalog of `(code, file contents)` pairs; the first pair is the default locale.
    ///
    /// Each file must start with its code as the root key (`en:`); plural
    /// rules come from the code (`fr-CH` uses French rules). Files are
    /// parsed on first use; one that does not parse becomes an empty table
    /// and one entry in [`errors`](Self::errors), logged. An empty `sources`
    /// gives an empty `en` catalog with an error telling to declare locales.
    /// Never fails: missing translations show up at lookup time instead.
    ///
    /// # Examples
    ///
    /// ```
    /// let catalog = ocre::i18n::Catalog::load(&[("en", "en:\n  title: Posts\n")]);
    /// assert!(catalog.errors().is_empty());
    /// ```
    pub fn load(sources: &[(&'static str, &'static str)]) -> Self {
        let mut tables: Vec<Table> = sources.iter().map(|(code, text)| Table::new(code, text)).collect();
        let undeclared = tables.is_empty();
        if undeclared {
            crate::error::log_internal(NO_LOCALES);
            tables.push(Table::new("en", "en:\n"));
        }
        Self { tables, undeclared, errors: OnceLock::new(), unprefixed_default: false }
    }

    /// Serves the default locale's pages without a locale prefix: `/pricing` in English, `/fr/pricing` in French.
    ///
    /// By default every locale has its prefix (`/en/pricing`). With this
    /// option, [`I18n::path`], [`I18n::alternates`] and
    /// [`I18n::alternate_links`] leave the default locale's paths as they
    /// are, and [`localized`] serves pages both ways. Declare it with the
    /// locales: `ocre::locales!("en", "fr"; unprefixed_default)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n"), ("fr", "fr:\n")]).unprefixed_default());
    /// assert_eq!(LOCALES.locale("en").path("/pricing"), "/pricing");
    /// assert_eq!(LOCALES.locale("fr").path("/pricing"), "/fr/pricing");
    /// ```
    pub fn unprefixed_default(mut self) -> Self {
        self.unprefixed_default = true;
        self
    }

    /// Parse errors of the locale files, one line each (`locales/fr.yml line 3: ...`).
    ///
    /// Parses every file not parsed yet. Empty when every file parsed.
    /// `ocre dev` and `ocre deploy` check the same files before building, so
    /// a deployed app normally has none.
    ///
    /// # Examples
    ///
    /// ```
    /// let catalog = ocre::i18n::Catalog::load(&[("en", "en:\n  title: [Posts]\n")]);
    /// assert_eq!(catalog.errors().len(), 1);
    /// assert!(catalog.errors()[0].starts_with("locales/en.yml line 2: "));
    /// ```
    pub fn errors(&self) -> &[String] {
        self.errors.get_or_init(|| {
            let parse_errors = self.tables.iter().filter_map(|table| table.parsed().error.clone());
            if self.undeclared { vec![NO_LOCALES.to_owned()] } else { parse_errors.collect() }
        })
    }

    /// Returns the default locale code: the first code given to [`locales!`](crate::locales).
    ///
    /// Keys missing from another locale fall back to it in release builds,
    /// and requests without a usable locale get it.
    ///
    /// # Examples
    ///
    /// ```
    /// let catalog = ocre::i18n::Catalog::load(&[("fr", "fr:\n"), ("en", "en:\n")]);
    /// assert_eq!(catalog.default_locale(), "fr");
    /// ```
    pub fn default_locale(&self) -> &'static str {
        self.tables[0].code
    }

    /// Returns every locale code, default first, e.g. for a language switcher.
    ///
    /// # Examples
    ///
    /// ```
    /// let catalog = ocre::i18n::Catalog::load(&[("en", "en:\n"), ("pt-BR", "pt-BR:\n")]);
    /// assert_eq!(catalog.codes().collect::<Vec<_>>(), ["en", "pt-BR"]);
    /// ```
    pub fn codes(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.tables.iter().map(|table| table.code)
    }

    /// Returns translations in locale `code`, for code outside requests such as mailers and jobs.
    ///
    /// `code` matches case-insensitively and exactly (`fr-CH` does not match
    /// `fr` here, unlike `Accept-Language`); an unknown code gives the
    /// default locale. Requires a `'static` catalog, i.e. a [`Locales`]
    /// static.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("en", "en:\n  subject: Welcome\n"), ("fr", "fr:\n  subject: Bienvenue\n")])
    /// });
    ///
    /// let user_locale = "FR";
    /// assert_eq!(LOCALES.locale(user_locale).t("subject").to_string(), "Bienvenue");
    /// assert_eq!(LOCALES.locale("de").locale(), "en");
    /// ```
    pub fn locale(&'static self, code: &str) -> I18n {
        I18n { catalog: self, index: self.find(code).unwrap_or(0), scope: "" }
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

/// A problem in the locale files, found by [`check`].
///
/// Displays as one line pointing at the file, as `ocre i18n missing` prints
/// it: `locales/fr.yml: missing posts.title`.
///
/// # Examples
///
/// ```
/// use ocre::i18n::Problem;
///
/// let missing = Problem::Missing { locale: "fr".into(), key: "posts.title".into() };
/// assert_eq!(missing.to_string(), "locales/fr.yml: missing posts.title");
/// let invalid = Problem::Invalid { locale: "fr".into(), line: 3, message: "expected `key: value`".into() };
/// assert_eq!(invalid.to_string(), "locales/fr.yml:3: expected `key: value`");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// The file is not valid locale YAML; nothing in it is used.
    Invalid {
        /// Locale code of the file, e.g. `fr` for `locales/fr.yml`.
        locale: String,
        /// 1-based line of the first error.
        line: usize,
        /// What is wrong, and how to fix it.
        message: String,
    },
    /// A key of the default locale (or a plural form this language needs)
    /// is missing from `locale`.
    Missing {
        /// Locale code of the file lacking the key.
        locale: String,
        /// Full dotted key, e.g. `posts.title` or `posts.count.few`.
        key: String,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { locale, line, message } => write!(f, "locales/{locale}.yml:{line}: {message}"),
            Self::Missing { locale, key } => write!(f, "locales/{locale}.yml: missing {key}"),
        }
    }
}

/// Checks locale files for syntax errors and keys missing from non-default locales.
///
/// Reads `(code, file contents)` pairs the way [`Catalog::load`] does: first
/// the syntax of each file ([`Problem::Invalid`]), then every key of the
/// default (first) locale in every other one ([`Problem::Missing`]), with the
/// plural forms each language needs (Russian needs `one`, `few`, `many` and
/// `other`; Japanese only `other`). A plural key present as plain text counts
/// as present. When the default file is invalid, only syntax problems are
/// reported. Nothing is logged; `ocre i18n missing` prints the result and
/// fails when it is not empty.
///
/// # Examples
///
/// ```
/// use ocre::i18n::{Problem, check};
///
/// let problems = check(&[
///     ("en", "en:\n  title: Posts\n  count:\n    one: \"%{count} post\"\n    other: \"%{count} posts\"\n"),
///     ("fr", "fr:\n  count:\n    one: \"%{count} article\"\n    other: \"%{count} articles\"\n"),
/// ]);
/// assert_eq!(problems, [Problem::Missing { locale: "fr".into(), key: "title".into() }]);
///
/// // Russian needs more plural forms than English.
/// let problems = check(&[
///     ("en", "en:\n  count:\n    one: \"%{count} post\"\n    other: \"%{count} posts\"\n"),
///     ("ru", "ru:\n  count:\n    one: \"%{count} пост\"\n    other: \"%{count} поста\"\n"),
/// ]);
/// let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
/// assert_eq!(lines, ["locales/ru.yml: missing count.few", "locales/ru.yml: missing count.many"]);
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

/// Axum extractor giving translations in the request's locale, like Rails' `I18n.t` with a per-request locale.
///
/// The locale comes from the `{locale}` path segment (routes nested with
/// `.nest("/{locale}", ..)`, see [`LOCALE_PARAM`]), else the `locale` cookie
/// ([`LOCALE_COOKIE`]), else `Accept-Language` (by quality; `fr-CH` matches
/// `fr`, `pt` matches `pt-BR`), else the default locale. It is `Copy`: pass
/// it by value to template structs, where `{{ i18n.t("posts.title") }}`
/// writes the text. Selecting the locale reads headers only; no billed
/// resource is used.
///
/// Rejections render as HTML pages in full-stack apps and as JSON in
/// API-only apps:
/// - [`Error::NotFound`] (404) when the `{locale}`
///   path segment is not one of the app's locales;
/// - [`Error::Internal`] (500) when [`layer`] is
///   missing; the log line names the fix.
///
/// # Examples
///
/// ```
/// use axum::{Router, routing::get};
/// use ocre::{Ctx, i18n::I18n};
///
/// async fn title(i18n: I18n) -> String {
///     i18n.t("posts.title").to_string()
/// }
///
/// // `/en/posts`, `/fr/posts`; `/de/posts` is a 404 unless the app has German.
/// fn routes() -> Router<Ctx> {
///     Router::new().nest("/{locale}", Router::new().route("/posts", get(title)))
/// }
/// # let _ = routes;
/// ```
#[derive(Clone, Copy)]
pub struct I18n {
    catalog: &'static Catalog,
    index: usize,
    /// Prefix of keys starting with `.`, set by [`scope`](Self::scope).
    scope: &'static str,
}

impl fmt::Debug for I18n {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("I18n").field("locale", &self.locale()).finish()
    }
}

impl I18n {
    /// Returns the locale code, e.g. for `<html lang="{{ i18n.locale() }}">`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n")]));
    /// assert_eq!(LOCALES.locale("en").locale(), "en");
    /// ```
    pub fn locale(&self) -> &'static str {
        self.catalog.tables[self.index].code
    }

    /// Returns every locale code of the app, default first, e.g. for a language switcher.
    ///
    /// # Examples
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

    /// Starts the translation of a dotted key, like Rails' `t("posts.created")`.
    ///
    /// The returned [`Translation`] is a [`Display`](fmt::Display) value:
    /// askama writes it (escaped) into the page without an extra `String`,
    /// `.to_string()` gives one (for flashes, emails). Add `%{name}` values
    /// with [`Translation::arg`] and the plural count with
    /// [`Translation::count`]. The lookup happens when it is displayed.
    ///
    /// # Examples
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
        Translation {
            i18n: *self,
            key: self.resolve(key),
            alternatives: Vec::new(),
            default: None,
            count: None,
            args: Vec::new(),
        }
    }

    /// Returns the same translations with `scope` as the prefix of keys starting with `.` (Rails' lazy lookup).
    ///
    /// `i18n.scope("posts.index").t(".title")` looks up `posts.index.title`;
    /// keys without a leading dot stay absolute. Rails derives the scope from
    /// the view's path; in Ocre, the handler sets it before passing `i18n` to
    /// its template. A scope replaces the previous one.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("en", "en:\n  app:\n    name: Blog\n  posts:\n    index:\n      title: Posts\n")])
    /// });
    /// let i18n = LOCALES.locale("en").scope("posts.index");
    /// assert_eq!(i18n.t(".title").to_string(), "Posts");
    /// assert_eq!(i18n.t("app.name").to_string(), "Blog");
    /// ```
    pub fn scope(&self, scope: &'static str) -> Self {
        Self { scope, ..*self }
    }

    /// Returns the same translations in locale `code` (Rails' `locale:` option), keeping the scope.
    ///
    /// `code` matches like [`Catalog::locale`]: exactly, case-insensitively,
    /// and an unknown code gives the default locale.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("en", "en:\n  hello: Hello\n"), ("fr", "fr:\n  hello: Bonjour\n")])
    /// });
    /// let i18n = LOCALES.locale("en");
    /// assert_eq!(i18n.in_locale("fr").t("hello").to_string(), "Bonjour");
    /// assert_eq!(i18n.in_locale("de").locale(), "en");
    /// ```
    pub fn in_locale(&self, code: &str) -> Self {
        Self { index: self.catalog.find(code).unwrap_or(0), ..*self }
    }

    /// Whether `key` has a translation: in the locale, the default locale or the built-in translations.
    ///
    /// Keys starting with `.` are relative to the [`scope`](Self::scope).
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n  posts:\n    title: Posts\n")]));
    /// let i18n = LOCALES.locale("en");
    /// assert!(i18n.exists("posts.title"));
    /// assert!(i18n.exists("errors.messages.blank"), "built in");
    /// assert!(!i18n.exists("posts"), "a namespace is not a translation");
    /// ```
    pub fn exists(&self, key: &str) -> bool {
        self.lookup(&[&self.resolve(key)], None, false).is_some()
    }

    /// Returns every translation under `prefix`, by key relative to it (Rails' namespace lookup).
    ///
    /// Nested keys keep their dots (`form.submit`); plural keys give their
    /// `other` form. Only the app's files are read. Release builds add the
    /// default locale's keys missing from the locale, as [`t`](Self::t) falls
    /// back to them. Keys starting with `.` are relative to the
    /// [`scope`](Self::scope). Handy to hand a group of texts to JavaScript:
    /// `Json(i18n.namespace("editor"))`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("en", "en:\n  editor:\n    bold: Bold\n    link:\n      title: Link\n  other: x\n")])
    /// });
    /// let texts = LOCALES.locale("en").namespace("editor");
    /// assert_eq!(texts.into_iter().collect::<Vec<_>>(), [("bold", "Bold"), ("link.title", "Link")]);
    /// ```
    pub fn namespace(&self, prefix: &str) -> BTreeMap<&'static str, &'static str> {
        self.namespace_in(prefix, cfg!(debug_assertions))
    }

    fn namespace_in(&self, prefix: &str, strict: bool) -> BTreeMap<&'static str, &'static str> {
        let start = format!("{}.", self.resolve(prefix));
        let mut texts = BTreeMap::new();
        let tables = if strict || self.index == 0 { &[self.index][..] } else { &[0, self.index][..] };
        for &index in tables {
            let table: &'static Table = &self.catalog.tables[index];
            for (key, _) in table.values().range(start.clone()..).take_while(|(key, _)| key.starts_with(&start)) {
                if let Some(text) = table.get(key, None) {
                    texts.insert(&key[start.len()..], text);
                }
            }
        }
        texts
    }

    /// Prefixes `path` with the locale, for routes nested under `/{locale}` (Rails' `default_url_options`).
    ///
    /// `i18n.path("/posts")` is `/fr/posts` for French visitors, so links
    /// keep the locale; `"/"` gives `/fr`. Use it in templates:
    /// `<a href="{{ i18n.path("/posts") }}">`. With
    /// [`unprefixed_default`](Catalog::unprefixed_default), the default
    /// locale's paths stay as they are (`/posts`).
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n"), ("fr", "fr:\n")]));
    /// let i18n = LOCALES.locale("fr");
    /// assert_eq!(i18n.path("/posts/3"), "/fr/posts/3");
    /// assert_eq!(i18n.path("/"), "/fr");
    /// ```
    pub fn path(&self, path: &str) -> String {
        let rest = path.trim_start_matches('/');
        match rest {
            _ if self.catalog.unprefixed_default && self.index == 0 => format!("/{rest}"),
            "" => format!("/{}", self.locale()),
            rest => format!("/{}/{rest}", self.locale()),
        }
    }

    /// The text direction of the locale, `rtl` or `ltr`, for `<html dir="{{ i18n.dir() }}">`.
    ///
    /// Right to left for Arabic, Hebrew, Persian, Urdu, Pashto, Sindhi,
    /// Uyghur, Yiddish, Dhivehi and Central Kurdish (by language, so `ar-EG`
    /// too).
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n"), ("ar-EG", "ar-EG:\n")]));
    /// assert_eq!(LOCALES.locale("ar-EG").dir(), "rtl");
    /// assert_eq!(LOCALES.locale("en").dir(), "ltr");
    /// ```
    pub fn dir(&self) -> &'static str {
        const RTL: [&str; 10] = ["ar", "he", "fa", "ur", "ps", "sd", "ug", "yi", "dv", "ckb"];
        if RTL.iter().any(|language| primary(self.locale()).eq_ignore_ascii_case(language)) { "rtl" } else { "ltr" }
    }

    /// The absolute URL of `path` in every locale, the default first:
    /// `(code, base_url + "/<code>" + path)`, for a sitemap's alternates
    /// ([`Sitemap::add_localized`](crate::seo::Sitemap::add_localized)).
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n"), ("zh-Hans", "zh-Hans:\n")]));
    /// let urls = LOCALES.locale("zh-Hans").alternates("https://ex.com/", "/pricing");
    /// assert_eq!(urls, [("en", "https://ex.com/en/pricing".to_owned()), ("zh-Hans", "https://ex.com/zh-Hans/pricing".to_owned())]);
    /// ```
    pub fn alternates(&self, base_url: &str, path: &str) -> Vec<(&'static str, String)> {
        let base = base_url.trim_end_matches('/');
        self.codes().map(|code| (code, format!("{base}{}", self.in_locale(code).path(path)))).collect()
    }

    /// The `<link>` elements a localized page's `<head>` needs: `canonical`
    /// (this locale's URL), one `alternate` per locale with its `hreflang`,
    /// and `x-default` (the default locale's URL). `base_url` is the app's
    /// public origin (`APP_URL`, [`ocre::mail::url`](crate::mail::url)),
    /// `path` the page without its locale prefix. Values are escaped; in
    /// askama: `{{ i18n.alternate_links(base_url, "/pricing")|safe }}`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n"), ("fr", "fr:\n")]));
    /// assert_eq!(
    ///     LOCALES.locale("fr").alternate_links("https://ex.com", "/"),
    ///     "<link rel=\"canonical\" href=\"https://ex.com/fr\">\n\
    ///      <link rel=\"alternate\" hreflang=\"en\" href=\"https://ex.com/en\">\n\
    ///      <link rel=\"alternate\" hreflang=\"fr\" href=\"https://ex.com/fr\">\n\
    ///      <link rel=\"alternate\" hreflang=\"x-default\" href=\"https://ex.com/en\">"
    /// );
    /// ```
    pub fn alternate_links(&self, base_url: &str, path: &str) -> String {
        let base = base_url.trim_end_matches('/');
        let attribute = |text: &str| text.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
        let mut links =
            vec![format!("<link rel=\"canonical\" href=\"{}\">", attribute(&format!("{base}{}", self.path(path))))];
        let alternates = self.alternates(base, path);
        for (code, url) in &alternates {
            links.push(format!(
                "<link rel=\"alternate\" hreflang=\"{}\" href=\"{}\">",
                attribute(code),
                attribute(url)
            ));
        }
        if let Some((_, default)) = alternates.first() {
            links.push(format!("<link rel=\"alternate\" hreflang=\"x-default\" href=\"{}\">", attribute(default)));
        }
        links.join("\n")
    }

    /// `.title` with the scope `posts.index` is `posts.index.title`.
    fn resolve<'a>(&self, key: &'a str) -> Cow<'a, str> {
        match key.strip_prefix('.') {
            Some(rest) if !self.scope.is_empty() => Cow::Owned(format!("{}.{rest}", self.scope)),
            Some(rest) => Cow::Borrowed(rest),
            None => Cow::Borrowed(key),
        }
    }

    /// The first of `keys` found in: the locale's file, the built-in
    /// translations of its language, then (unless `strict`) the default
    /// locale's file and its built-in translations, then built-in English.
    fn lookup(&self, keys: &[&str], count: Option<i64>, strict: bool) -> Option<&'static str> {
        let catalog: &'static Catalog = self.catalog;
        let own = &catalog.tables[self.index];
        let default = &catalog.tables[0];
        let app = |table: &'static Table| keys.iter().find_map(|key| table.get(key, count));
        let built_in = |code: &str| keys.iter().find_map(|key| builtin::get(code, key, count));
        app(own)
            .or_else(|| built_in(own.code))
            .or_else(|| if strict { None } else { app(default).or_else(|| built_in(default.code)) })
            .or_else(|| built_in("en"))
    }

    /// Returns a `Set-Cookie` value remembering this locale for a year.
    ///
    /// The cookie is `locale=<code>; Path=/; Max-Age=31536000; SameSite=Lax`;
    /// later requests without a `{locale}` path segment use it. Send it
    /// when the visitor picks a language.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::LazyLock;
    /// use axum::{extract::Path, http::header, response::{IntoResponse, Redirect}};
    ///
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n"), ("fr", "fr:\n")]));
    ///
    /// // POST /locale/{code}
    /// async fn switch(Path(code): Path<String>) -> impl IntoResponse {
    ///     let cookie = LOCALES.locale(&code).cookie(); // unknown codes give the default
    ///     ([(header::SET_COOKIE, cookie)], Redirect::to("/"))
    /// }
    /// # let _ = switch;
    ///
    /// assert_eq!(LOCALES.locale("fr").cookie(), "locale=fr; Path=/; Max-Age=31536000; SameSite=Lax");
    /// ```
    pub fn cookie(&self) -> String {
        format!("{LOCALE_COOKIE}={}; Path=/; Max-Age=31536000; SameSite=Lax", self.locale())
    }

    /// The extractor's logic: the catalog put there by [`layer`], then [`select`](Self::select).
    fn from_parts(
        catalog: Option<&'static Catalog>,
        path: Option<&str>,
        default_page: bool,
        headers: &HeaderMap,
    ) -> Result<Self> {
        let catalog = catalog.ok_or_else(|| {
            Error::internal(
                "the I18n extractor needs the catalog. Fix: add `.layer(ocre::i18n::layer(&LOCALES))` at the end of \
                 routes() in src/lib.rs (`ocre g locale` does it)",
            )
        })?;
        if default_page {
            return Ok(Self { catalog, index: 0, scope: "" });
        }
        Self::select(catalog, path, headers)
    }

    /// The locale of a request: path segment, cookie, `Accept-Language`, default.
    fn select(catalog: &'static Catalog, path: Option<&str>, headers: &HeaderMap) -> Result<Self> {
        if let Some(code) = path {
            return catalog.find(code).map(|index| Self { catalog, index, scope: "" }).ok_or(Error::NotFound);
        }
        Ok(Self { catalog, index: catalog.preferred(headers).unwrap_or(0), scope: "" })
    }
}

impl Catalog {
    /// The visitor's locale from the `locale` cookie, else `Accept-Language`.
    fn preferred(&self, headers: &HeaderMap) -> Option<usize> {
        self.cookie_locale(headers).or_else(|| {
            let accept = headers.get(header::ACCEPT_LANGUAGE)?.to_str().ok()?;
            self.negotiate(accept)
        })
    }

    /// The locale the `locale` cookie names, if it is one of the app's.
    fn cookie_locale(&self, headers: &HeaderMap) -> Option<usize> {
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .filter_map(|pair| pair.trim().strip_prefix(LOCALE_COOKIE)?.strip_prefix('='))
            .find_map(|code| self.find(code))
    }

    /// [`locale`](Self::locale) by index, on a `'static` catalog.
    fn locale_ref(&'static self, index: usize) -> I18n {
        I18n { catalog: self, index, scope: "" }
    }
}

/// Marks a request served by the unprefixed copy of [`localized`] pages: the URL says the default locale.
#[derive(Debug, Clone, Copy)]
struct DefaultLocalePage;

/// Serves `pages` in every locale: `/path` in the default locale, `/<code>/path` in the others.
///
/// For public pages search engines index in each language (Rails' `scope
/// "(:locale)"`), with the catalog declared `unprefixed_default`
/// (`ocre::locales!("en", "fr"; unprefixed_default)`): the URL decides the
/// language, so each page has one address per locale for `hreflang`
/// ([`I18n::alternate_links`]).
///
/// - `/pricing` is the default locale's page. A visitor whose `locale`
///   cookie or `Accept-Language` prefers another locale of the app gets a
///   `303 See Other` to it (`/fr/pricing`); crawlers send neither, so they index each
///   URL in its own language. Only `GET` and `HEAD` are redirected.
/// - `/fr/pricing` is French, and remembers the choice in the `locale`
///   cookie, so the visitor's next unprefixed URL leads back to French.
/// - `/en/pricing` (the default locale's code) answers `308 Permanent Redirect` to `/pricing`;
///   an unknown code (`/xx/pricing`) is a `404`.
///
/// Pages outside `localized` keep the usual choice (cookie, then
/// `Accept-Language`). A language switcher sets the cookie with
/// [`I18n::cookie`] and links to [`I18n::path`]. No billed resource is
/// used: headers only.
///
/// # Errors
///
/// A request answers 500 when [`layer`] is missing or the catalog is not
/// declared `unprefixed_default`; the log line names the fix.
///
/// # Examples
///
/// ```
/// use std::sync::LazyLock;
/// use axum::{Router, routing::get};
/// use ocre::{Ctx, i18n::{Catalog, I18n, Locales}};
///
/// static LOCALES: Locales =
///     LazyLock::new(|| Catalog::load(&[("en", "en:\n  title: Pricing\n"), ("fr", "fr:\n  title: Tarifs\n")]).unprefixed_default());
///
/// async fn pricing(i18n: I18n) -> String {
///     i18n.t("title").to_string()
/// }
///
/// fn routes() -> Router<Ctx> {
///     let pages = Router::new().route("/pricing", get(pricing));
///     Router::new()
///         .merge(ocre::i18n::localized(pages)) // /pricing, /fr/pricing
///         // ocre:routes
///         .layer(ocre::i18n::layer(&LOCALES))
/// }
/// # let _ = routes;
/// ```
pub fn localized<S: Clone + Send + Sync + 'static>(pages: Router<S>) -> Router<S> {
    let prefixed = Router::new().nest(&format!("/{{{LOCALE_PARAM}}}"), pages.clone().layer(from_fn(prefixed_page)));
    pages.layer(from_fn(default_page)).merge(prefixed)
}

/// The catalog of a request, or the error naming the missing layer or option.
fn localized_catalog(request: &Request) -> Result<&'static Catalog> {
    let catalog = request.extensions().get::<&'static Catalog>().copied().ok_or_else(|| {
        Error::internal(
            "localized pages need the catalog. Fix: add `.layer(ocre::i18n::layer(&LOCALES))` at the end of routes() \
             in src/lib.rs (`ocre g locale` does it)",
        )
    })?;
    if !catalog.unprefixed_default {
        return Err(Error::internal(
            "ocre::i18n::localized serves the default locale without a prefix. Fix: declare the locales with \
             `ocre::locales!(\"en\", \"fr\"; unprefixed_default)` in src/lib.rs",
        ));
    }
    Ok(catalog)
}

/// The unprefixed copy: the default locale, or a redirect to the visitor's preferred one.
async fn default_page(mut request: Request, next: Next) -> Response {
    let catalog = match localized_catalog(&request) {
        Ok(catalog) => catalog,
        Err(err) => return crate::session::reject(err).into_response(),
    };
    let read = matches!(*request.method(), Method::GET | Method::HEAD);
    if let Some(index) = catalog.preferred(request.headers()).filter(|&index| index != 0 && read) {
        let uri = request.uri();
        let query = uri.query().map(|query| format!("?{query}")).unwrap_or_default();
        return Redirect::to(&format!("{}{query}", catalog.locale_ref(index).path(uri.path()))).into_response();
    }
    request.extensions_mut().insert(DefaultLocalePage);
    next.run(request).await
}

/// The prefixed copy: `/en/...` to the unprefixed URL, unknown codes to 404, and the choice remembered.
async fn prefixed_page(params: RawPathParams, request: Request, next: Next) -> Response {
    let catalog = match localized_catalog(&request) {
        Ok(catalog) => catalog,
        Err(err) => return crate::session::reject(err).into_response(),
    };
    let code = params.iter().find(|(name, _)| *name == LOCALE_PARAM).map(|(_, code)| code).unwrap_or_default();
    let Some(index) = catalog.find(code) else { return crate::session::reject(Error::NotFound).into_response() };
    if index == 0 {
        // Inside the nested router, the URI is the page's own path, without the locale.
        let uri = request.uri();
        let query = uri.query().map(|query| format!("?{query}")).unwrap_or_default();
        return Redirect::permanent(&format!("{}{query}", uri.path())).into_response();
    }
    let remembered = catalog.cookie_locale(request.headers()) == Some(index);
    let mut response = next.run(request).await;
    if !remembered && let Ok(cookie) = HeaderValue::from_str(&catalog.locale_ref(index).cookie()) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
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
            parts.extensions.get::<DefaultLocalePage>().is_some(),
            &parts.headers,
        )
        .map_err(crate::session::reject)
    }
}

/// A translation being built by [`I18n::t`], displayed (looked up and interpolated) when written.
///
/// Add `%{name}` values with [`arg`](Self::arg) and the plural count with
/// [`count`](Self::count). Displaying it looks the key up in the locale,
/// then (release builds only) in the default locale, and replaces each
/// `%{name}` with its value; a placeholder without a value stays as written.
/// A key missing everywhere, and in debug builds (`ocre dev`, tests) a key
/// missing from the locale, displays as `translation missing: fr.posts.title`.
/// The text is not escaped here: askama escapes it when it writes it.
///
/// # Examples
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
    key: Cow<'a, str>,
    alternatives: Vec<Cow<'a, str>>,
    default: Option<Cow<'a, str>>,
    count: Option<i64>,
    args: Vec<(&'a str, String)>,
}

impl<'a> Translation<'a> {
    /// Sets the value for the `%{name}` placeholder.
    ///
    /// `value` is formatted with [`Display`](fmt::Display) right away.
    /// Calling it twice with the same name keeps the first value.
    ///
    /// # Examples
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

    /// Sets the plural count: picks the form for `n` and fills `%{count}`.
    ///
    /// The form follows the locale's CLDR rule (French: 0 and 1 are `one`;
    /// Russian: `one`, `few`, `many`); a `zero` form, when present, wins
    /// for 0. A missing form falls back to `other`. An explicit
    /// [`arg`](Self::arg)`("count", ..)` overrides the `%{count}` text.
    ///
    /// # Examples
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

    /// Tries another key when this one has no translation (Rails' `default: :"other.key"`).
    ///
    /// Alternatives are tried in the order given, each like the first key
    /// (locale, then default locale in release builds); a leading `.` is
    /// relative to the [`scope`](I18n::scope).
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n  actions:\n    save: Save\n")]));
    /// let i18n = LOCALES.locale("en");
    /// assert_eq!(i18n.t("posts.form.save").or_key("actions.save").to_string(), "Save");
    /// ```
    pub fn or_key(mut self, key: &'a str) -> Self {
        self.alternatives.push(self.i18n.resolve(key));
        self
    }

    /// Uses `text` when neither the key nor its alternatives have a translation (Rails' `default: "text"`).
    ///
    /// It replaces `translation missing: ...` in every build (debug builds
    /// included, where a key missing from the locale shows `text` rather
    /// than the default locale's translation). `%{name}` placeholders in
    /// `text` are filled like in a translation.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n")]));
    /// let i18n = LOCALES.locale("en");
    /// assert_eq!(i18n.t("posts.empty").or("No posts yet, %{name}").arg("name", "Ada").to_string(), "No posts yet, Ada");
    /// ```
    pub fn or(mut self, text: impl Into<Cow<'a, str>>) -> Self {
        self.default = Some(text.into());
        self
    }

    /// Marks the translation as HTML: its text is written as is, the values of [`arg`](Self::arg) escaped.
    ///
    /// Rails' `_html` keys: for translations containing markup, such as
    /// `terms: "I accept the <a href=\"/terms\">terms</a>, %{name}"`. askama
    /// writes the result without escaping it again (feature `html`); the
    /// `%{name}` values are escaped, so user input stays text. Use it only
    /// for keys whose text you wrote, never for text from users.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("en", "en:\n  welcome: \"Hello <b>%{name}</b>\"\n")])
    /// });
    /// let i18n = LOCALES.locale("en");
    /// assert_eq!(i18n.t("welcome").arg("name", "<script>").html().to_string(), "Hello <b>&lt;script&gt;</b>");
    /// ```
    pub fn html(self) -> HtmlTranslation<'a> {
        HtmlTranslation(self)
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, strict: bool) -> fmt::Result {
        self.write_with(f, strict, false)
    }

    fn write_with(&self, f: &mut fmt::Formatter<'_>, strict: bool, escape: bool) -> fmt::Result {
        let mut keys = Vec::with_capacity(1 + self.alternatives.len());
        keys.push(self.key.as_ref());
        keys.extend(self.alternatives.iter().map(AsRef::as_ref));
        let text = match (self.i18n.lookup(&keys, self.count, strict), &self.default) {
            (Some(text), _) => text,
            (None, Some(default)) => default,
            (None, None) => return write!(f, "translation missing: {}.{}", self.i18n.locale(), self.key),
        };
        interpolate(f, text, &self.args, self.count, escape)
    }
}

/// Writes `text` with each `%{name}` replaced by its value in `args` (or
/// `count` for `%{count}`), HTML-escaped when `escape`; a placeholder without
/// a value stays as written.
fn interpolate(
    out: &mut impl fmt::Write,
    text: &str,
    args: &[(&str, String)],
    count: Option<i64>,
    escape: bool,
) -> fmt::Result {
    let mut rest = text;
    while let Some(start) = rest.find("%{") {
        let Some(len) = rest[start..].find('}') else { break };
        out.write_str(&rest[..start])?;
        let name = &rest[start + 2..start + len];
        match args.iter().find(|(arg, _)| *arg == name) {
            Some((_, value)) if escape => write_escaped(out, value)?,
            Some((_, value)) => out.write_str(value)?,
            None => match count {
                Some(count) if name == "count" => write!(out, "{count}")?,
                _ => out.write_str(&rest[start..=start + len])?,
            },
        }
        rest = &rest[start + len + 1..];
    }
    out.write_str(rest)
}

fn write_escaped(out: &mut impl fmt::Write, text: &str) -> fmt::Result {
    let mut done = 0;
    for (at, c) in text.char_indices() {
        let entity = match c {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#39;",
            _ => continue,
        };
        out.write_str(&text[done..at])?;
        out.write_str(entity)?;
        done = at + 1;
    }
    out.write_str(&text[done..])
}

/// A [`Translation`] written as HTML, from [`Translation::html`]: the text as is, every value escaped.
///
/// askama writes it without escaping (it implements
/// `askama::filters::HtmlSafe` with the `html` feature), so
/// `{{ i18n.t("terms").arg("name", user.name).html() }}` keeps the
/// translation's markup while `user.name` stays text.
///
/// # Examples
///
/// ```
/// # use std::sync::LazyLock;
/// static LOCALES: ocre::i18n::Locales =
///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n  note: \"<em>%{text}</em>\"\n")]));
/// let html = LOCALES.locale("en").t("note").arg("text", "a & b").html();
/// assert_eq!(html.to_string(), "<em>a &amp; b</em>");
/// ```
#[derive(Debug, Clone)]
pub struct HtmlTranslation<'a>(Translation<'a>);

impl fmt::Display for HtmlTranslation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write_with(f, cfg!(debug_assertions), true)
    }
}

#[cfg(feature = "html")]
impl askama::filters::HtmlSafe for HtmlTranslation<'_> {}

/// English `datetime.distance_in_words.<key>` for `count`, as [`helpers`](crate::helpers) writes it.
pub(crate) fn english_distance(key: &str, count: i64) -> String {
    let text = builtin::get("en", &format!("datetime.distance_in_words.{key}"), Some(count)).unwrap_or(key);
    let mut out = String::new();
    interpolate(&mut out, text, &[], Some(count), false).expect("writing to a String");
    out
}

/// A number [`Translation::count`] accepts: every integer type, and references to them.
///
/// References are accepted because askama passes template fields by
/// reference. Values outside the `i64` range count as `i64::MAX`.
///
/// # Examples
///
/// ```
/// use ocre::i18n::Count;
/// assert_eq!(3_usize.to_count(), 3);
/// assert_eq!((&-2_i32).to_count(), -2);
/// assert_eq!(u64::MAX.to_count(), i64::MAX);
/// ```
pub trait Count {
    /// Returns the number as `i64`, or `i64::MAX` when it does not fit.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::i18n::Count;
    /// assert_eq!(7_u8.to_count(), 7);
    /// ```
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

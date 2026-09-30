//! Localized dates, times, numbers and durations: Rails' `l`, `number_*` and
//! `time_ago_in_words` in the locale of an [`I18n`].

use std::fmt;

use super::{I18n, builtin, interpolate};
use crate::helpers::{self, DateNames, ENGLISH_NAMES};

impl I18n {
    /// Formats a date or time with a format of the locale, like Rails' `l(value, format: :short)`.
    ///
    /// `value` is what [`helpers`] take: Unix seconds or the
    /// text D1 stores (`2026-09-29`, `2026-09-29 14:05:00`...), in UTC.
    /// `format` is a name looked up in `date.formats.<name>` for a date
    /// without time (`2026-09-29`) and in `time.formats.<name>` otherwise
    /// (built-in names: `default`, `short`, `long`), or a pattern when it
    /// contains `%` (the directives of [`strftime`](crate::helpers::strftime)).
    /// Month and day names (`%B %b %A %a`) and `%p` come from
    /// `date.month_names`, `date.abbr_month_names`, `date.day_names`
    /// (Sunday first), `date.abbr_day_names` (comma-separated lists) and
    /// `time.am`/`time.pm`. Built in for English, French, German, Spanish,
    /// Italian, Portuguese and Dutch; locale files override any of them.
    /// A value that is not a time is returned unchanged; an unknown format
    /// name gives `translation missing: fr.date.formats.<name>`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| {
    ///     ocre::i18n::Catalog::load(&[("en", "en:\n"), ("fr", "fr:\n  date:\n    formats:\n      short: \"%-d %b\"\n")])
    /// });
    /// let fr = LOCALES.locale("fr");
    /// assert_eq!(fr.l("2026-09-29", "long"), "29 septembre 2026");
    /// assert_eq!(fr.l("2026-09-29", "short"), "29 sept.");
    /// assert_eq!(fr.l("2026-09-29 14:05:00", "%A %-d %B à %Hh%M"), "mardi 29 septembre à 14h05");
    /// assert_eq!(LOCALES.locale("en").l("2026-09-29", "long"), "September 29, 2026");
    /// ```
    pub fn l(&self, value: impl fmt::Display, format: &str) -> String {
        self.localize(value.to_string(), format)
    }

    fn localize(&self, text: String, format: &str) -> String {
        let Some(time) = helpers::parse_time(&text) else { return text };
        let pattern = if format.contains('%') {
            format
        } else {
            let trimmed = text.trim();
            let scope = if trimmed.len() == 10 && trimmed.as_bytes()[4] == b'-' { "date" } else { "time" };
            let key = format!("{scope}.formats.{format}");
            match self.lookup(&[&key], None, false) {
                Some(pattern) => pattern,
                None => return format!("translation missing: {}.{key}", self.locale()),
            }
        };
        helpers::format_time_with(time, pattern, &self.date_names())
    }

    fn date_names(&self) -> DateNames<'static> {
        DateNames {
            months: self.names("date.month_names").unwrap_or(ENGLISH_NAMES.months),
            abbr_months: self.names("date.abbr_month_names").unwrap_or(ENGLISH_NAMES.abbr_months),
            days: self.names("date.day_names").unwrap_or(ENGLISH_NAMES.days),
            abbr_days: self.names("date.abbr_day_names").unwrap_or(ENGLISH_NAMES.abbr_days),
            am: self.lookup(&["time.am"], None, false).unwrap_or(ENGLISH_NAMES.am),
            pm: self.lookup(&["time.pm"], None, false).unwrap_or(ENGLISH_NAMES.pm),
        }
    }

    /// The comma-separated list at `key` when it has exactly `N` names, else
    /// the built-in list of the language.
    fn names<const N: usize>(&self, key: &str) -> Option<[&'static str; N]> {
        let parse = |text: &'static str| {
            let mut parts = text.split(',');
            let mut names = [""; N];
            for name in &mut names {
                *name = parts.next()?.trim();
            }
            parts.next().is_none().then_some(names)
        };
        self.lookup(&[key], None, false)
            .and_then(parse)
            .or_else(|| builtin::get(self.locale(), key, None).and_then(parse))
    }

    /// Groups thousands and writes the decimal separator of the locale: `1 234 567,5` in French.
    ///
    /// Rails' `number_with_delimiter` with `number.format.delimiter` and
    /// `number.format.separator` (built in: `,` and `.` in English, a
    /// no-break space and `,` in French, `.` and `,` in German, Spanish,
    /// Italian, Portuguese and Dutch). Text that is not a number is returned
    /// unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n"), ("de", "de:\n")]));
    /// assert_eq!(LOCALES.locale("en").number(1234567.5), "1,234,567.5");
    /// assert_eq!(LOCALES.locale("de").number(1234567.5), "1.234.567,5");
    /// assert_eq!(LOCALES.locale("de").number("n/a"), "n/a");
    /// ```
    pub fn number(&self, value: impl fmt::Display) -> String {
        let (delimiter, separator) = self.number_format();
        helpers::delimit(&value.to_string(), delimiter, separator)
    }

    /// Rounds to `precision` decimals with the locale's decimal separator: `3,14` in French.
    ///
    /// Like Rails' `number_with_precision`: no thousands delimiter. Text
    /// that is not a number is returned unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| ocre::i18n::Catalog::load(&[("fr", "fr:\n")]));
    /// assert_eq!(LOCALES.locale("fr").number_with_precision(3.14159, 2), "3,14");
    /// ```
    pub fn number_with_precision(&self, value: impl fmt::Display, precision: usize) -> String {
        let text = value.to_string();
        match helpers::number(&text) {
            Some(n) => helpers::delimit(&format!("{n:.precision$}"), "", self.number_format().1),
            None => text,
        }
    }

    /// Formats an amount with two decimals and `unit` placed as the locale does: `1 234,50 €` in French.
    ///
    /// The layout is `number.currency.format.format` (`%u` the unit, `%n`
    /// the number; built in: `%u%n` in English, `%n %u` in French, German,
    /// Spanish and Italian, `%u %n` in Portuguese and Dutch, with a no-break
    /// space), and the number uses [`number`](Self::number)'s separators. A
    /// negative amount starts with `-`. Text that is not a number is returned
    /// unchanged. For cents, divide first: `i18n.currency(cents as f64 / 100.0, "€")`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales =
    ///     LazyLock::new(|| ocre::i18n::Catalog::load(&[("en", "en:\n"), ("fr", "fr:\n")]));
    /// assert_eq!(LOCALES.locale("en").currency(-1234.5, "$"), "-$1,234.50");
    /// assert_eq!(LOCALES.locale("fr").currency(1234.5, "€"), "1\u{a0}234,50\u{a0}€");
    /// ```
    pub fn currency(&self, value: impl fmt::Display, unit: &str) -> String {
        let text = value.to_string();
        let Some(n) = helpers::number(&text) else { return text };
        let (delimiter, separator) = self.number_format();
        let amount = helpers::delimit(&format!("{:.2}", n.abs()), delimiter, separator);
        let sign = if n < 0.0 && (n * 100.0).round() != 0.0 { "-" } else { "" };
        let layout = self.lookup(&["number.currency.format.format"], None, false).unwrap_or("%u%n");
        format!("{sign}{}", layout.replace("%u", unit).replace("%n", &amount))
    }

    /// `(delimiter, separator)` of the locale.
    fn number_format(&self) -> (&'static str, &'static str) {
        (
            self.lookup(&["number.format.delimiter"], None, false).unwrap_or(","),
            self.lookup(&["number.format.separator"], None, false).unwrap_or("."),
        )
    }

    /// The time from `value` to [`now`](crate::now) in words, in the locale: `environ 3 heures`.
    ///
    /// Rails' `time_ago_in_words` with the `datetime.distance_in_words.*`
    /// keys (`less_than_x_minutes`, `x_minutes`, `about_x_hours`, `x_days`,
    /// `about_x_months`, `x_months`, `about_x_years`, `over_x_years`,
    /// `almost_x_years`, each with plural forms and `%{count}`); the ranges
    /// are those of [`helpers::time_ago_in_words`].
    /// `value` is Unix seconds or D1 text; anything else is returned
    /// unchanged. Add "ago" with a translation: `t("posts.ago").arg("time", ..)`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| ocre::i18n::Catalog::load(&[("fr", "fr:\n")]));
    /// assert_eq!(LOCALES.locale("fr").time_ago_in_words(ocre::now() - 3 * 3600), "environ 3 heures");
    /// ```
    pub fn time_ago_in_words(&self, value: impl fmt::Display) -> String {
        self.distance_text(value.to_string(), &crate::now().to_string())
    }

    /// The time between two times in words, in the locale (Rails' `distance_of_time_in_words`).
    ///
    /// The order does not matter; see [`time_ago_in_words`](Self::time_ago_in_words).
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::LazyLock;
    /// static LOCALES: ocre::i18n::Locales = LazyLock::new(|| ocre::i18n::Catalog::load(&[("de", "de:\n")]));
    /// let de = LOCALES.locale("de");
    /// assert_eq!(de.distance_of_time_in_words("2026-01-01 10:00:00", "2026-01-03 09:00:00"), "2 Tage");
    /// ```
    pub fn distance_of_time_in_words(&self, from: impl fmt::Display, to: impl fmt::Display) -> String {
        self.distance_text(from.to_string(), &to.to_string())
    }

    fn distance_text(&self, text: String, to: &str) -> String {
        let (Some(from), Some(to)) = (helpers::parse_time(&text), helpers::parse_time(to)) else { return text };
        let (key, count) = helpers::distance_key(from, to);
        let key = format!("datetime.distance_in_words.{key}");
        let template = self.lookup(&[&key], Some(count), false).unwrap_or_default();
        let mut out = String::new();
        interpolate(&mut out, template, &[], Some(count), false).expect("writing to a String");
        out
    }
}

#[cfg(test)]
#[path = "../../tests/i18n/format.rs"]
mod tests;

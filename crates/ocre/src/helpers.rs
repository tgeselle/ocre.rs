//! View helpers: numbers, dates and text formatted like Rails' `number_to_currency`, `time_ago_in_words`, `excerpt`...
//!
//! Plain functions returning `String`, usable anywhere (a JSON field, an
//! email, a log line). Templates use them as askama filters through
//! [`filters`](crate::filters) (`{{ post.price|number_to_currency("$") }}`,
//! feature `html`).
//!
//! Number helpers take any value whose text is a number (`i64`, `f64`, a
//! `String` from D1...) and return any other text unchanged. Time helpers
//! take Unix seconds (`i64`, as [`now`](crate::now) returns) or the text D1
//! stores (`2026-09-29 14:05:00`, `2026-09-29T14:05:00Z`, `2026-09-29`,
//! with an optional `+02:00` offset), read as UTC when there is no offset,
//! and return other text unchanged. All are plain string work: microseconds
//! of CPU, no binding call. English only; [`I18n`](crate::i18n::I18n) has
//! the localized versions (`l`, `number`, `currency`, `time_ago_in_words`...).

use std::fmt::{Display, Write as _};

/// Groups the integer part by thousands: `1234567.891` becomes `1,234,567.891` (Rails' `number_with_delimiter`).
///
/// # Examples
///
/// ```
/// use ocre::helpers::number_with_delimiter;
///
/// assert_eq!(number_with_delimiter(-1234567), "-1,234,567");
/// assert_eq!(number_with_delimiter("1234.5"), "1,234.5");
/// assert_eq!(number_with_delimiter("n/a"), "n/a");
/// ```
pub fn number_with_delimiter(value: impl Display) -> String {
    number_with_delimiter_text(value.to_string())
}

fn number_with_delimiter_text(text: String) -> String {
    with_delimiter(&text)
}

/// Rounds to `precision` decimals: `3.14159` with 2 becomes `3.14` (Rails' `number_with_precision`).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::helpers::number_with_precision(2.46, 1), "2.5");
/// ```
pub fn number_with_precision(value: impl Display, precision: usize) -> String {
    number_with_precision_text(value.to_string(), precision)
}

fn number_with_precision_text(text: String, precision: usize) -> String {
    number(&text).map_or(text, |n| format!("{n:.precision$}"))
}

/// Two decimals, thousands grouped, `unit` first: `1234.5` with `"$"` becomes `$1,234.50`, `-3` becomes `-$3.00` (Rails' `number_to_currency`).
///
/// For amounts stored in cents, divide first: `number_to_currency(cents as f64 / 100.0, "€")`.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::helpers::number_to_currency(1234.5, "€"), "€1,234.50");
/// ```
pub fn number_to_currency(value: impl Display, unit: &str) -> String {
    number_to_currency_text(value.to_string(), unit)
}

fn number_to_currency_text(text: String, unit: &str) -> String {
    match number(&text) {
        Some(n) => {
            let sign = if n < 0.0 && (n * 100.0).round() != 0.0 { "-" } else { "" };
            format!("{sign}{unit}{}", with_delimiter(&format!("{:.2}", n.abs())))
        }
        None => text,
    }
}

/// Rounds to `precision` decimals and adds `%`: `12.345` with 1 becomes `12.3%` (Rails' `number_to_percentage`).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::helpers::number_to_percentage(99.6, 0), "100%");
/// ```
pub fn number_to_percentage(value: impl Display, precision: usize) -> String {
    number_to_percentage_text(value.to_string(), precision)
}

fn number_to_percentage_text(text: String, precision: usize) -> String {
    number(&text).map_or(text, |n| format!("{n:.precision$}%"))
}

/// A byte count in 1024 steps: `1536` becomes `1.5 KB` (Rails' `number_to_human_size`, as [`storage::human_size`](crate::storage::human_size)).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::helpers::number_to_human_size(3 * 1024 * 1024), "3 MB");
/// ```
pub fn number_to_human_size(value: impl Display) -> String {
    number_to_human_size_text(value.to_string())
}

fn number_to_human_size_text(text: String) -> String {
    match number(&text) {
        Some(n) if n >= 0.0 => crate::storage::human_size(n as u64),
        _ => text,
    }
}

/// Three significant digits and a word, Thousand to Quadrillion: `1234567` becomes `1.23 Million` (Rails' `number_to_human`).
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::helpers::number_to_human(489_939), "490 Thousand");
/// assert_eq!(ocre::helpers::number_to_human(123), "123");
/// ```
pub fn number_to_human(value: impl Display) -> String {
    number_to_human_text(value.to_string())
}

fn number_to_human_text(text: String) -> String {
    number(&text).map_or(text, human)
}

/// The time from `value` to [`now`](crate::now), in words: `about 3 hours` (Rails' `time_ago_in_words`).
///
/// Add "ago" yourself: `{{ post.created_at|time_ago_in_words }} ago`. The
/// wording is Rails': `less than a minute`, `1 minute`, `N minutes`,
/// `about 1 hour`, `about N hours`, `1 day`, `N days`, `about 1 month`,
/// `about 2 months`, `N months`, then `about N years`, `over N years` and
/// `almost N years`.
///
/// # Examples
///
/// ```
/// let three_hours_ago = ocre::now() - 3 * 3600;
/// assert_eq!(ocre::helpers::time_ago_in_words(three_hours_ago), "about 3 hours");
/// ```
pub fn time_ago_in_words(value: impl Display) -> String {
    time_ago_in_words_text(value.to_string())
}

fn time_ago_in_words_text(text: String) -> String {
    parse_time(&text).map_or(text, |time| distance_in_words(time, crate::now()))
}

/// The time between two times, in words: `5 minutes`, `2 days` (Rails' `distance_of_time_in_words`).
///
/// The order does not matter. See [`time_ago_in_words`] for the wording.
///
/// # Examples
///
/// ```
/// use ocre::helpers::distance_of_time_in_words;
///
/// assert_eq!(distance_of_time_in_words("2026-01-01 10:00:00", "2026-01-03 09:00:00"), "2 days");
/// ```
pub fn distance_of_time_in_words(from: impl Display, to: impl Display) -> String {
    distance_text(from.to_string(), &to.to_string())
}

fn distance_text(text: String, to: &str) -> String {
    match (parse_time(&text), parse_time(to)) {
        (Some(from), Some(to)) => distance_in_words(from, to),
        _ => text,
    }
}

/// Formats a time in UTC with `strftime` directives: `%b %-d, %Y` gives `Sep 29, 2026`.
///
/// Directives: `%Y` (2026), `%y` (26), `%m` (09), `%d` (29), `%e` (day
/// padded with a space), `%H` (14), `%I` (02), `%M`, `%S`, `%p` (AM/PM),
/// `%b` (Sep), `%B` (September), `%a` (Tue), `%A` (Tuesday), `%j` (day of
/// the year), `%F` (2026-09-29), `%T` (14:05:00), `%z` (`+0000`: always
/// UTC), `%%`; `%-d`, `%-m`, `%-H`, `%-I` drop the zero padding. Unknown
/// directives are copied as they are. For month and day names in another
/// language, see [`I18n::l`](crate::i18n::I18n::l).
///
/// # Examples
///
/// ```
/// use ocre::helpers::strftime;
///
/// assert_eq!(strftime("2026-09-29 14:05:00", "%A %-d %B %Y, %I:%M %p"), "Tuesday 29 September 2026, 02:05 PM");
/// ```
pub fn strftime(value: impl Display, format: &str) -> String {
    strftime_text(value.to_string(), format)
}

fn strftime_text(text: String, format: &str) -> String {
    parse_time(&text).map_or(text, |time| format_time(time, format))
}

/// The first match of `phrase` (case-insensitive) with `radius` characters around it, `...` where cut (Rails' `excerpt`).
///
/// Empty when the phrase is not found.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::helpers::excerpt("This is an example", "an", 5), "...s is an exam...");
/// ```
pub fn excerpt(value: impl Display, phrase: &str, radius: usize) -> String {
    excerpt_text(value.to_string(), phrase, radius)
}

fn excerpt_text(text: String, phrase: &str, radius: usize) -> String {
    let text: Vec<char> = text.chars().collect();
    let Some(start) = find_ignore_case(&text, phrase, 0) else {
        return String::new();
    };
    let end = start + phrase.chars().count();
    let (from, to) = (start.saturating_sub(radius), (end + radius).min(text.len()));
    let mut out = String::new();
    if from > 0 {
        out.push_str("...");
    }
    out.extend(&text[from..to]);
    if to < text.len() {
        out.push_str("...");
    }
    out
}

/// The text, HTML-escaped, with each match of `phrase` (case-insensitive) in `<mark>` (Rails' `highlight`).
///
/// The result is HTML: the `highlight` filter marks it safe.
///
/// # Examples
///
/// ```
/// assert_eq!(
///     ocre::helpers::highlight("Rust & rusty <tools>", "rust"),
///     "<mark>Rust</mark> &amp; <mark>rust</mark>y &lt;tools&gt;"
/// );
/// ```
pub fn highlight(value: impl Display, phrase: &str) -> String {
    highlight_text(value.to_string(), phrase)
}

fn highlight_text(text: String, phrase: &str) -> String {
    let text: Vec<char> = text.chars().collect();
    let length = phrase.chars().count();
    let mut out = String::with_capacity(text.len() + 16);
    let mut at = 0;
    while let Some(start) = (length > 0).then(|| find_ignore_case(&text, phrase, at)).flatten() {
        escape_into(&mut out, &text[at..start]);
        out.push_str("<mark>");
        escape_into(&mut out, &text[start..start + length]);
        out.push_str("</mark>");
        at = start + length;
    }
    escape_into(&mut out, &text[at..]);
    out
}

/// Breaks lines longer than `width` characters at spaces (Rails' `word_wrap`).
///
/// Words longer than `width` stay whole; existing line breaks are kept.
/// In HTML, show the breaks with `|linebreaksbr` or in a `<pre>`.
///
/// # Examples
///
/// ```
/// assert_eq!(ocre::helpers::word_wrap("Once upon a time", 8), "Once\nupon a\ntime");
/// ```
pub fn word_wrap(value: impl Display, width: usize) -> String {
    word_wrap_text(value.to_string(), width)
}

fn word_wrap_text(text: String, width: usize) -> String {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// Space-separated class names whose condition is true (Rails' `class_names` / `token_list`).
///
/// In a template (askama passes the array by reference):
/// `<a class="{{ ocre::helpers::class_names([("tab", true), ("active", is_current)]) }}">`.
///
/// # Examples
///
/// ```
/// let classes = ocre::helpers::class_names(&[("btn", true), ("btn-danger", false), ("wide", true)]);
/// assert_eq!(classes, "btn wide");
/// ```
pub fn class_names(classes: &[(&str, bool)]) -> String {
    let mut out = String::new();
    for &(name, on) in classes {
        if on && !name.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(name);
        }
    }
    out
}

/// Whether `url` names the page being shown (Rails' `current_page?`), for "you are here" links.
///
/// `current` is the request's URI as text (`uri.to_string()` of the `Uri`
/// or `OriginalUri` extractor, or just its path and query); `url` is a link
/// target: a path, a path with a query, or an absolute URL. Paths are
/// compared without a trailing slash (`/posts/` is `/posts`) and without a
/// `#fragment`. The query only matters when `url` has one, and the host
/// only when both are absolute. Pure string work, no binding call.
///
/// # Examples
///
/// ```
/// use ocre::helpers::current_page;
///
/// let current = "https://shop.example.com/posts?page=2";
/// assert!(current_page(current, "/posts"));
/// assert!(current_page(current, "/posts/?page=2"));
/// assert!(!current_page(current, "/posts?page=3"));
/// assert!(current_page(current, "https://shop.example.com/posts#top"));
/// assert!(!current_page(current, "https://other.example.com/posts"));
/// ```
///
/// In a template, with `current: String` set from the `Uri` extractor:
/// `<a href="/posts" class="{{ ocre::helpers::class_names([("active", ocre::helpers::current_page(current, "/posts"))]) }}">`.
pub fn current_page(current: &str, url: &str) -> bool {
    let (current_host, current_path, current_query) = split_url(current);
    let (host, path, query) = split_url(url);
    let same_host = match (current_host, host) {
        (Some(current_host), Some(host)) => current_host.eq_ignore_ascii_case(host),
        _ => true,
    };
    same_host && current_path == path && query.is_none_or(|query| current_query == Some(query))
}

/// `https://host/path/?q#f` is `(Some("host"), "/path", Some("q"))`.
fn split_url(url: &str) -> (Option<&str>, &str, Option<&str>) {
    let url = url.split('#').next().unwrap_or_default();
    let (host, rest) = match url.split_once("://") {
        Some((_, after)) => {
            let end = after.find(['/', '?']).unwrap_or(after.len());
            (Some(&after[..end]), &after[end..])
        }
        None => (None, url),
    };
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query).filter(|query| !query.is_empty())),
        None => (rest, None),
    };
    let path = path.trim_end_matches('/');
    (host, if path.is_empty() { "/" } else { path }, query)
}

pub(crate) fn number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

/// `-1234567.5` -> `-1,234,567.5`; anything else unchanged.
fn with_delimiter(text: &str) -> String {
    delimit(text, ",", ".")
}

/// `-1234567.5` with `delimiter` between thousands and `separator` before the decimals; non-numbers unchanged.
pub(crate) fn delimit(text: &str, delimiter: &str, separator: &str) -> String {
    if number(text).is_none() {
        return text.to_owned();
    }
    let text = text.trim();
    let (sign, rest) = text.strip_prefix('-').map_or(("", text), |rest| ("-", rest));
    let (int, frac) = rest.split_once('.').map_or((rest, None), |(int, frac)| (int, Some(frac)));
    if !int.bytes().all(|b| b.is_ascii_digit()) {
        return text.to_owned();
    }
    let mut out = String::from(sign);
    for (i, digit) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push_str(delimiter);
        }
        out.push(digit);
    }
    if let Some(frac) = frac {
        out.push_str(separator);
        out.push_str(frac);
    }
    out
}

fn human(n: f64) -> String {
    const UNITS: [&str; 5] = ["Thousand", "Million", "Billion", "Trillion", "Quadrillion"];
    let mut exponent = 0;
    let mut value = n.abs();
    while value >= 999.5 && exponent < UNITS.len() {
        value /= 1000.0;
        exponent += 1;
    }
    let digits = if value >= 99.95 {
        0
    } else if value >= 9.995 {
        1
    } else {
        2
    };
    let rounded = format!("{value:.digits$}");
    let rounded = if rounded.contains('.') { rounded.trim_end_matches('0').trim_end_matches('.') } else { &rounded };
    let sign = if n < 0.0 { "-" } else { "" };
    match exponent {
        0 => format!("{sign}{rounded}"),
        _ => format!("{sign}{rounded} {}", UNITS[exponent - 1]),
    }
}

/// Unix seconds from `1727618700`, `2026-09-29`, `2026-09-29 14:05[:00[.123]]`
/// (or with `T`), followed by nothing, `Z` or `±hh:mm`.
pub(crate) fn parse_time(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Ok(seconds) = text.parse::<i64>() {
        return Some(seconds);
    }
    let number = |part: &str| -> Option<i64> {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse().ok()).flatten()
    };
    let (date, rest) = text.split_at(text.len().min(10));
    let mut ymd = date.splitn(3, '-');
    let (year, month, day) = (number(ymd.next()?)?, number(ymd.next()?)?, number(ymd.next()?)?);
    if date.len() != 10 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut seconds = days_from_civil(year, month, day) * 86_400;
    let rest = rest.strip_prefix(['T', ' ']).unwrap_or(rest);
    let zone_at = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(zone_at);
    if !clock.is_empty() {
        let clock = clock.split('.').next().unwrap_or(clock);
        let mut hms = clock.split(':');
        let (hour, minute) = (number(hms.next()?)?, number(hms.next()?)?);
        let second = hms.next().map_or(Some(0), number)?;
        if hms.next().is_some() || hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        seconds += hour * 3600 + minute * 60 + second;
    }
    match zone {
        "" | "Z" => Some(seconds),
        _ => {
            let (sign, offset) = zone.split_at(1);
            let (hours, minutes) = offset.split_once(':').unwrap_or((offset, "0"));
            let offset = number(hours)? * 3600 + number(minutes)? * 60;
            Some(if sign == "+" { seconds - offset } else { seconds + offset })
        }
    }
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// (year, month, day) of a day count since 1970-01-01.
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Month and day names [`format_time_with`] writes for `%B %b %A %a %p`.
pub(crate) struct DateNames<'a> {
    pub(crate) months: [&'a str; 12],
    pub(crate) abbr_months: [&'a str; 12],
    /// Sunday first, as Rails' `date.day_names`.
    pub(crate) days: [&'a str; 7],
    pub(crate) abbr_days: [&'a str; 7],
    pub(crate) am: &'a str,
    pub(crate) pm: &'a str,
}

pub(crate) const ENGLISH_NAMES: DateNames<'static> = DateNames {
    months: [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
    abbr_months: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    days: ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"],
    abbr_days: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    am: "AM",
    pm: "PM",
};

fn format_time(time: i64, format: &str) -> String {
    format_time_with(time, format, &ENGLISH_NAMES)
}

/// [`strftime`] with the names of a language.
pub(crate) fn format_time_with(time: i64, format: &str, names: &DateNames<'_>) -> String {
    let days = time.div_euclid(86_400);
    let of_day = time.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (of_day / 3600, of_day % 3600 / 60, of_day % 60);
    let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    let month_index = (month - 1) as usize;
    // 1970-01-01 was a Thursday.
    let weekday = (days + 4).rem_euclid(7) as usize;
    let mut out = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let (unpadded, directive) = match chars.next() {
            Some('-') => (true, chars.next()),
            other => (false, other),
        };
        let padded = |value: i64| if unpadded { value.to_string() } else { format!("{value:02}") };
        let written = match directive {
            Some('Y') => write!(out, "{year}"),
            Some('y') => write!(out, "{:02}", year.rem_euclid(100)),
            Some('m') => write!(out, "{}", padded(month)),
            Some('d') => write!(out, "{}", padded(day)),
            Some('e') => write!(out, "{day:>2}"),
            Some('H') => write!(out, "{}", padded(hour)),
            Some('I') => write!(out, "{}", padded(hour12)),
            Some('M') => write!(out, "{minute:02}"),
            Some('S') => write!(out, "{second:02}"),
            Some('p') => write!(out, "{}", if hour < 12 { names.am } else { names.pm }),
            Some('b') => write!(out, "{}", names.abbr_months[month_index]),
            Some('B') => write!(out, "{}", names.months[month_index]),
            Some('a') => write!(out, "{}", names.abbr_days[weekday]),
            Some('A') => write!(out, "{}", names.days[weekday]),
            Some('j') => write!(out, "{:03}", days - days_from_civil(year, 1, 1) + 1),
            Some('F') => write!(out, "{year}-{month:02}-{day:02}"),
            Some('T') => write!(out, "{hour:02}:{minute:02}:{second:02}"),
            Some('z') => write!(out, "+0000"),
            Some('%') => write!(out, "%"),
            Some(other) => write!(out, "%{}{other}", if unpadded { "-" } else { "" }),
            None => write!(out, "%"),
        };
        written.expect("writing to a String");
    }
    out
}

/// Rails' `distance_of_time_in_words`, without seconds precision, in English.
fn distance_in_words(from: i64, to: i64) -> String {
    let (key, count) = distance_key(from, to);
    crate::i18n::english_distance(key, count)
}

/// The `datetime.distance_in_words.<key>` Rails picks for the time between
/// `from` and `to`, with its `count`.
pub(crate) fn distance_key(from: i64, to: i64) -> (&'static str, i64) {
    const HOUR: i64 = 60;
    const DAY: i64 = 1440;
    const MONTH: i64 = 43_200;
    const YEAR: i64 = 525_600;
    let minutes = ((from - to).abs() + 30) / 60;
    let rounded = |unit: i64| (minutes + unit / 2) / unit;
    match minutes {
        0 => ("less_than_x_minutes", 1),
        1..45 => ("x_minutes", minutes),
        45..90 => ("about_x_hours", 1),
        90..DAY => ("about_x_hours", rounded(HOUR)),
        DAY..2520 => ("x_days", 1),
        2520..MONTH => ("x_days", rounded(DAY)),
        MONTH..86_400 => ("about_x_months", rounded(MONTH)),
        86_400..YEAR => ("x_months", rounded(MONTH)),
        _ => {
            let (years, remainder) = (minutes / YEAR, minutes % YEAR);
            if remainder < YEAR / 4 {
                ("about_x_years", years)
            } else if remainder < YEAR * 3 / 4 {
                ("over_x_years", years)
            } else {
                ("almost_x_years", years + 1)
            }
        }
    }
}

/// Index (in chars) of the first case-insensitive match of `phrase` at or after `from`.
fn find_ignore_case(text: &[char], phrase: &str, from: usize) -> Option<usize> {
    let phrase: Vec<char> = phrase.chars().flat_map(char::to_lowercase).collect();
    if phrase.is_empty() || phrase.len() > text.len() {
        return None;
    }
    (from..=text.len() - phrase.len()).find(|&start| {
        text[start..start + phrase.len()].iter().flat_map(|c| c.to_lowercase()).eq(phrase.iter().copied())
    })
}

fn escape_into(out: &mut String, text: &[char]) {
    for c in text {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(*c),
        }
    }
}

/// IANA time zone names, as browsers list them (`Intl.supportedValuesOf("timeZone")`): what
/// [`time_zone_options`] offers and `ocre time-zones` prints.
///
/// # Examples
///
/// ```
/// assert!(ocre::helpers::TIME_ZONES.contains(&"Europe/Paris"));
/// ```
pub const TIME_ZONES: &[&str] = &[
    "UTC",
    "Africa/Abidjan",
    "Africa/Accra",
    "Africa/Addis_Ababa",
    "Africa/Algiers",
    "Africa/Asmera",
    "Africa/Bamako",
    "Africa/Bangui",
    "Africa/Banjul",
    "Africa/Bissau",
    "Africa/Blantyre",
    "Africa/Brazzaville",
    "Africa/Bujumbura",
    "Africa/Cairo",
    "Africa/Casablanca",
    "Africa/Ceuta",
    "Africa/Conakry",
    "Africa/Dakar",
    "Africa/Dar_es_Salaam",
    "Africa/Djibouti",
    "Africa/Douala",
    "Africa/El_Aaiun",
    "Africa/Freetown",
    "Africa/Gaborone",
    "Africa/Harare",
    "Africa/Johannesburg",
    "Africa/Juba",
    "Africa/Kampala",
    "Africa/Khartoum",
    "Africa/Kigali",
    "Africa/Kinshasa",
    "Africa/Lagos",
    "Africa/Libreville",
    "Africa/Lome",
    "Africa/Luanda",
    "Africa/Lubumbashi",
    "Africa/Lusaka",
    "Africa/Malabo",
    "Africa/Maputo",
    "Africa/Maseru",
    "Africa/Mbabane",
    "Africa/Mogadishu",
    "Africa/Monrovia",
    "Africa/Nairobi",
    "Africa/Ndjamena",
    "Africa/Niamey",
    "Africa/Nouakchott",
    "Africa/Ouagadougou",
    "Africa/Porto-Novo",
    "Africa/Sao_Tome",
    "Africa/Tripoli",
    "Africa/Tunis",
    "Africa/Windhoek",
    "America/Adak",
    "America/Anchorage",
    "America/Anguilla",
    "America/Antigua",
    "America/Araguaina",
    "America/Argentina/La_Rioja",
    "America/Argentina/Rio_Gallegos",
    "America/Argentina/Salta",
    "America/Argentina/San_Juan",
    "America/Argentina/San_Luis",
    "America/Argentina/Tucuman",
    "America/Argentina/Ushuaia",
    "America/Aruba",
    "America/Asuncion",
    "America/Bahia",
    "America/Bahia_Banderas",
    "America/Barbados",
    "America/Belem",
    "America/Belize",
    "America/Blanc-Sablon",
    "America/Boa_Vista",
    "America/Bogota",
    "America/Boise",
    "America/Buenos_Aires",
    "America/Cambridge_Bay",
    "America/Campo_Grande",
    "America/Cancun",
    "America/Caracas",
    "America/Catamarca",
    "America/Cayenne",
    "America/Cayman",
    "America/Chicago",
    "America/Chihuahua",
    "America/Ciudad_Juarez",
    "America/Coral_Harbour",
    "America/Cordoba",
    "America/Costa_Rica",
    "America/Coyhaique",
    "America/Creston",
    "America/Cuiaba",
    "America/Curacao",
    "America/Danmarkshavn",
    "America/Dawson",
    "America/Dawson_Creek",
    "America/Denver",
    "America/Detroit",
    "America/Dominica",
    "America/Edmonton",
    "America/Eirunepe",
    "America/El_Salvador",
    "America/Fort_Nelson",
    "America/Fortaleza",
    "America/Glace_Bay",
    "America/Godthab",
    "America/Goose_Bay",
    "America/Grand_Turk",
    "America/Grenada",
    "America/Guadeloupe",
    "America/Guatemala",
    "America/Guayaquil",
    "America/Guyana",
    "America/Halifax",
    "America/Havana",
    "America/Hermosillo",
    "America/Indiana/Knox",
    "America/Indiana/Marengo",
    "America/Indiana/Petersburg",
    "America/Indiana/Tell_City",
    "America/Indiana/Vevay",
    "America/Indiana/Vincennes",
    "America/Indiana/Winamac",
    "America/Indianapolis",
    "America/Inuvik",
    "America/Iqaluit",
    "America/Jamaica",
    "America/Jujuy",
    "America/Juneau",
    "America/Kentucky/Monticello",
    "America/Kralendijk",
    "America/La_Paz",
    "America/Lima",
    "America/Los_Angeles",
    "America/Louisville",
    "America/Lower_Princes",
    "America/Maceio",
    "America/Managua",
    "America/Manaus",
    "America/Marigot",
    "America/Martinique",
    "America/Matamoros",
    "America/Mazatlan",
    "America/Mendoza",
    "America/Menominee",
    "America/Merida",
    "America/Metlakatla",
    "America/Mexico_City",
    "America/Miquelon",
    "America/Moncton",
    "America/Monterrey",
    "America/Montevideo",
    "America/Montserrat",
    "America/Nassau",
    "America/New_York",
    "America/Nome",
    "America/Noronha",
    "America/North_Dakota/Beulah",
    "America/North_Dakota/Center",
    "America/North_Dakota/New_Salem",
    "America/Ojinaga",
    "America/Panama",
    "America/Paramaribo",
    "America/Phoenix",
    "America/Port-au-Prince",
    "America/Port_of_Spain",
    "America/Porto_Velho",
    "America/Puerto_Rico",
    "America/Punta_Arenas",
    "America/Rankin_Inlet",
    "America/Recife",
    "America/Regina",
    "America/Resolute",
    "America/Rio_Branco",
    "America/Santarem",
    "America/Santiago",
    "America/Santo_Domingo",
    "America/Sao_Paulo",
    "America/Scoresbysund",
    "America/Sitka",
    "America/St_Barthelemy",
    "America/St_Johns",
    "America/St_Kitts",
    "America/St_Lucia",
    "America/St_Thomas",
    "America/St_Vincent",
    "America/Swift_Current",
    "America/Tegucigalpa",
    "America/Thule",
    "America/Tijuana",
    "America/Toronto",
    "America/Tortola",
    "America/Vancouver",
    "America/Whitehorse",
    "America/Winnipeg",
    "America/Yakutat",
    "Antarctica/Casey",
    "Antarctica/Davis",
    "Antarctica/DumontDUrville",
    "Antarctica/Macquarie",
    "Antarctica/Mawson",
    "Antarctica/McMurdo",
    "Antarctica/Palmer",
    "Antarctica/Rothera",
    "Antarctica/Syowa",
    "Antarctica/Troll",
    "Antarctica/Vostok",
    "Arctic/Longyearbyen",
    "Asia/Aden",
    "Asia/Almaty",
    "Asia/Amman",
    "Asia/Anadyr",
    "Asia/Aqtau",
    "Asia/Aqtobe",
    "Asia/Ashgabat",
    "Asia/Atyrau",
    "Asia/Baghdad",
    "Asia/Bahrain",
    "Asia/Baku",
    "Asia/Bangkok",
    "Asia/Barnaul",
    "Asia/Beirut",
    "Asia/Bishkek",
    "Asia/Brunei",
    "Asia/Calcutta",
    "Asia/Chita",
    "Asia/Colombo",
    "Asia/Damascus",
    "Asia/Dhaka",
    "Asia/Dili",
    "Asia/Dubai",
    "Asia/Dushanbe",
    "Asia/Famagusta",
    "Asia/Gaza",
    "Asia/Hebron",
    "Asia/Hong_Kong",
    "Asia/Hovd",
    "Asia/Irkutsk",
    "Asia/Jakarta",
    "Asia/Jayapura",
    "Asia/Jerusalem",
    "Asia/Kabul",
    "Asia/Kamchatka",
    "Asia/Karachi",
    "Asia/Katmandu",
    "Asia/Khandyga",
    "Asia/Krasnoyarsk",
    "Asia/Kuala_Lumpur",
    "Asia/Kuching",
    "Asia/Kuwait",
    "Asia/Macau",
    "Asia/Magadan",
    "Asia/Makassar",
    "Asia/Manila",
    "Asia/Muscat",
    "Asia/Nicosia",
    "Asia/Novokuznetsk",
    "Asia/Novosibirsk",
    "Asia/Omsk",
    "Asia/Oral",
    "Asia/Phnom_Penh",
    "Asia/Pontianak",
    "Asia/Pyongyang",
    "Asia/Qatar",
    "Asia/Qostanay",
    "Asia/Qyzylorda",
    "Asia/Rangoon",
    "Asia/Riyadh",
    "Asia/Saigon",
    "Asia/Sakhalin",
    "Asia/Samarkand",
    "Asia/Seoul",
    "Asia/Shanghai",
    "Asia/Singapore",
    "Asia/Srednekolymsk",
    "Asia/Taipei",
    "Asia/Tashkent",
    "Asia/Tbilisi",
    "Asia/Tehran",
    "Asia/Thimphu",
    "Asia/Tokyo",
    "Asia/Tomsk",
    "Asia/Ulaanbaatar",
    "Asia/Urumqi",
    "Asia/Ust-Nera",
    "Asia/Vientiane",
    "Asia/Vladivostok",
    "Asia/Yakutsk",
    "Asia/Yekaterinburg",
    "Asia/Yerevan",
    "Atlantic/Azores",
    "Atlantic/Bermuda",
    "Atlantic/Canary",
    "Atlantic/Cape_Verde",
    "Atlantic/Faeroe",
    "Atlantic/Madeira",
    "Atlantic/Reykjavik",
    "Atlantic/South_Georgia",
    "Atlantic/St_Helena",
    "Atlantic/Stanley",
    "Australia/Adelaide",
    "Australia/Brisbane",
    "Australia/Broken_Hill",
    "Australia/Darwin",
    "Australia/Eucla",
    "Australia/Hobart",
    "Australia/Lindeman",
    "Australia/Lord_Howe",
    "Australia/Melbourne",
    "Australia/Perth",
    "Australia/Sydney",
    "Europe/Amsterdam",
    "Europe/Andorra",
    "Europe/Astrakhan",
    "Europe/Athens",
    "Europe/Belgrade",
    "Europe/Berlin",
    "Europe/Bratislava",
    "Europe/Brussels",
    "Europe/Bucharest",
    "Europe/Budapest",
    "Europe/Busingen",
    "Europe/Chisinau",
    "Europe/Copenhagen",
    "Europe/Dublin",
    "Europe/Gibraltar",
    "Europe/Guernsey",
    "Europe/Helsinki",
    "Europe/Isle_of_Man",
    "Europe/Istanbul",
    "Europe/Jersey",
    "Europe/Kaliningrad",
    "Europe/Kiev",
    "Europe/Kirov",
    "Europe/Lisbon",
    "Europe/Ljubljana",
    "Europe/London",
    "Europe/Luxembourg",
    "Europe/Madrid",
    "Europe/Malta",
    "Europe/Mariehamn",
    "Europe/Minsk",
    "Europe/Monaco",
    "Europe/Moscow",
    "Europe/Oslo",
    "Europe/Paris",
    "Europe/Podgorica",
    "Europe/Prague",
    "Europe/Riga",
    "Europe/Rome",
    "Europe/Samara",
    "Europe/San_Marino",
    "Europe/Sarajevo",
    "Europe/Saratov",
    "Europe/Simferopol",
    "Europe/Skopje",
    "Europe/Sofia",
    "Europe/Stockholm",
    "Europe/Tallinn",
    "Europe/Tirane",
    "Europe/Ulyanovsk",
    "Europe/Vaduz",
    "Europe/Vatican",
    "Europe/Vienna",
    "Europe/Vilnius",
    "Europe/Volgograd",
    "Europe/Warsaw",
    "Europe/Zagreb",
    "Europe/Zurich",
    "Indian/Antananarivo",
    "Indian/Chagos",
    "Indian/Christmas",
    "Indian/Cocos",
    "Indian/Comoro",
    "Indian/Kerguelen",
    "Indian/Mahe",
    "Indian/Maldives",
    "Indian/Mauritius",
    "Indian/Mayotte",
    "Indian/Reunion",
    "Pacific/Apia",
    "Pacific/Auckland",
    "Pacific/Bougainville",
    "Pacific/Chatham",
    "Pacific/Easter",
    "Pacific/Efate",
    "Pacific/Enderbury",
    "Pacific/Fakaofo",
    "Pacific/Fiji",
    "Pacific/Funafuti",
    "Pacific/Galapagos",
    "Pacific/Gambier",
    "Pacific/Guadalcanal",
    "Pacific/Guam",
    "Pacific/Honolulu",
    "Pacific/Kiritimati",
    "Pacific/Kosrae",
    "Pacific/Kwajalein",
    "Pacific/Majuro",
    "Pacific/Marquesas",
    "Pacific/Midway",
    "Pacific/Nauru",
    "Pacific/Niue",
    "Pacific/Norfolk",
    "Pacific/Noumea",
    "Pacific/Pago_Pago",
    "Pacific/Palau",
    "Pacific/Pitcairn",
    "Pacific/Ponape",
    "Pacific/Port_Moresby",
    "Pacific/Rarotonga",
    "Pacific/Saipan",
    "Pacific/Tahiti",
    "Pacific/Tarawa",
    "Pacific/Tongatapu",
    "Pacific/Truk",
    "Pacific/Wake",
    "Pacific/Wallis",
];

/// The `<option>`s of every [`TIME_ZONES`] name, `selected` marked (Rails'
/// `time_zone_select`): write them in a `<select>`, and store the IANA
/// name the form sends. Ocre's own time helpers stay UTC; convert for
/// display in the browser (`Intl.DateTimeFormat` with `timeZone`).
///
/// # Examples
///
/// ```
/// let options = ocre::helpers::time_zone_options("Europe/Paris");
/// assert!(options.contains("<option selected>Europe/Paris</option>"));
/// assert!(options.contains("<option>Asia/Tokyo</option>"));
/// ```
///
/// In an askama template: `<select name="time_zone">{{ ocre::helpers::time_zone_options(user.time_zone.as_str())|safe }}</select>`.
pub fn time_zone_options(selected: &str) -> String {
    let mut out = String::with_capacity(TIME_ZONES.len() * 32);
    for zone in TIME_ZONES {
        // Zone names are letters, digits, `/`, `_`, `-` and `+`: nothing to escape.
        let mark = if *zone == selected { " selected" } else { "" };
        write!(out, "<option{mark}>{zone}</option>").expect("writing to a String");
    }
    out
}

#[cfg(test)]
#[path = "../tests/helpers.rs"]
mod tests;

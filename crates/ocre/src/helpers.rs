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
//! of CPU, no binding call. English only; translate with
//! [`i18n`](crate::i18n) where needed.

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
/// the year), `%F` (2026-09-29), `%T` (14:05:00), `%%`; `%-d`, `%-m`, `%-H`,
/// `%-I` drop the zero padding. Unknown directives are copied as they are.
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

fn number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

/// `-1234567.5` -> `-1,234,567.5`; anything else unchanged.
fn with_delimiter(text: &str) -> String {
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
            out.push(',');
        }
        out.push(digit);
    }
    if let Some(frac) = frac {
        out.push('.');
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
fn parse_time(text: &str) -> Option<i64> {
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
fn civil_from_days(days: i64) -> (i64, i64, i64) {
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

const MONTHS: [&str; 12] = [
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
];
const WEEKDAYS: [&str; 7] = ["Thursday", "Friday", "Saturday", "Sunday", "Monday", "Tuesday", "Wednesday"];

fn format_time(time: i64, format: &str) -> String {
    let days = time.div_euclid(86_400);
    let of_day = time.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (of_day / 3600, of_day % 3600 / 60, of_day % 60);
    let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    let month_name = MONTHS[(month - 1) as usize];
    let weekday = WEEKDAYS[days.rem_euclid(7) as usize];
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
            Some('p') => write!(out, "{}", if hour < 12 { "AM" } else { "PM" }),
            Some('b') => write!(out, "{}", &month_name[..3]),
            Some('B') => write!(out, "{month_name}"),
            Some('a') => write!(out, "{}", &weekday[..3]),
            Some('A') => write!(out, "{weekday}"),
            Some('j') => write!(out, "{:03}", days - days_from_civil(year, 1, 1) + 1),
            Some('F') => write!(out, "{year}-{month:02}-{day:02}"),
            Some('T') => write!(out, "{hour:02}:{minute:02}:{second:02}"),
            Some('%') => write!(out, "%"),
            Some(other) => write!(out, "%{}{other}", if unpadded { "-" } else { "" }),
            None => write!(out, "%"),
        };
        written.expect("writing to a String");
    }
    out
}

/// Rails' `distance_of_time_in_words`, without seconds precision.
fn distance_in_words(from: i64, to: i64) -> String {
    const HOUR: i64 = 60;
    const DAY: i64 = 1440;
    const MONTH: i64 = 43_200;
    const YEAR: i64 = 525_600;
    let minutes = ((from - to).abs() + 30) / 60;
    let plural = |n: i64, unit: &str| if n == 1 { format!("1 {unit}") } else { format!("{n} {unit}s") };
    let rounded = |unit: i64| (minutes + unit / 2) / unit;
    match minutes {
        0 => "less than a minute".to_owned(),
        1..45 => plural(minutes, "minute"),
        45..90 => "about 1 hour".to_owned(),
        90..DAY => format!("about {}", plural(rounded(HOUR), "hour")),
        DAY..2520 => "1 day".to_owned(),
        2520..MONTH => plural(rounded(DAY), "day"),
        MONTH..86_400 => format!("about {}", plural(rounded(MONTH), "month")),
        86_400..YEAR => plural(rounded(MONTH), "month"),
        _ => {
            let (years, remainder) = (minutes / YEAR, minutes % YEAR);
            if remainder < YEAR / 4 {
                format!("about {}", plural(years, "year"))
            } else if remainder < YEAR * 3 / 4 {
                format!("over {}", plural(years, "year"))
            } else {
                format!("almost {}", plural(years + 1, "year"))
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

#[cfg(test)]
#[path = "../tests/helpers.rs"]
mod tests;

//! [`scrub`]: personal data and secrets out of error reports, ids and durations normalized.

use serde_json::Value;

use super::Report;

/// Prefixes of well-known API keys and tokens.
const SECRET_PREFIXES: &[&str] = &[
    "whsec_",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "rk_test_",
    "pk_live_",
    "pk_test_",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "phc_",
    "phx_",
    "AKIA",
];

/// Query-string parameters whose value is a credential.
const SECRET_PARAMS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "access_token",
    "api_key",
    "apikey",
    "key",
    "signature",
    "sig",
    "X-Amz-Signature",
    "X-Amz-Credential",
];

/// Replaces what must not leave the app in an error report, and what keeps the same error from grouping.
///
/// For [`Subscriber`](crate::errors::Subscriber)s that send reports to an
/// outside service ([`Report::scrubbed`] applies it to a whole report):
///
/// | Found | Becomes |
/// |---|---|
/// | `Bearer <token>`, `Basic <credentials>`, well-known key prefixes (`sk_live_`, `whsec_`, `ghp_`, `AKIA`...) | `[secret]` |
/// | `password=`, `token=`, `key=`, `signature=`... query values | `<name>=[secret]` |
/// | email addresses | `[email]` |
/// | UUIDs | `[id]` |
/// | runs of 32 or more letters and digits (tokens, digests) | `[secret]` |
/// | durations (`12 ms`, `3.5s`, `40µs`) | `[n]ms`, `[n]s`, `[n]µs` |
///
/// Pure CPU, a single pass per rule over the text.
///
/// # Examples
///
/// ```
/// use ocre::errors::scrub;
///
/// assert_eq!(scrub("Authorization: Bearer eyJ.abc/def="), "Authorization: [secret]");
/// assert_eq!(scrub("no user ada.l+x@example.co.uk"), "no user [email]");
/// assert_eq!(scrub("video 0b9c2f44-1d2e-4f7a-9c3b-2a1b0c9d8e7f gone"), "video [id] gone");
/// assert_eq!(scrub("GET /reset?token=abc123&x=1 took 12.5 ms"), "GET /reset?token=[secret]&x=1 took [n]ms");
/// ```
pub fn scrub(text: &str) -> String {
    let text = secrets(text);
    let text = params(&text);
    let text = emails(&text);
    let text = uuids(&text);
    let text = tokens(&text);
    durations(&text)
}

impl Report {
    /// The report with [`scrub`] applied to its message and every text in its context.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::errors::{Options, Reporter};
    ///
    /// let reporter = Reporter::default();
    /// reporter.report(&"no user ada@example.com", Options::new().context("email", "ada@example.com"));
    /// let report = reporter.take().remove(0).scrubbed();
    /// assert_eq!(report.message, "no user [email]");
    /// assert_eq!(report.context["email"], "[email]");
    /// ```
    pub fn scrubbed(mut self) -> Self {
        self.message = scrub(&self.message);
        for value in self.context.values_mut() {
            scrub_value(value);
        }
        self
    }
}

fn scrub_value(value: &mut Value) {
    match value {
        Value::String(text) => *text = scrub(text),
        Value::Array(items) => items.iter_mut().for_each(scrub_value),
        Value::Object(map) => map.values_mut().for_each(scrub_value),
        _ => {}
    }
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Replaces the ranges `find(chars, i) -> Some((i, end))` returns with `with(matched)`.
fn replace(text: &str, with: impl Fn(&[char]) -> String, find: impl Fn(&[char], usize) -> Option<usize>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        match find(&chars, i) {
            Some(end) if end > i => {
                out.push_str(&with(&chars[i..end]));
                i = end;
            }
            _ => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

fn starts_with(chars: &[char], i: usize, prefix: &str) -> bool {
    (i..).zip(prefix.chars()).all(|(n, p)| chars.get(n) == Some(&p))
}

fn run(chars: &[char], from: usize, allowed: impl Fn(char) -> bool) -> usize {
    let mut end = from;
    while end < chars.len() && allowed(chars[end]) {
        end += 1;
    }
    end
}

fn boundary(chars: &[char], i: usize) -> bool {
    i == 0 || !word(chars[i - 1])
}

fn secrets(text: &str) -> String {
    replace(
        text,
        |_| "[secret]".to_owned(),
        |chars, i| {
            if !boundary(chars, i) {
                return None;
            }
            for prefix in SECRET_PREFIXES {
                if starts_with(chars, i, prefix) {
                    let start = i + prefix.chars().count();
                    let end = run(chars, start, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                    if end > start {
                        return Some(end);
                    }
                }
            }
            for scheme in ["Bearer", "Basic"] {
                if starts_with(chars, i, scheme) {
                    let after = i + scheme.len();
                    let spaces = run(chars, after, |c| c == ' ' || c == '\t');
                    let end = run(chars, spaces, |c| c.is_ascii_alphanumeric() || "._~+/=-".contains(c));
                    if spaces > after && end > spaces {
                        return Some(end);
                    }
                }
            }
            None
        },
    )
}

/// `name=value` of a credential parameter: the value only.
fn params(text: &str) -> String {
    replace(
        text,
        |matched| {
            let name: String = matched.iter().take_while(|c| **c != '=').collect();
            format!("{name}=[secret]")
        },
        |chars, i| {
            let after_separator = i == 0 || matches!(chars[i - 1], '?' | '&' | ' ' | '"' | '\'' | ';');
            if !after_separator {
                return None;
            }
            let name = SECRET_PARAMS.iter().find(|name| {
                let len = name.chars().count();
                chars.get(i..i + len).is_some_and(|got| got.iter().copied().eq(name.chars()))
                    && chars.get(i + len) == Some(&'=')
            })?;
            let start = i + name.chars().count() + 1;
            let end = run(chars, start, |c| !c.is_whitespace() && !"&\"'#;".contains(c));
            (end > start).then_some(end)
        },
    )
}

/// `[\w.+-]+@[\w-]+\.[\w.-]+`.
fn emails(text: &str) -> String {
    let local = |c: char| word(c) || ".+-".contains(c);
    replace(
        text,
        |_| "[email]".to_owned(),
        |chars, i| {
            if !local(chars[i]) || (i > 0 && local(chars[i - 1])) {
                return None;
            }
            let at = run(chars, i, local);
            if chars.get(at) != Some(&'@') {
                return None;
            }
            let domain = run(chars, at + 1, |c| word(c) || c == '-');
            if domain == at + 1 || chars.get(domain) != Some(&'.') {
                return None;
            }
            let end = run(chars, domain + 1, |c| word(c) || ".-".contains(c));
            (end > domain + 1).then_some(end)
        },
    )
}

fn uuids(text: &str) -> String {
    replace(
        text,
        |_| "[id]".to_owned(),
        |chars, i| {
            if !boundary(chars, i) {
                return None;
            }
            let mut n = i;
            for (k, len) in [8, 4, 4, 4, 12].into_iter().enumerate() {
                if k > 0 {
                    if chars.get(n) != Some(&'-') {
                        return None;
                    }
                    n += 1;
                }
                let end = run(chars, n, |c| c.is_ascii_hexdigit());
                if end - n != len {
                    return None;
                }
                n = end;
            }
            chars.get(n).is_none_or(|c| !word(*c)).then_some(n)
        },
    )
}

/// Runs of 32 or more URL-safe characters mixing letters and digits.
fn tokens(text: &str) -> String {
    let token = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    replace(
        text,
        |_| "[secret]".to_owned(),
        |chars, i| {
            if i > 0 && token(chars[i - 1]) {
                return None;
            }
            let end = run(chars, i, token);
            let run = &chars[i..end];
            let mixed = run.iter().any(char::is_ascii_digit) && run.iter().any(char::is_ascii_alphabetic);
            (end - i >= 32 && mixed).then_some(end)
        },
    )
}

/// `\b\d+(\.\d+)?\s?(ms|µs|s)\b` → `[n]<unit>`.
fn durations(text: &str) -> String {
    replace(
        text,
        |matched| {
            let unit = if matched.ends_with(&['m', 's']) {
                "ms"
            } else if matched.ends_with(&['µ', 's']) {
                "µs"
            } else {
                "s"
            };
            format!("[n]{unit}")
        },
        |chars, i| {
            if !boundary(chars, i) || !chars[i].is_ascii_digit() {
                return None;
            }
            let mut end = run(chars, i, |c| c.is_ascii_digit());
            if chars.get(end) == Some(&'.') && chars.get(end + 1).is_some_and(char::is_ascii_digit) {
                end = run(chars, end + 1, |c| c.is_ascii_digit());
            }
            let unit_at = if chars.get(end) == Some(&' ') { end + 1 } else { end };
            ["ms", "µs", "s"].into_iter().find_map(|unit| {
                let after = unit_at + unit.chars().count();
                (starts_with(chars, unit_at, unit) && chars.get(after).is_none_or(|c| !word(*c))).then_some(after)
            })
        },
    )
}

#[cfg(test)]
#[path = "../../tests/errors/scrub.rs"]
mod tests;

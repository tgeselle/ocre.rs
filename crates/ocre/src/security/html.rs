//! Cleaning user-supplied HTML and escaping values for scripts.

use std::fmt::Write as _;

/// Tags [`sanitize`] keeps: Rails' safe list (`Rails::HTML5::SafeListSanitizer`).
///
/// # Examples
///
/// ```
/// assert!(ocre::security::SANITIZE_TAGS.contains(&"strong"));
/// assert!(!ocre::security::SANITIZE_TAGS.contains(&"script"));
/// ```
pub const SANITIZE_TAGS: &[&str] = &[
    "a",
    "abbr",
    "acronym",
    "address",
    "b",
    "big",
    "blockquote",
    "br",
    "cite",
    "code",
    "dd",
    "del",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "pre",
    "samp",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "time",
    "tt",
    "ul",
    "var",
];

/// Attributes [`sanitize`] keeps on those tags: Rails' safe list.
///
/// `href`, `src` and `cite` also need an `http:`, `https:`, `mailto:` or
/// `tel:` URL (or a relative one). `style` and event handlers (`onclick`...)
/// are never kept.
///
/// # Examples
///
/// ```
/// assert!(ocre::security::SANITIZE_ATTRIBUTES.contains(&"href"));
/// assert!(!ocre::security::SANITIZE_ATTRIBUTES.contains(&"style"));
/// ```
pub const SANITIZE_ATTRIBUTES: &[&str] = &[
    "abbr", "alt", "cite", "class", "datetime", "height", "href", "lang", "name", "src", "title", "width", "xml:lang",
];

/// Elements removed with their content, whatever the allow list says.
const DROPPED_WITH_CONTENT: &[&str] = &[
    "script", "style", "template", "noscript", "iframe", "noembed", "noframes", "xmp", "textarea", "title", "svg",
    "math",
];
/// Elements without a closing tag.
const VOID: &[&str] = &["br", "hr", "img", "wbr"];
/// Attributes holding a URL, whose scheme is checked.
const URL_ATTRIBUTES: &[&str] = &["href", "src", "cite", "action", "formaction", "poster", "background"];
/// URL schemes kept in URL attributes.
const SAFE_SCHEMES: &[&str] = &["http", "https", "mailto", "tel"];

/// Cleans user-supplied HTML down to [`SANITIZE_TAGS`] and [`SANITIZE_ATTRIBUTES`] (Rails' `sanitize`).
///
/// The output is rebuilt, not filtered: kept tags are written back with
/// quoted, escaped attributes; other tags are removed but their text kept;
/// `<script>`, `<style>`, `<iframe>`, `<svg>` and similar are removed with
/// their content; comments are removed; `href`/`src` with a scheme other
/// than `http`, `https`, `mailto` or `tel` (e.g. `javascript:`) are removed;
/// text is escaped; unclosed tags are closed. The result is safe to insert
/// with askama's `|safe`: `{{ comment.body_html|safe }}`.
///
/// Sanitize when saving (store the clean HTML) rather than on every render:
/// it costs CPU proportional to the input, about 1 ms per 100 KB.
///
/// # Examples
///
/// ```
/// use ocre::security::sanitize;
///
/// assert_eq!(
///     sanitize(r#"<p onclick="steal()">Hi <b>there</b><script>alert(1)</script></p>"#),
///     "<p>Hi <b>there</b></p>"
/// );
/// assert_eq!(sanitize(r#"<a href="javascript:alert(1)">x</a>"#), "<a>x</a>");
/// assert_eq!(sanitize(r#"<a href="https://example.com" target="_blank">x</a>"#), r#"<a href="https://example.com">x</a>"#);
/// assert_eq!(sanitize("<em>unclosed"), "<em>unclosed</em>");
/// assert_eq!(sanitize("1 < 2 & 3"), "1 &lt; 2 &amp; 3");
/// ```
pub fn sanitize(html: &str) -> String {
    sanitize_with(html, SANITIZE_TAGS, SANITIZE_ATTRIBUTES)
}

/// Like [`sanitize`], with your own allowed tags and attributes (Rails' `sanitize(html, tags:, attributes:)`).
///
/// Names are lowercase. Script-like elements, event handlers and unsafe URL
/// schemes stay out whatever the lists say.
///
/// # Examples
///
/// ```
/// use ocre::security::sanitize_with;
///
/// let html = r#"<p class="x"><a href="/about" title="About">About</a> <img src="x.png"></p>"#;
/// assert_eq!(sanitize_with(html, &["a"], &["href"]), r#"<a href="/about">About</a> "#);
/// ```
pub fn sanitize_with(html: &str, tags: &[&str], attributes: &[&str]) -> String {
    Cleaner { tags, attributes, out: String::with_capacity(html.len()), open: Vec::new() }.run(html)
}

/// Removes every tag and comment and keeps the text, escaped (Rails' `strip_tags`).
///
/// `<script>` and `<style>` content is removed too. The result is HTML
/// (`&lt;` stays escaped): insert it with `|safe`, or decode it for plain
/// text.
///
/// # Examples
///
/// ```
/// use ocre::security::strip_tags;
///
/// assert_eq!(strip_tags("<p>Hello <b>world</b>!</p><!-- note -->"), "Hello world!");
/// assert_eq!(strip_tags("<script>alert(1)</script>a &lt; b"), "a &lt; b");
/// ```
pub fn strip_tags(html: &str) -> String {
    sanitize_with(html, &[], &[])
}

/// Escapes a JSON string for a `<script>` element (Rails' `json_escape`).
///
/// `<`, `>` and `&` become `\u003c`, `\u003e` and `\u0026`, and U+2028 and
/// U+2029 become escapes, so the JSON cannot close the script element or
/// break JavaScript parsing; it still parses to the same value.
///
/// # Examples
///
/// ```
/// use ocre::security::json_escape;
///
/// let json = serde_json::json!({ "title": "</script><script>alert(1)</script>" }).to_string();
/// let safe = json_escape(&json);
/// assert_eq!(safe, r#"{"title":"\u003c/script\u003e\u003cscript\u003ealert(1)\u003c/script\u003e"}"#);
/// assert_eq!(serde_json::from_str::<serde_json::Value>(&safe)?["title"], "</script><script>alert(1)</script>");
/// # Ok::<(), serde_json::Error>(())
/// ```
pub fn json_escape(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// Escapes text for a JavaScript string literal in single, double or back quotes (Rails' `escape_javascript`).
///
/// Backslashes, quotes, `` ` ``, `$`, newlines, U+2028/U+2029 and `</` are
/// escaped. Prefer passing data as JSON ([`json_escape`]) or `data-`
/// attributes; with a Content-Security-Policy, inline scripts also need a
/// nonce ([`CspNonce`](super::CspNonce)).
///
/// # Examples
///
/// ```
/// use ocre::security::escape_javascript;
///
/// assert_eq!(escape_javascript("It's \"fine\"\n</script>"), r#"It\'s \"fine\"\n<\/script>"#);
/// ```
pub fn escape_javascript(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '`' => out.push_str("\\`"),
            '$' => out.push_str("\\$"),
            '\r' => {
                chars.next_if_eq(&'\n');
                out.push_str("\\n");
            }
            '\n' => out.push_str("\\n"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            '<' if chars.peek() == Some(&'/') => out.push_str("<\\"),
            c => out.push(c),
        }
    }
    out
}

struct Cleaner<'a> {
    tags: &'a [&'a str],
    attributes: &'a [&'a str],
    out: String,
    /// Kept elements not closed yet, innermost last.
    open: Vec<String>,
}

impl Cleaner<'_> {
    fn run(mut self, html: &str) -> String {
        let mut rest = html;
        while let Some(start) = rest.find('<') {
            self.text(&rest[..start]);
            rest = &rest[start..];
            rest = self.markup(rest);
        }
        self.text(rest);
        while let Some(name) = self.open.pop() {
            let _ = write!(self.out, "</{name}>");
        }
        self.out
    }

    /// Handles the markup at the start of `rest` (which starts with `<`) and returns what follows.
    fn markup<'h>(&mut self, rest: &'h str) -> &'h str {
        if let Some(comment) = rest.strip_prefix("<!--") {
            return comment.find("-->").map_or("", |end| &comment[end + 3..]);
        }
        let after = &rest[1..];
        if after.starts_with(['!', '?']) {
            // Doctype, CDATA, processing instruction: removed.
            return after.find('>').map_or("", |end| &after[end + 1..]);
        }
        let closing = after.starts_with('/');
        let name_start = if closing { &after[1..] } else { after };
        if !name_start.starts_with(|c: char| c.is_ascii_alphabetic()) {
            self.out.push_str("&lt;");
            return after;
        }
        let Some((tag, following)) = parse_tag(name_start) else {
            // Never closed: the rest is text.
            self.text(rest);
            return "";
        };
        if closing {
            self.close(&tag.name);
            return following;
        }
        if DROPPED_WITH_CONTENT.contains(&tag.name.as_str()) {
            return skip_element(following, &tag.name);
        }
        if self.tags.contains(&tag.name.as_str()) {
            self.open_tag(&tag);
        }
        following
    }

    fn open_tag(&mut self, tag: &Tag) {
        self.out.push('<');
        self.out.push_str(&tag.name);
        for (name, value) in &tag.attributes {
            if !self.attributes.contains(&name.as_str()) || name.starts_with("on") {
                continue;
            }
            if URL_ATTRIBUTES.contains(&name.as_str()) && !safe_url(value) {
                continue;
            }
            let _ = write!(self.out, " {name}=\"");
            escape_into(&mut self.out, value, true);
            self.out.push('"');
        }
        self.out.push('>');
        if !VOID.contains(&tag.name.as_str()) {
            self.open.push(tag.name.clone());
        }
    }

    /// Closes `name` and the kept elements opened inside it; ignores it when it is not open.
    fn close(&mut self, name: &str) {
        let Some(index) = self.open.iter().rposition(|open| open == name) else { return };
        for name in self.open.drain(index..).rev() {
            let _ = write!(self.out, "</{name}>");
        }
    }

    fn text(&mut self, text: &str) {
        escape_into(&mut self.out, text, false);
    }
}

struct Tag {
    name: String,
    /// Lowercase names, entity-decoded values.
    attributes: Vec<(String, String)>,
}

/// Parses `name attr="value" ...>` and returns the tag and what follows `>`,
/// or `None` when the tag never ends.
fn parse_tag(input: &str) -> Option<(Tag, &str)> {
    let name_end = input.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == ':')).unwrap_or(input.len());
    let name = input[..name_end].to_ascii_lowercase();
    let mut rest = &input[name_end..];
    let mut attributes = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if let Some(after) = rest.strip_prefix('>') {
            return Some((Tag { name, attributes }, after));
        }
        if rest.is_empty() {
            return None;
        }
        let attr_end =
            rest.find(|c: char| c.is_ascii_whitespace() || matches!(c, '=' | '>' | '/')).unwrap_or(rest.len());
        // A lone `=` or other junk still moves forward by at least one character.
        let attr_end = attr_end.max(rest.chars().next().map_or(1, char::len_utf8));
        let attr = rest[..attr_end].to_ascii_lowercase();
        rest = rest[attr_end..].trim_start_matches(|c: char| c.is_ascii_whitespace());
        let mut value = String::new();
        if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start_matches(|c: char| c.is_ascii_whitespace());
            let (raw, following) = match after.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let body = &after[1..];
                    let end = body.find(quote)?;
                    (&body[..end], &body[end + 1..])
                }
                _ => {
                    let end = after.find(|c: char| c.is_ascii_whitespace() || c == '>').unwrap_or(after.len());
                    (&after[..end], &after[end..])
                }
            };
            value = decode_entities(raw);
            rest = following;
        }
        attributes.push((attr, value));
    }
}

/// Skips to after `</name>` (any case), or to the end.
fn skip_element<'h>(input: &'h str, name: &str) -> &'h str {
    let lower = input.to_ascii_lowercase();
    let closing = format!("</{name}");
    let mut from = 0;
    while let Some(found) = lower[from..].find(&closing) {
        let at = from + found + closing.len();
        if lower[at..].starts_with(|c: char| c == '>' || c.is_ascii_whitespace() || c == '/') {
            return input[at..].find('>').map_or("", |end| &input[at + end + 1..]);
        }
        from = at;
    }
    ""
}

/// Whether a (decoded) URL is relative or uses a safe scheme.
fn safe_url(url: &str) -> bool {
    // Browsers ignore tabs and newlines in URLs and trim control characters and spaces.
    let url: String =
        url.chars().filter(|c| !c.is_ascii_control() && *c != ' ').collect::<String>().to_ascii_lowercase();
    match url.find([':', '/', '?', '#']) {
        Some(index) if url.as_bytes()[index] == b':' => SAFE_SCHEMES.contains(&&url[..index]),
        _ => true,
    }
}

/// Escapes `<`, `>`, `"` (in attributes) and `&`, keeping well-formed entity references in text.
fn escape_into(out: &mut String, text: &str, attribute: bool) {
    for (index, c) in text.char_indices() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '&' if attribute || entity_len(&text[index..]).is_none() => out.push_str("&amp;"),
            c => out.push(c),
        }
    }
}

/// Length of the entity reference at the start of `text` (`&amp;`, `&#39;`, `&#x27;`), if it is one.
fn entity_len(text: &str) -> Option<usize> {
    let body = text.strip_prefix('&')?;
    let end = body.find(';')?;
    let name = &body[..end];
    let valid = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        !hex.is_empty() && hex.len() <= 6 && hex.bytes().all(|b| b.is_ascii_hexdigit())
    } else if let Some(decimal) = name.strip_prefix('#') {
        !decimal.is_empty() && decimal.len() <= 7 && decimal.bytes().all(|b| b.is_ascii_digit())
    } else {
        (2..=31).contains(&name.len())
            && name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name.bytes().all(|b| b.is_ascii_alphanumeric())
    };
    valid.then_some(end + 2)
}

/// Decodes numeric references and the common named ones; others stay as written.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        // Browsers accept numeric references without the `;` in attributes.
        let body = &rest[1..];
        let (decoded, used) = if let Some(number) = body.strip_prefix('#') {
            let (digits, radix) = match number.strip_prefix(['x', 'X']) {
                Some(hex) => (hex, 16),
                None => (number, 10),
            };
            let len = digits.find(|c: char| !c.is_digit(radix)).unwrap_or(digits.len());
            let prefix = body.len() - number.len() + (number.len() - digits.len());
            let value = u32::from_str_radix(&digits[..len], radix).ok().and_then(char::from_u32);
            let semicolon = usize::from(digits[len..].starts_with(';'));
            (value, 1 + prefix + len + semicolon)
        } else {
            let named = [
                ("amp;", '&'),
                ("lt;", '<'),
                ("gt;", '>'),
                ("quot;", '"'),
                ("apos;", '\''),
                ("colon;", ':'),
                ("tab;", '\t'),
                ("newline;", '\n'),
            ];
            let lower = body.get(..8).unwrap_or(body).to_ascii_lowercase();
            named
                .iter()
                .find(|(name, _)| lower.starts_with(name))
                .map_or((None, 1), |(name, c)| (Some(*c), 1 + name.len()))
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push('&'),
        }
        rest = if decoded.is_some() { &rest[used..] } else { &rest[1..] };
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[path = "../../tests/security/html.rs"]
mod tests;

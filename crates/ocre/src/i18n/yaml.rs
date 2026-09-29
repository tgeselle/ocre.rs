//! The YAML subset of locale files: nested mappings of strings, the part of
//! YAML that Rails locale files use. Every file Ocre accepts is valid YAML
//! with the same meaning; anything else is an error naming the line and the fix.
//!
//! ```yaml
//! fr:
//!   posts:
//!     created: "Article créé."   # comment
//!     count:
//!       one: "%{count} article"
//!       other: '%{count} articles'
//! ```

use std::{borrow::Cow, collections::BTreeSet};

/// A syntax error, with its 1-based line number.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParseError {
    pub line: usize,
    pub message: String,
}

impl ParseError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self { line, message: message.into() }
    }
}

/// An open mapping: its indentation, key and the indentation of its children.
struct Frame {
    indent: usize,
    key: String,
    /// Line of the key, for "has no value" errors.
    line: usize,
    children: Option<usize>,
}

/// Parses the file of locale `code`: one root key, `code:`, holding nested
/// mappings. Returns every string with its dotted key (`posts.created`),
/// without the locale, in file order.
pub(crate) fn parse_locale<'t>(code: &str, text: &'t str) -> Result<Vec<(String, Cow<'t, str>)>, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut entries: Vec<(String, Cow<'t, str>)> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut root_seen = false;
    let mut seen = BTreeSet::new();
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let content = line.trim_start_matches(' ');
        if content.is_empty() || content.starts_with('#') || (!root_seen && content == "---") {
            continue;
        }
        let indent = line.len() - content.len();
        if content.starts_with('\t') {
            return Err(ParseError::new(number, "indent with spaces, not tabs"));
        }
        if content.starts_with("- ") || content == "-" {
            return Err(ParseError::new(number, "lists are not supported: use one key per string"));
        }
        let (key, rest) =
            split_key(content).ok_or_else(|| ParseError::new(number, "expected `key: value` or `key:`"))?;
        while stack.last().is_some_and(|frame| frame.indent >= indent) {
            close(stack.pop().expect("checked by the loop condition"), stack.is_empty())?;
        }
        match stack.last_mut() {
            None if indent > 0 => return Err(ParseError::new(number, format!("`{code}:` must start the line"))),
            None if root_seen => {
                return Err(ParseError::new(number, format!("only one root key, `{code}:`; indent `{key}` under it")));
            }
            None if key != code || (!rest.is_empty() && !rest.starts_with('#')) => {
                return Err(ParseError::new(number, format!("the file must start with `{code}:` on its own line")));
            }
            None => root_seen = true,
            Some(parent) => match parent.children {
                Some(children) if children != indent => {
                    return Err(ParseError::new(
                        number,
                        format!("inconsistent indentation: siblings of this key are indented {children} spaces"),
                    ));
                }
                _ => parent.children = Some(indent),
            },
        }
        let path = stack.iter().skip(1).map(|frame| frame.key.as_str()).chain([key]).collect::<Vec<_>>().join(".");
        if !stack.is_empty() && !seen.insert(path.clone()) {
            return Err(ParseError::new(number, format!("duplicate key `{path}`")));
        }
        if rest.is_empty() || rest.starts_with('#') {
            stack.push(Frame { indent, key: key.to_owned(), line: number, children: None });
            continue;
        }
        let value = scalar(rest).map_err(|message| ParseError::new(number, message))?;
        entries.push((path, value));
    }
    if !root_seen {
        return Err(ParseError::new(1, format!("the file must start with `{code}:` on its own line")));
    }
    while let Some(frame) = stack.pop() {
        close(frame, stack.is_empty())?;
    }
    Ok(entries)
}

/// A mapping without children is YAML `null`: allowed for the root only (a
/// new, empty locale file).
fn close(frame: Frame, is_root: bool) -> Result<(), ParseError> {
    if frame.children.is_none() && !is_root {
        return Err(ParseError::new(
            frame.line,
            format!("`{}` has no value: write `{}: \"text\"`, or indent keys under it", frame.key, frame.key),
        ));
    }
    Ok(())
}

/// `key: rest` or `key:`; keys are letters, digits, `_` and `-`.
fn split_key(content: &str) -> Option<(&str, &str)> {
    let (key, rest) = content.split_once(':')?;
    let valid = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !valid || !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    Some((key, rest.trim_start_matches(' ')))
}

/// A value: double-quoted, single-quoted or plain, then an optional comment.
fn scalar(rest: &str) -> Result<Cow<'_, str>, String> {
    if let Some(body) = rest.strip_prefix('"') {
        return double_quoted(body);
    }
    if let Some(body) = rest.strip_prefix('\'') {
        return single_quoted(body);
    }
    let first = rest.chars().next().expect("callers pass a non-empty value");
    if "|>".contains(first) {
        return Err("block scalars (`|`, `>`) are not supported: use a double-quoted string with \\n".to_owned());
    }
    if "[{&*!%@`,?-".contains(first) {
        return Err(format!("quote values starting with `{first}`, e.g. \"{}\"", rest.trim_end()));
    }
    let plain = match rest.find(" #") {
        Some(end) => &rest[..end],
        None => rest,
    }
    .trim_end();
    if plain.contains(": ") || plain.ends_with(':') {
        return Err(format!("quote values containing `: ` or ending with `:`, e.g. \"{plain}\""));
    }
    if ["~", "null", "true", "false", "yes", "no", "on", "off"].contains(&plain.to_ascii_lowercase().as_str()) {
        return Err(format!("YAML does not read `{plain}` as text: quote it, \"{plain}\""));
    }
    Ok(Cow::Borrowed(plain))
}

fn double_quoted(body: &str) -> Result<Cow<'_, str>, String> {
    let mut out: Option<String> = None;
    let mut chars = body.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => {
                trailing(&body[at + 1..])?;
                return Ok(out.map_or(Cow::Borrowed(&body[..at]), Cow::Owned));
            }
            '\\' => {
                let text = out.get_or_insert_with(|| body[..at].to_owned());
                match chars.next().map(|(_, escaped)| escaped) {
                    Some('n') => text.push('\n'),
                    Some('t') => text.push('\t'),
                    Some(c @ ('"' | '\\' | '/')) => text.push(c),
                    Some('u') => {
                        let hex: String = chars.by_ref().take(4).map(|(_, c)| c).collect();
                        let c = u32::from_str_radix(&hex, 16).ok().filter(|_| hex.len() == 4).and_then(char::from_u32);
                        text.push(c.ok_or_else(|| format!("`\\u{hex}` is not a character: use \\u and 4 hex digits"))?);
                    }
                    other => {
                        return Err(format!(
                            "unknown escape `\\{}`: use \\n, \\t, \\\", \\\\ or \\uXXXX",
                            other.map(String::from).unwrap_or_default()
                        ));
                    }
                }
            }
            c => {
                if let Some(text) = &mut out {
                    text.push(c);
                }
            }
        }
    }
    Err("close the double quote on the same line".to_owned())
}

fn single_quoted(body: &str) -> Result<Cow<'_, str>, String> {
    let mut out: Option<String> = None;
    let mut rest = body;
    loop {
        let end = rest.find('\'').ok_or("close the single quote on the same line")?;
        let (before, after) = (&rest[..end], &rest[end + 1..]);
        if let Some(next) = after.strip_prefix('\'') {
            let text = out.get_or_insert_with(String::new);
            text.push_str(before);
            text.push('\'');
            rest = next;
            continue;
        }
        trailing(after)?;
        return Ok(match out {
            Some(mut text) => {
                text.push_str(before);
                Cow::Owned(text)
            }
            None => Cow::Borrowed(before),
        });
    }
}

/// After a closing quote: spaces, then nothing or a comment.
fn trailing(after: &str) -> Result<(), String> {
    let after = after.trim_start_matches(' ');
    if after.is_empty() || after.starts_with('#') {
        Ok(())
    } else {
        Err(format!("unexpected `{after}` after the closing quote"))
    }
}

#[cfg(test)]
#[path = "../../tests/i18n/yaml.rs"]
mod tests;

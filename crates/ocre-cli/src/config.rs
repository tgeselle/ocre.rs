//! `cloudflare.config.ts`: the app's Cloudflare configuration, read and
//! edited without Node.
//!
//! Reads: a small structural reader walks `defineConfig({ worker: { ... } })`
//! (comments and strings skipped), finds the `KEY: bindings.<kind>(...)`,
//! `triggers.<kind>(...)` and `KEY: exports.<kind>(...)` calls, and parses
//! each call's arguments as JSON5 (JavaScript object literals with unquoted
//! keys and trailing commas). A value that is not a literal (a variable, a
//! template string) is an error only when Ocre needs that fact.
//!
//! Writes: generators insert canonical entries after the `// ocre:env`,
//! `// ocre:triggers` and `// ocre:exports` markers; `ocre deploy` rewrites
//! the arguments of a KV binding in place to add its `id`. `ocre doctor`
//! validates the whole file with cf's own loader.

use std::{ops::Range, path::Path};

use serde_json::Value;

use crate::{generate::insert_after_marker, output::CliError};

/// The file, relative to the app root.
pub const FILE: &str = "cloudflare.config.ts";
/// How the Rust code is built (read by cf through wrangler).
pub const BUILD_FILE: &str = "wrangler.config.ts";
/// Inside `worker.env`: generators add bindings after it.
pub const ENV_MARKER: &str = "// ocre:env";
/// Inside `worker.triggers`: generators add triggers after it.
pub const TRIGGERS_MARKER: &str = "// ocre:triggers";
/// Inside `worker.exports`: generators add exported classes after it.
pub const EXPORTS_MARKER: &str = "// ocre:exports";

/// One `bindings.<kind>(...)`, `triggers.<kind>(...)` or `exports.<kind>(...)` call.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// `DB` in `DB: bindings.d1(...)`; `None` for triggers.
    pub key: Option<String>,
    /// `d1`, `kv`, `queue`, `scheduled`, `durableObject`...
    pub kind: String,
    /// Byte range of the arguments, between the parentheses.
    pub args: Range<usize>,
    /// The arguments, `None` when they are not literals.
    pub values: Option<Vec<Value>>,
}

impl Call {
    /// A string field of the first (object) argument.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.values.as_ref()?.first()?.get(name)?.as_str()
    }

    /// The first argument when it is a string: `bindings.text("...")`.
    pub fn text(&self) -> Option<&str> {
        self.values.as_ref()?.first()?.as_str()
    }
}

/// The parsed file. Entries keep their byte ranges for in-place edits.
#[derive(Debug)]
pub struct Config {
    text: String,
    /// `worker.name`, when a string literal.
    pub name: Option<String>,
    /// `worker.compatibilityDate`, when a string literal.
    pub compatibility_date: Option<String>,
    /// `worker.env` entries that are `bindings.*` calls, in file order.
    pub env: Vec<Call>,
    /// `worker.triggers` entries that are `triggers.*` calls.
    pub triggers: Vec<Call>,
    /// `worker.exports` entries that are `exports.*` calls.
    pub exports: Vec<Call>,
    /// `worker.domains` (custom domains) when a list of string literals.
    domains: Option<Vec<String>>,
    /// Byte range of the `worker.domains` value, when present.
    domains_at: Option<Range<usize>>,
    /// Byte range of the `worker.name` value, when present.
    name_at: Option<Range<usize>>,
}

impl Config {
    /// Reads `<root>/cloudflare.config.ts`.
    pub fn read(root: &Path) -> Result<Self, CliError> {
        Self::parse(std::fs::read_to_string(root.join(FILE))?)
    }

    pub fn parse(text: String) -> Result<Self, CliError> {
        let reader = Reader::new(&text);
        let worker = reader.worker().ok_or_else(|| {
            CliError::new(format!("{FILE} has no `export default defineConfig({{ worker: {{ ... }} }})`")).hint(
                "restore the structure `ocre new` writes: `export default defineConfig({ worker: { name, env: { ... }, triggers: [ ... ], exports: { ... } } })`",
            )
        })?;
        let mut config = Self {
            name: None,
            compatibility_date: None,
            env: Vec::new(),
            triggers: Vec::new(),
            exports: Vec::new(),
            domains: None,
            domains_at: None,
            name_at: None,
            text: String::new(),
        };
        for (key, value) in reader.properties(worker) {
            match key.as_str() {
                "name" => {
                    config.name = reader.string(value.clone());
                    config.name_at = Some(value);
                }
                "compatibilityDate" => config.compatibility_date = reader.string(value),
                "env" => config.env = reader.calls_in_object(value, "bindings")?,
                "exports" => config.exports = reader.calls_in_object(value, "exports")?,
                "triggers" => config.triggers = reader.calls_in_array(value, "triggers"),
                "domains" => {
                    config.domains = json5::from_str::<Vec<String>>(&text[value.clone()]).ok();
                    config.domains_at = Some(value);
                }
                _ => {}
            }
        }
        config.text = text;
        Ok(config)
    }

    /// The Worker's name, needed by most remote operations.
    pub fn worker_name(&self) -> Result<&str, CliError> {
        self.name.as_deref().ok_or_else(|| {
            CliError::new(format!("{FILE} has no `worker.name` Ocre can read"))
                .hint("write it as a string literal inside `worker: { ... }`: `name: \"<app-name>\",`")
        })
    }

    /// The `env` binding named `key`.
    pub fn binding(&self, key: &str) -> Option<&Call> {
        self.env.iter().find(|call| call.key.as_deref() == Some(key))
    }

    /// `env` bindings of one kind, e.g. `kv`.
    pub fn bindings<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Call> {
        self.env.iter().filter(move |call| call.kind == kind)
    }

    /// Triggers of one kind, e.g. `scheduled`.
    pub fn triggers<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Call> {
        self.triggers.iter().filter(move |call| call.kind == kind)
    }

    /// The `exports` entry named `key`.
    pub fn export(&self, key: &str) -> Option<&Call> {
        self.exports.iter().find(|call| call.key.as_deref() == Some(key))
    }

    /// A string field of the `key` binding's options, e.g. `DB`'s `name`;
    /// an error naming the canonical form when it cannot be read.
    pub fn binding_field(&self, key: &str, field: &str, canonical: &str) -> Result<String, CliError> {
        self.binding(key)
            .and_then(|call| call.field(field))
            .map(str::to_owned)
            .ok_or_else(|| unreadable(key, canonical))
    }

    /// `name` of the `DB` D1 binding.
    pub fn database_name(&self) -> Result<String, CliError> {
        let name = self.name.as_deref().unwrap_or("<app-name>");
        let canonical = format!("DB: bindings.d1({{ name: \"{name}\" }}),");
        match self.binding("DB") {
            Some(call) if call.kind == "d1" => {
                call.field("name").map(str::to_owned).ok_or_else(|| unreadable("DB", &canonical))
            }
            Some(_) => Err(unreadable("DB", &canonical)),
            None => Err(CliError::new(format!("{FILE} has no D1 database bound to `DB`"))
                .hint(format!("add `{canonical}` inside `worker.env`"))),
        }
    }

    /// Plain-text variables: `KEY: bindings.text("value")`, value `None`
    /// when not a literal.
    pub fn vars(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.bindings("text").filter_map(|call| Some((call.key.as_deref()?, call.text())))
    }

    /// Every queue the file names, once each, in order: producers' `name`,
    /// then consumers' `name` and `deadLetterQueue`.
    pub fn queue_names(&self) -> Result<Vec<String>, CliError> {
        let mut names = Vec::new();
        for call in self.bindings("queue") {
            let key = call.key.as_deref().unwrap_or("JOBS");
            let name = call
                .field("name")
                .ok_or_else(|| unreadable(key, &format!("{key}: bindings.queue({{ name: \"<queue>\" }}),")))?;
            push_once(&mut names, name);
        }
        for call in self.triggers("queue") {
            let name = call.field("name").ok_or_else(|| {
                CliError::new(format!("{FILE} has a queue trigger Ocre cannot read"))
                    .hint("write it as `triggers.queue({ name: \"<queue>\", deadLetterQueue: \"<queue>-failed\" }),`")
            })?;
            push_once(&mut names, name);
            if let Some(dead_letter) = call.field("deadLetterQueue") {
                push_once(&mut names, dead_letter);
            }
        }
        Ok(names)
    }

    /// The `name` of each R2 binding, once each.
    pub fn bucket_names(&self) -> Result<Vec<String>, CliError> {
        let mut names = Vec::new();
        for call in self.bindings("r2") {
            let key = call.key.as_deref().unwrap_or("STORAGE");
            let name = call
                .field("name")
                .ok_or_else(|| unreadable(key, &format!("{key}: bindings.r2({{ name: \"<bucket>\" }}),")))?;
            push_once(&mut names, name);
        }
        Ok(names)
    }

    /// KV bindings without an `id`: `ocre deploy` creates or finds their namespace.
    pub fn kv_without_id(&self) -> Vec<&str> {
        self.bindings("kv").filter(|call| call.field("id").is_none()).filter_map(|call| call.key.as_deref()).collect()
    }

    /// Cron expressions of the `triggers.scheduled` entries.
    pub fn crons(&self) -> Result<Vec<String>, CliError> {
        self.triggers("scheduled")
            .map(|call| {
                call.field("schedule").map(str::to_owned).ok_or_else(|| {
                    CliError::new(format!("{FILE} has a scheduled trigger Ocre cannot read"))
                        .hint("write it as `triggers.scheduled({ schedule: \"0 3 * * *\" }),`")
                })
            })
            .collect()
    }

    /// The text with `entry` added after `marker` (with the marker's
    /// indentation); unchanged when the entry is already there.
    pub fn insert(&self, marker: &str, entry: &str) -> Result<String, CliError> {
        insert_after_marker(&self.text, marker, entry).ok_or_else(|| {
            let place = match marker {
                ENV_MARKER => "inside `worker.env: { ... }`",
                TRIGGERS_MARKER => "inside `worker.triggers: [ ... ]`",
                _ => "inside `worker.exports: { ... }`",
            };
            CliError::new(format!("{FILE} is missing the `{marker}` marker"))
                .hint(format!("put `{marker}` on its own line {place}: Ocre adds its entries after it"))
        })
    }

    /// The text with `id` added to the options of the `binding` KV binding:
    /// `bindings.kv()` becomes `bindings.kv({ id: "<id>" })`.
    pub fn set_kv_id(&self, binding: &str, id: &str) -> Result<String, CliError> {
        let canonical = format!("{binding}: bindings.kv(),");
        let call =
            self.binding(binding).filter(|call| call.kind == "kv").ok_or_else(|| unreadable(binding, &canonical))?;
        let args = &self.text[call.args.clone()];
        let compact: String = args.split_whitespace().collect();
        let replacement = match compact.as_str() {
            "" | "{}" => format!("{{ id: \"{id}\" }}"),
            object if object.starts_with('{') && call.values.is_some() => {
                let open = call.args.start + args.find('{').expect("starts with a brace");
                return Ok(format!("{} id: \"{id}\",{}", &self.text[..=open], &self.text[open + 1..]));
            }
            _ => return Err(unreadable(binding, &canonical)),
        };
        Ok(format!("{}{replacement}{}", &self.text[..call.args.start], &self.text[call.args.end..]))
    }

    /// `worker.domains`: the Worker's custom domains (none when absent).
    pub fn domains(&self) -> Result<Vec<String>, CliError> {
        match (&self.domains, &self.domains_at) {
            (Some(domains), _) => Ok(domains.clone()),
            (None, None) => Ok(Vec::new()),
            (None, Some(_)) => Err(CliError::new(format!("{FILE} has a `domains` entry Ocre cannot read"))
                .hint("write it as a list of string literals: `domains: [\"www.example.com\"],`")),
        }
    }

    /// The text with `worker.domains` set to `domains`: the list replaced,
    /// or added on the line after `worker.name` when the file has none.
    pub fn with_domains(&self, domains: &[String]) -> Result<String, CliError> {
        let list = format!("[{}]", domains.iter().map(|d| format!("\"{d}\"")).collect::<Vec<_>>().join(", "));
        if let Some(at) = &self.domains_at {
            self.domains()?;
            return Ok(format!("{}{list}{}", &self.text[..at.start], &self.text[at.end..]));
        }
        let Some(name) = &self.name_at else { return Err(self.worker_name().expect_err("no name entry")) };
        let line_start = self.text[..name.start].rfind('\n').map_or(0, |at| at + 1);
        let indent: String = self.text[line_start..].chars().take_while(|c| c.is_whitespace()).collect();
        let line_end = self.text[name.end..].find('\n').map_or(self.text.len(), |at| name.end + at);
        Ok(format!(
            "{}\n{indent}// Custom domains (`ocre domains add`): `ocre deploy` publishes the Worker on\n\
             {indent}// them; Cloudflare creates the DNS record and certificate (zone on this account).\n\
             {indent}domains: {list},{}",
            &self.text[..line_end],
            &self.text[line_end..]
        ))
    }
}

/// `accountId: "<id>",` added right before `worker:` (for `ocre new --account-id`).
pub fn with_account_id(text: &str, account_id: &str) -> String {
    let at = text.find("\tworker: {").expect("the template has a worker entry");
    format!("{}\taccountId: \"{account_id}\",\n{}", &text[..at], &text[at..])
}

/// `assetsDirectory` of wrangler.config.ts, when a string literal.
pub fn assets_directory(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(BUILD_FILE)).ok()?;
    let reader = Reader::new(&text);
    let object = reader.define_argument("defineWranglerConfig")?;
    reader.properties(object).into_iter().find(|(key, _)| key == "assetsDirectory").and_then(|(_, v)| reader.string(v))
}

fn unreadable(key: &str, canonical: &str) -> CliError {
    CliError::new(format!("{FILE} defines `{key}` in a form Ocre cannot read"))
        .hint(format!("write it as a literal: `{canonical}`"))
}

fn push_once(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|known| known == name) {
        names.push(name.to_owned());
    }
}

/// Walks the structure of a TypeScript object literal. `code` is the text
/// with comments blanked and string contents replaced by spaces, so
/// brackets, commas and colons found in it are real syntax; byte offsets
/// are the same in both.
struct Reader<'a> {
    text: &'a str,
    code: Vec<u8>,
}

impl<'a> Reader<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, code: mask(text) }
    }

    /// The `{ ... }` range (braces included) of `worker` in `defineConfig({ worker: { ... } })`.
    fn worker(&self) -> Option<Range<usize>> {
        let config = self.define_argument("defineConfig")?;
        let (_, worker) = self.properties(config).into_iter().find(|(key, _)| key == "worker")?;
        (self.code.get(worker.start) == Some(&b'{')).then_some(worker)
    }

    /// The object literal passed to `<function>(`.
    fn define_argument(&self, function: &str) -> Option<Range<usize>> {
        let mut from = 0;
        while let Some(found) = find(&self.code[from..], function.as_bytes()) {
            let at = from + found;
            from = at + function.len();
            if at > 0 && is_ident(self.code[at - 1]) {
                continue;
            }
            let open = self.skip_space(from);
            if self.code.get(open) != Some(&b'(') {
                continue;
            }
            let brace = self.skip_space(open + 1);
            if self.code.get(brace) == Some(&b'{') {
                return Some(brace..self.closing(brace)? + 1);
            }
        }
        None
    }

    fn skip_space(&self, mut at: usize) -> usize {
        while at < self.code.len() && self.code[at].is_ascii_whitespace() {
            at += 1;
        }
        at
    }

    /// Index of the bracket closing the one at `open`.
    fn closing(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (at, byte) in self.code.iter().enumerate().skip(open) {
            match byte {
                b'{' | b'[' | b'(' => depth += 1,
                b'}' | b']' | b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(at);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Trimmed ranges of the comma-separated members of the bracketed range.
    fn members(&self, outer: Range<usize>) -> Vec<Range<usize>> {
        let mut members = Vec::new();
        let (mut depth, mut start) = (0usize, outer.start + 1);
        for at in outer.start + 1..outer.end - 1 {
            match self.code[at] {
                b'{' | b'[' | b'(' => depth += 1,
                b'}' | b']' | b')' => depth -= 1,
                b',' if depth == 0 => {
                    members.push(start..at);
                    start = at + 1;
                }
                _ => {}
            }
        }
        members.push(start..outer.end - 1);
        members.into_iter().map(|range| self.trim(range)).filter(|range| !range.is_empty()).collect()
    }

    fn trim(&self, mut range: Range<usize>) -> Range<usize> {
        while range.start < range.end && self.code[range.start].is_ascii_whitespace() {
            range.start += 1;
        }
        while range.end > range.start && self.code[range.end - 1].is_ascii_whitespace() {
            range.end -= 1;
        }
        range
    }

    /// `key: value` members of an object literal (others are skipped).
    fn properties(&self, object: Range<usize>) -> Vec<(String, Range<usize>)> {
        self.members(object)
            .into_iter()
            .filter_map(|member| {
                let colon = member.start + self.code[member.clone()].iter().position(|&b| b == b':')?;
                let key = self.text[member.start..colon].trim().trim_matches(|c| c == '"' || c == '\'');
                Some((key.to_owned(), self.trim(colon + 1..member.end)))
            })
            .collect()
    }

    /// A string literal value.
    fn string(&self, value: Range<usize>) -> Option<String> {
        json5::from_str::<Value>(&self.text[value]).ok()?.as_str().map(str::to_owned)
    }

    /// `<namespace>.<kind>(args)`, when `value` is exactly that.
    fn call(&self, value: Range<usize>, namespace: &str) -> Option<(String, Range<usize>)> {
        let code = &self.code[value.clone()];
        let rest = code.strip_prefix(namespace.as_bytes())?;
        let dot = value.start + namespace.len() + rest.iter().position(|b| !b.is_ascii_whitespace())?;
        if self.code[dot] != b'.' {
            return None;
        }
        let kind_start = self.skip_space(dot + 1);
        let kind_len = self.code[kind_start..value.end].iter().take_while(|&&b| is_ident(b)).count();
        let open = self.skip_space(kind_start + kind_len);
        if kind_len == 0 || self.code.get(open) != Some(&b'(') {
            return None;
        }
        let close = self.closing(open)?;
        (close + 1 == value.end).then(|| (self.text[kind_start..kind_start + kind_len].to_owned(), open + 1..close))
    }

    fn parsed(&self, kind: String, key: Option<String>, args: Range<usize>) -> Call {
        let values = json5::from_str::<Vec<Value>>(&format!("[{}]", &self.text[args.clone()])).ok();
        Call { key, kind, args, values }
    }

    fn calls_in_object(&self, object: Range<usize>, namespace: &str) -> Result<Vec<Call>, CliError> {
        if self.code.get(object.start) != Some(&b'{') {
            return Ok(Vec::new());
        }
        let mut calls: Vec<Call> = Vec::new();
        for (key, value) in self.properties(object) {
            let Some((kind, args)) = self.call(value, namespace) else { continue };
            if calls.iter().any(|call| call.key.as_deref() == Some(key.as_str())) {
                return Err(CliError::new(format!("{FILE} defines `{key}` twice"))
                    .hint(format!("keep one `{key}: {namespace}.{kind}(...)` entry")));
            }
            calls.push(self.parsed(kind, Some(key), args));
        }
        Ok(calls)
    }

    fn calls_in_array(&self, array: Range<usize>, namespace: &str) -> Vec<Call> {
        if self.code.get(array.start) != Some(&b'[') {
            return Vec::new();
        }
        self.members(array)
            .into_iter()
            .filter_map(|member| self.call(member, namespace))
            .map(|(kind, args)| self.parsed(kind, None, args))
            .collect()
    }
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// The text with comments blanked and string contents replaced by spaces
/// (quotes kept), byte for byte; newlines are kept.
fn mask(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    out[at] = b' ';
                    at += 1;
                }
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                let end = find(&bytes[at + 2..], b"*/").map_or(bytes.len(), |found| at + 2 + found + 2);
                for byte in &mut out[at..end] {
                    if *byte != b'\n' {
                        *byte = b' ';
                    }
                }
                at = end;
            }
            quote @ (b'"' | b'\'' | b'`') => {
                at += 1;
                while at < bytes.len() && bytes[at] != quote {
                    let escaped = bytes[at] == b'\\';
                    out[at] = b' ';
                    at += 1;
                    if escaped && at < bytes.len() {
                        out[at] = b' ';
                        at += 1;
                    }
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
    out
}

#[cfg(test)]
#[path = "../tests/config.rs"]
mod tests;

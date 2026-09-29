//! `ocre routes`: the app's HTTP routes, read from its Rust source without
//! building it (like `rails routes`).
//!
//! A tolerant scanner, not a Rust parser: it finds `.route("<path>", <chain>)`
//! calls, follows `.merge(<module>::routes())` into `src/<module>.rs` (or
//! `src/<module>/mod.rs`) and knows that `ocre::graphql::routes(...)` serves
//! `GET` and `POST /graphql`. Anything else is skipped.

use std::{collections::HashSet, path::PathBuf};

use serde::Serialize;

use crate::{
    CliResult,
    output::{CliError, Report},
    project::Project,
};

/// HTTP methods in display order; the index sorts routes sharing a path.
const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

#[derive(Serialize, Debug, PartialEq)]
pub struct Route {
    pub method: &'static str,
    pub path: String,
    /// Handler path relative to the crate root, e.g. `posts::index`.
    pub handler: String,
}

/// Lists the routes, keeping those whose method, path or handler contains
/// `filter` (case-insensitive).
pub fn run(project: &Project, filter: Option<&str>) -> CliResult {
    let src = project.root.join("src");
    let lib = std::fs::read_to_string(src.join("lib.rs")).map_err(|err| {
        CliError::new(format!("cannot read src/lib.rs: {err}"))
            .hint("`ocre routes` reads the router built in src/lib.rs; run it inside an Ocre app")
    })?;
    let mut scanner = Scanner { src, seen: HashSet::new(), stack: Vec::new(), routes: Vec::new() };
    scanner.scan(&lib, &[], "");
    let mut routes = scanner.routes;
    let rank = |route: &Route| METHODS.iter().position(|m| *m == route.method);
    routes.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| rank(a).cmp(&rank(b))));
    if let Some(filter) = filter {
        let filter = filter.to_lowercase();
        routes.retain(|route| {
            [route.method, &route.path, &route.handler].iter().any(|text| text.to_lowercase().contains(&filter))
        });
    }
    Ok(Report { routes: Some(routes), ..Report::new("routes") })
}

/// Human form: an aligned `METHOD  PATH  HANDLER` table.
pub fn table(routes: &[Route]) -> String {
    if routes.is_empty() {
        return "No routes.\n".to_owned();
    }
    let method_width = routes.iter().map(|r| r.method.len()).max().unwrap_or(0).max("METHOD".len());
    let path_width = routes.iter().map(|r| r.path.len()).max().unwrap_or(0).max("PATH".len());
    let mut out = format!("{:<method_width$}  {:<path_width$}  HANDLER\n", "METHOD", "PATH");
    for route in routes {
        out.push_str(&format!("{:<method_width$}  {:<path_width$}  {}\n", route.method, route.path, route.handler));
    }
    out
}

struct Scanner {
    src: PathBuf,
    /// Files already scanned under a prefix, so a module merged twice is listed once.
    seen: HashSet<(PathBuf, String)>,
    /// Files being scanned, so merge and nest cycles terminate.
    stack: Vec<PathBuf>,
    routes: Vec<Route>,
}

impl Scanner {
    /// Collects the routes defined in `source`, the file of `module` (empty
    /// for the crate root), mounted under `prefix` (`/admin` when a parent
    /// nests it there, empty otherwise).
    fn scan(&mut self, source: &str, module: &[String], prefix: &str) {
        let code = strip_comments(source);
        for args in calls(&code, ".route(") {
            let [path, chain, ..] = split_top_level(args, ',')[..] else { continue };
            let Some(path) = string_literal(path) else { continue };
            for segment in split_top_level(chain, '.') {
                let Some((method, handler)) = method_call(segment) else { continue };
                let path = join(prefix, path);
                self.routes.push(Route { method, path, handler: qualify(module, handler) });
            }
        }
        for _ in calls(&code, "ocre::graphql::routes(") {
            for (method, handler) in [("GET", "graphiql"), ("POST", "respond")] {
                let handler = format!("ocre::graphql::{handler}");
                self.routes.push(Route { method, path: join(prefix, "/graphql"), handler });
            }
        }
        for args in calls(&code, ".merge(") {
            if let Some(target) = merged_module(args, module) {
                self.follow(target, prefix.to_owned());
            }
        }
        for args in calls(&code, ".nest(") {
            let [path, target] = split_top_level(args, ',')[..] else { continue };
            if let (Some(path), Some(target)) = (string_literal(path), merged_module(target, module)) {
                self.follow(target, join(prefix, path));
            }
        }
    }

    fn follow(&mut self, module: Vec<String>, prefix: String) {
        let base = module.iter().fold(self.src.clone(), |dir, segment| dir.join(segment));
        let candidates = [base.with_extension("rs"), base.join("mod.rs")];
        let Some((file, source)) =
            candidates.into_iter().find_map(|file| std::fs::read_to_string(&file).ok().map(|text| (file, text)))
        else {
            return;
        };
        if !self.stack.contains(&file) && self.seen.insert((file.clone(), prefix.clone())) {
            self.stack.push(file);
            self.scan(&source, &module, &prefix);
            self.stack.pop();
        }
    }
}

/// `"/posts"` -> `/posts`; `None` for anything but a string literal.
fn string_literal(text: &str) -> Option<&str> {
    text.trim().strip_prefix('"')?.strip_suffix('"')
}

/// `/admin` + `/posts` -> `/admin/posts`; axum's `nest` serves the nested
/// router's `/` at the prefix itself.
fn join(prefix: &str, path: &str) -> String {
    match (prefix, path) {
        ("", _) => path.to_owned(),
        (_, "/") => prefix.to_owned(),
        _ => format!("{prefix}{path}"),
    }
}

/// `<module>::routes()` relative to `module`, or from the crate root with
/// `crate::`, as an absolute module path.
fn merged_module(args: &str, module: &[String]) -> Option<Vec<String>> {
    let path = args.trim().strip_suffix("::routes()")?;
    let (mut target, path) = match path.strip_prefix("crate::") {
        Some(rest) => (Vec::new(), rest),
        None => (module.to_vec(), path),
    };
    for segment in path.split("::") {
        if !is_identifier(segment) || segment == "super" || segment == "self" {
            return None;
        }
        target.push(segment.to_owned());
    }
    Some(target)
}

/// `get(index)` -> `("GET", "index")`; `None` for other calls or handlers
/// that are not plain paths (closures...).
fn method_call(segment: &str) -> Option<(&'static str, &str)> {
    let (name, rest) = segment.trim().split_once('(')?;
    let name = name.rsplit("::").next().unwrap_or(name).trim();
    let method = METHODS.iter().find(|m| m.eq_ignore_ascii_case(name))?;
    let handler = rest.strip_suffix(')')?.trim();
    handler.split("::").all(is_identifier).then_some((*method, handler))
}

fn qualify(module: &[String], handler: &str) -> String {
    match handler.strip_prefix("crate::") {
        Some(absolute) => absolute.to_owned(),
        None if module.is_empty() => handler.to_owned(),
        None => format!("{}::{handler}", module.join("::")),
    }
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The argument text of every `<opener>...)` call in `code`.
fn calls<'a>(code: &'a str, opener: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    code.match_indices(opener).filter_map(move |(start, _)| {
        let args = &code[start + opener.len()..];
        let mut depth = 1;
        let mut in_string = false;
        let mut escaped = false;
        for (i, c) in args.char_indices() {
            match (in_string, c) {
                (true, _) if escaped => escaped = false,
                (true, '\\') => escaped = true,
                (_, '"') => in_string = !in_string,
                (false, '(' | '[' | '{') => depth += 1,
                (false, ')' | ']' | '}') => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&args[..i]);
                    }
                }
                _ => {}
            }
        }
        None
    })
}

/// Splits on `separator` outside brackets and string literals.
fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start, mut in_string, mut escaped) = (0, 0, false, false);
    for (i, c) in text.char_indices() {
        match (in_string, c) {
            (true, _) if escaped => escaped = false,
            (true, '\\') => escaped = true,
            (_, '"') => in_string = !in_string,
            (false, '(' | '[' | '{') => depth += 1,
            (false, ')' | ']' | '}') => depth -= 1,
            (false, c) if c == separator && depth == 0 => {
                parts.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Removes `//` and `/* */` comments, keeping string and char literals.
fn strip_comments(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < chars.len() {
        match (chars[i], chars.get(i + 1)) {
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            ('"', _) => {
                out.push('"');
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        out.push('\\');
                        i += 1;
                    }
                    if let Some(&c) = chars.get(i) {
                        out.push(c);
                    }
                    i += 1;
                }
                out.push('"');
                i += 1;
            }
            // Char literals such as '"' or '\'' (a lone `'` is a lifetime).
            ('\'', Some('\\')) => {
                i += 3;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
                i += 1;
            }
            ('\'', Some(_)) if chars.get(i + 2) == Some(&'\'') => i += 3,
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

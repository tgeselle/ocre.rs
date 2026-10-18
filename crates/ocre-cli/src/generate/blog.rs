//! `ocre generate blog`: Markdown articles in `content/blog/<slug>/<locale>.md`,
//! compiled into the Worker by `build.rs` (pulldown-cmark, a build
//! dependency: no Markdown parser in the WebAssembly), served at `/blog` and
//! `/blog/<slug>` by `src/blog.rs`, in every locale of the app.

use super::{Edits, register_routes};
use crate::{CliResult, i18n::declared, output::CliError, project::Project};

const BUILD_RS: &str = include_str!("../../templates/blog/build.rs");
const BLOG_RS: &str = include_str!("../../templates/blog/blog.rs");
const INDEX_HTML: &str = include_str!("../../templates/blog/index.html");
const SHOW_HTML: &str = include_str!("../../templates/blog/show.html");
const README: &str = include_str!("../../templates/blog/README.md");

/// The build dependency compiling the Markdown.
const PULLDOWN_CMARK: &str = "pulldown-cmark = { version = \"0.13\", default-features = false, features = [\"html\"] }";

/// How the app's URLs carry the locale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Urls {
    /// No translations: `/blog`.
    Plain,
    /// `/en/blog`, `/fr/blog` (routes nested under `/{locale}`).
    Prefixed,
    /// `/blog`, `/fr/blog` (`unprefixed_default`).
    UnprefixedDefault,
}

pub fn blog(project: &Project) -> CliResult {
    if project.api_only {
        return Err(CliError::new("a blog needs HTML pages; this app is API-only")
            .hint("serve the articles from the front end that uses this API"));
    }
    let mut edits = Edits::new(project);
    let lib = edits.read("src/lib.rs")?.unwrap_or_default();
    let codes = declared(&lib).unwrap_or_default();
    let urls = match codes.first() {
        None => Urls::Plain,
        Some(_) if lib.contains("unprefixed_default") => Urls::UnprefixedDefault,
        Some(_) => Urls::Prefixed,
    };
    let codes = if codes.is_empty() { vec!["en".to_owned()] } else { codes };
    let default = codes[0].as_str();

    edits.create("build.rs", BUILD_RS.replace("__DEFAULT_LOCALE__", default))?;
    edits.create("content/blog/README.md", README.replace("__DEFAULT_LOCALE__", default))?;
    for code in &codes {
        edits.create(&format!("content/blog/welcome/{code}.md"), sample(code == default))?;
    }
    edits.create("src/blog.rs", blog_rs(urls))?;
    edits.create("templates/blog/index.html", INDEX_HTML.to_owned())?;
    edits.create("templates/blog/show.html", SHOW_HTML.to_owned())?;
    let cargo = edits.read("Cargo.toml")?.unwrap_or_default();
    edits.update("Cargo.toml", with_build_dependency(&cargo, PULLDOWN_CMARK));
    register_routes(&mut edits, "blog")?;

    let mut report = edits.apply("generate blog")?;
    let index = match urls {
        Urls::Prefixed => format!("http://localhost:8787/{default}/blog"),
        Urls::Plain | Urls::UnprefixedDefault => "http://localhost:8787/blog".to_owned(),
    };
    report.next = vec![
        "write articles in content/blog/<slug>/<locale>.md (format: content/blog/README.md)".to_owned(),
        format!("ocre dev, then open {index}"),
        "list `blog::pages()` in the sitemap of src/seo.rs (`ocre g seo`)".to_owned(),
    ];
    Ok(report)
}

/// src/blog.rs for the app's kind of URLs.
fn blog_rs(urls: Urls) -> String {
    let localized = urls != Urls::Plain;
    let replacements = [
        ("__LOCALE_URLS__", if localized { " in every locale" } else { "" }),
        ("__WITHOUT_LOCALE__", if localized { ", without locale" } else { "" }),
        ("__I18N_IMPORT__", if localized { "use ocre::i18n::I18n;\n" } else { "" }),
        ("__HANDLER_ARGS__", if localized { "i18n: I18n" } else { "" }),
        ("__SHOW_ARGS__", if localized { ", i18n: I18n" } else { "" }),
        ("__LANG_DIR__", if localized { "(i18n.locale(), i18n.dir())" } else { "(POST_LOCALES[0], \"ltr\")" }),
        ("__LINK__", if localized { "|path: &str| i18n.path(path)" } else { "|path: &str| path.to_owned()" }),
        (
            "__ROUTES__",
            match urls {
                Urls::Plain => "pages",
                Urls::Prefixed => {
                    "// `/<locale>/blog`, like the app's other localized pages.\n    Router::new().nest(\"/{locale}\", pages)"
                }
                Urls::UnprefixedDefault => {
                    "// `/blog` in the default locale, `/<locale>/blog` in the others.\n    ocre::i18n::localized(pages)"
                }
            },
        ),
    ];
    replacements.iter().fold(BLOG_RS.to_owned(), |text, (from, to)| text.replace(from, to))
}

/// The first article: its dates live in the default locale's file.
fn sample(default: bool) -> String {
    let dates = if default {
        let (year, month, day) = crate::pg_import::civil_from_days(now_days());
        format!("published: {year:04}-{month:02}-{day:02}\n")
    } else {
        String::new()
    };
    let note = if default { "" } else { "\n\nTranslate this file from the default locale's." };
    format!(
        "---\ntitle: Welcome to the blog\ndescription: The first article, to replace with your own.\n{dates}---\n\
         Articles are Markdown files in content/blog/, compiled into the app when it builds.{note}\n\n\
         ## Writing an article\n\nCreate `content/blog/<slug>/<locale>.md` for each locale; the format is in \
         content/blog/README.md.\n"
    )
}

fn now_days() -> i64 {
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    i64::try_from(seconds / 86_400).unwrap_or_default()
}

/// Cargo.toml with `line` under `[build-dependencies]` (the section added at the end when missing); unchanged when present.
fn with_build_dependency(cargo_toml: &str, line: &str) -> String {
    let name = line.split_whitespace().next().unwrap_or_default();
    if cargo_toml.lines().any(|existing| existing.split_whitespace().next() == Some(name)) {
        return cargo_toml.to_owned();
    }
    match cargo_toml.find("[build-dependencies]\n") {
        Some(start) => {
            let at = start + "[build-dependencies]\n".len();
            format!("{}{line}\n{}", &cargo_toml[..at], &cargo_toml[at..])
        }
        None => {
            let text = cargo_toml.trim_end();
            format!(
                "{text}\n\n# The blog's Markdown becomes HTML at build time (build.rs), never in the Worker.\n[build-dependencies]\n{line}\n"
            )
        }
    }
}

#[cfg(test)]
#[path = "../../tests/generate/blog.rs"]
mod tests;

//! Documentation site tooling. Run from the repository root:
//!
//! - `cargo docs-site build`: HTML site (mdBook) plus a raw Markdown twin of
//!   every page, `llms.txt`, `llms-full.txt`, `api-index.md` and rustdoc under
//!   `/api/`, all in `docs/book/`.
//! - `cargo docs-site serve [wrangler args]`: build, then `wrangler dev` on
//!   the Workers Static Assets config in `docs/wrangler.toml`.
//! - `cargo docs-site deploy`: build, then `wrangler deploy`.
//! - `cargo docs-site check`: compile every Rust block marked `rust,check`
//!   inside an app made by the real `ocre` CLI (see [`FIXTURE`]).
//!
//! `OCRE_DOCS_URL` overrides the base URL written into `llms.txt` and
//! `llms-full.txt` (default [`DEFAULT_BASE_URL`]).

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};

/// Where the site is published. Also written in the generated apps' AGENTS.md
/// (`crates/ocre-cli/templates/new/AGENTS.md`): change both together.
const DEFAULT_BASE_URL: &str = "https://ocre.rs";
/// The mdBook release the site is built and tested with.
const MDBOOK_VERSION: &str = "0.5.4";

/// Generators run on `ocre new docs-app --starter blog` (which already has a
/// `Post` scaffold: title, body, published) before the examples are compiled.
/// Checked examples may use everything these create; pages say which command
/// produced the code they build on.
const FIXTURE: &[&[&str]] = &[
    &["g", "scaffold", "Comment", "author:string", "body:text", "post:references"],
    &["g", "auth"],
    &["g", "model", "Author", "name:string^", "bio:text?"],
    &["g", "api", "Product", "name:string^", "price:float", "stock:integer?", "--graphql"],
    &["g", "scaffold", "Message", "body:text", "--realtime"],
    &["g", "scaffold", "Photo", "title:string", "image:attachment", "notes:attachment?"],
    &["g", "api", "Document", "name:string", "file:attachment?"],
    &["g", "mailer", "User", "welcome", "password_reset"],
    &["g", "mailbox"],
    &["g", "job", "SendWelcome", "user_id:integer"],
    &["g", "schedule", "nightly_cleanup", "0 3 * * *"],
    &["g", "cache"],
    &["g", "locale", "en", "fr"],
];

const USAGE: &str = "usage: cargo docs-site <build|serve|deploy|check> (from the repository root)
  build              site into docs/book/
  serve [args...]    build, then `wrangler dev` (extra args go to wrangler, e.g. --port 8788)
  deploy             build, then `wrangler deploy` (needs `npx wrangler login`)
  check [pages...]   compile the `rust,check` examples (of these docs/src pages) in a generated app";

type Res<T> = Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("build") => build().map(|out| println!("Built {}", out.display())),
        Some("serve") => build().and_then(|_| wrangler("dev", &args[1..])),
        Some("deploy") => build().and_then(|_| wrangler("deploy", &args[1..])),
        Some("check") => check(&args[1..]),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repository root")
}

fn base_url() -> String {
    env::var("OCRE_DOCS_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()).trim_end_matches('/').to_string()
}

// ---------------------------------------------------------------- build

fn build() -> Res<PathBuf> {
    let repo = repo();
    let docs = repo.join("docs");
    let src = docs.join("src");
    let out = docs.join("book");

    // The API index is generated elsewhere (scripts/api-index.py) into
    // docs/api-index.md; the book renders a copy (git-ignored).
    let api_index = docs.join("api-index.md");
    let api_index =
        read(&api_index).map_err(|e| format!("{e}\nhint: generate it with `python3 scripts/api-index.py`"))?;
    write(&src.join("api-index.md"), &api_index)?;

    let pages = summary(&src)?;
    check_links(&pages, &src)?;
    mdbook(&docs)?;

    for page in &pages {
        write(&out.join(&page.path), &read(&src.join(&page.path))?)?;
    }
    let base = base_url();
    write(&out.join("llms.txt"), &llms_txt(&pages, &src, &base)?)?;
    write(&out.join("llms-full.txt"), &llms_full(&pages, &src, &base)?)?;
    rustdoc(&repo, &out.join("api"))?;
    Ok(out)
}

fn mdbook(docs: &Path) -> Res<()> {
    let version = Command::new("mdbook").arg("--version").output().map_err(|_| {
        format!("mdbook is not installed\nhint: cargo install mdbook --version {MDBOOK_VERSION} --locked")
    })?;
    let version = String::from_utf8_lossy(&version.stdout);
    if !version.contains(MDBOOK_VERSION) {
        eprintln!("warning: the site is tested with mdbook {MDBOOK_VERSION}, found {}", version.trim());
    }
    run(Command::new("mdbook").arg("build").arg(docs))
}

/// rustdoc for the framework with every feature, under `<out>/api/`. A
/// separate target directory keeps other crates' docs out of the site.
fn rustdoc(repo: &Path, out: &Path) -> Res<()> {
    let target = repo.join("target/docs-api");
    run(Command::new("cargo")
        .args(["doc", "-q", "-p", "ocre", "--all-features", "--no-deps", "--target-dir"])
        .arg(&target)
        .current_dir(repo))?;
    if out.exists() {
        fs::remove_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    copy_dir(&target.join("doc"), out)?;
    write(
        &out.join("index.html"),
        "<!DOCTYPE html>\n<meta charset=\"utf-8\">\n<title>Ocre API</title>\n\
         <meta http-equiv=\"refresh\" content=\"0; url=ocre/index.html\">\n\
         <a href=\"ocre/index.html\">Ocre API reference</a>\n",
    )
}

fn wrangler(command: &str, args: &[String]) -> Res<()> {
    let repo = repo();
    run(Command::new("npx")
        .args(["--yes", "wrangler@4", command, "--config", "docs/wrangler.toml"])
        .args(args)
        .current_dir(repo))
}

// ---------------------------------------------------------------- pages

struct Page {
    /// Part of the book (`# Guides` in SUMMARY.md); empty before the first part.
    section: String,
    title: String,
    /// Relative to `docs/src`, e.g. `guides/models.md`.
    path: String,
}

/// Pages in reading order, from `SUMMARY.md`.
fn summary(src: &Path) -> Res<Vec<Page>> {
    let text = read(&src.join("SUMMARY.md"))?;
    let mut pages = Vec::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(title) = line.strip_prefix("# ") {
            if title != "Summary" {
                section = title.to_string();
            }
        } else if let Some((title, path)) = summary_link(line) {
            pages.push(Page { section: section.clone(), title: title.to_string(), path: path.to_string() });
        }
    }
    Ok(pages)
}

/// `- [Title](path.md)` or `[Title](path.md)`.
fn summary_link(line: &str) -> Option<(&str, &str)> {
    let line = line.strip_prefix("- ").unwrap_or(line);
    let rest = line.strip_prefix('[')?;
    let (title, rest) = rest.split_once("](")?;
    let path = rest.strip_suffix(')')?;
    (!path.is_empty()).then_some((title, path))
}

/// The first paragraph after the page's `# Title`: every page starts with a
/// one or two sentence summary, used as its description in `llms.txt`.
fn description(markdown: &str, path: &str) -> Res<String> {
    let mut lines = markdown.lines().skip_while(|l| !l.starts_with("# ")).skip(1).skip_while(|l| l.trim().is_empty());
    let first = lines.next().unwrap_or("");
    if first.is_empty() || first.starts_with(['#', '`', '|', '-', '<', '!', '>']) {
        return Err(format!("docs/src/{path}: the line after `# Title` must start a one-paragraph summary"));
    }
    let mut text = first.trim().to_string();
    for line in lines.take_while(|l| !l.trim().is_empty()) {
        text.push(' ');
        text.push_str(line.trim());
    }
    Ok(text)
}

fn llms_txt(pages: &[Page], src: &Path, base: &str) -> Res<String> {
    let intro = pages.first().ok_or("SUMMARY.md lists no page")?;
    let mut out = String::new();
    writeln!(out, "# Ocre\n").unwrap();
    let summary = description(&read(&src.join(&intro.path))?, &intro.path)?;
    writeln!(out, "> {}\n", absolute_links(&summary, &intro.path, base)).unwrap();
    writeln!(
        out,
        "Every page below is Markdown; the HTML version is the same URL without `.md`. \
         Each page is self-contained: prerequisites, complete code, commands and expected output. \
         All pages in one file: {base}/llms-full.txt. \
         The generated app's AGENTS.md holds the conventions an agent must follow."
    )
    .unwrap();
    let mut current = None;
    for page in pages.iter().skip(1) {
        if current != Some(&page.section) {
            let name = if page.section.is_empty() { "Overview" } else { &page.section };
            writeln!(out, "\n## {name}\n").unwrap();
            current = Some(&page.section);
        }
        let description = description(&read(&src.join(&page.path))?, &page.path)?;
        let description = absolute_links(&description, &page.path, base);
        writeln!(out, "- [{}]({base}/{}): {description}", page.title, page.path).unwrap();
    }
    writeln!(out, "\n## Optional\n").unwrap();
    writeln!(
        out,
        "- [Rustdoc API reference]({base}/api/ocre/): every public item of the `ocre` crate \
         (all features), generated by `cargo doc`; HTML only, prefer api-index.md."
    )
    .unwrap();
    Ok(out)
}

fn llms_full(pages: &[Page], src: &Path, base: &str) -> Res<String> {
    let mut out = String::from(
        "# Ocre documentation (all pages)\n\n\
         > Every page of the Ocre documentation in reading order. Each page starts with its URL; \
         links point to the Markdown version of pages.\n",
    );
    for page in pages {
        let markdown = read(&src.join(&page.path))?;
        write!(out, "\n---\n\nURL: {base}/{}\n\n{}", page.path, absolute_links(&markdown, &page.path, base)).unwrap();
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

/// Rewrites relative Markdown links (`](../guides/models.md#x)`) to absolute
/// URLs, so pages still link correctly once concatenated.
fn absolute_links(markdown: &str, path: &str, base: &str) -> String {
    let dir = Path::new(path).parent().unwrap_or(Path::new(""));
    map_links(markdown, |target| absolute(dir, target, path, base))
}

/// Replaces the target of every inline link (`[text](target)`) outside code
/// blocks and code spans with `f(target)`.
fn map_links(markdown: &str, mut f: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut fenced = false;
    for line in markdown.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if fenced {
            out.push_str(line);
            continue;
        }
        let mut in_code = false;
        let mut rest = line;
        while let Some(i) = rest.find(['`', ']']) {
            let (before, at) = rest.split_at(i);
            out.push_str(before);
            // `at` starts with '`' or ']', one byte each.
            let (mark, after) = at.split_at(1);
            rest = after;
            if mark == "`" {
                in_code = !in_code;
                out.push('`');
            } else if let Some((target, tail)) =
                after.strip_prefix('(').filter(|_| !in_code).and_then(|t| t.split_once(')'))
            {
                out.push_str("](");
                out.push_str(&f(target));
                out.push(')');
                rest = tail;
            } else {
                out.push(']');
            }
        }
        out.push_str(rest);
    }
    out
}

/// Fails on links to pages missing from SUMMARY.md or to headings that do
/// not exist (mdBook would render them as dead links).
fn check_links(pages: &[Page], src: &Path) -> Res<()> {
    let mut ids = std::collections::HashMap::new();
    let mut texts = Vec::new();
    for page in pages {
        let markdown = read(&src.join(&page.path))?;
        ids.insert(page.path.clone(), heading_ids(&markdown));
        texts.push((page, markdown));
    }
    let mut broken = Vec::new();
    for (page, markdown) in &texts {
        let dir = Path::new(&page.path).parent().unwrap_or(Path::new(""));
        map_links(markdown, |target| {
            let resolved = absolute(dir, target, &page.path, "");
            let resolved = resolved.trim_start_matches('/');
            let (file, fragment) = resolved.split_once('#').unwrap_or((resolved, ""));
            if target.contains("://") || target.starts_with("mailto:") || !file.ends_with(".md") {
                return String::new();
            }
            match ids.get(file) {
                None => broken.push(format!("docs/src/{}: {target} (no such page)", page.path)),
                Some(headings) if !fragment.is_empty() && !headings.iter().any(|id| id == fragment) => {
                    broken.push(format!("docs/src/{}: {target} (no such heading)", page.path))
                }
                Some(_) => {}
            }
            String::new()
        });
    }
    if broken.is_empty() { Ok(()) } else { Err(format!("broken links:\n  {}", broken.join("\n  "))) }
}

/// Heading anchors as mdBook 0.5 makes them: text lowercased, spaces to `-`,
/// other punctuation dropped, runs of `-` collapsed, `-1`, `-2`... on repeats.
fn heading_ids(markdown: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut seen = std::collections::HashMap::<String, usize>::new();
    let mut fenced = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let Some(text) = line.strip_prefix('#').filter(|_| !fenced).map(|t| t.trim_start_matches('#')) else {
            continue;
        };
        let text = map_links(text.trim(), |_| String::new()).replace("]()", "");
        let id: String = text
            .chars()
            .filter_map(|c| match c {
                c if c.is_alphanumeric() || c == '_' || c == '-' => Some(c.to_ascii_lowercase()),
                c if c.is_whitespace() => Some('-'),
                _ => None,
            })
            .collect();
        let id = id.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");
        let count = seen.entry(id.clone()).or_insert(0);
        ids.push(if *count == 0 { id } else { format!("{id}-{count}") });
        *count += 1;
    }
    ids
}

fn absolute(dir: &Path, target: &str, path: &str, base: &str) -> String {
    if target.contains("://") || target.starts_with("mailto:") {
        return target.to_string();
    }
    if let Some(fragment) = target.strip_prefix('#') {
        return format!("{base}/{path}#{fragment}");
    }
    if let Some(root) = target.strip_prefix('/') {
        return format!("{base}/{root}");
    }
    let mut parts: Vec<String> = Vec::new();
    for component in dir.join(target).components() {
        match component {
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => {}
        }
    }
    let joined = parts.join("/");
    if target.ends_with('/') { format!("{base}/{joined}/") } else { format!("{base}/{joined}") }
}

// ---------------------------------------------------------------- check

struct Example {
    /// `docs/src/<page>:<line>` of the opening fence.
    origin: String,
    code: String,
}

/// Rust blocks whose info string has the `check` attribute (```` ```rust,check ````),
/// and the number of other Rust blocks.
fn examples(markdown: &str, path: &str) -> (Vec<Example>, usize) {
    enum Fence {
        Outside,
        Other,
        Checked(Example, usize),
    }
    let mut found = Vec::new();
    let mut unchecked = 0;
    let mut fence = Fence::Outside;
    for (number, line) in markdown.lines().enumerate() {
        let trimmed = line.trim_start();
        let marker = trimmed.starts_with("```");
        fence = match fence {
            Fence::Outside if marker => {
                let mut attributes = trimmed[3..].trim().split([',', ' ']);
                let rust = attributes.next() == Some("rust");
                if rust && attributes.any(|a| a == "check") {
                    let origin = format!("docs/src/{path}:{}", number + 1);
                    Fence::Checked(Example { origin, code: String::new() }, line.len() - trimmed.len())
                } else {
                    unchecked += usize::from(rust);
                    Fence::Other
                }
            }
            Fence::Outside => Fence::Outside,
            Fence::Other if marker => Fence::Outside,
            Fence::Other => Fence::Other,
            Fence::Checked(example, _) if marker => {
                found.push(example);
                Fence::Outside
            }
            Fence::Checked(mut example, indent) => {
                example.code.push_str(line.get(indent..).unwrap_or(trimmed));
                example.code.push('\n');
                Fence::Checked(example, indent)
            }
        };
    }
    (found, unchecked)
}

/// `pages`: only these (paths relative to `docs/src`, e.g. `guides/caching.md`);
/// all pages when empty. `OCRE_DOCS_CHECK_APP` names the app directory under
/// `target/docs-examples/` (default `docs-app`), so that checks can run side by
/// side; they share one Cargo target directory.
fn check(pages: &[String]) -> Res<()> {
    let repo = repo();
    let src = repo.join("docs/src");
    let known = summary(&src)?;
    if let Some(missing) = pages.iter().find(|p| !known.iter().any(|k| &k.path == *p)) {
        return Err(format!("{missing} is not in docs/src/SUMMARY.md"));
    }
    let mut all = Vec::new();
    let mut unchecked = 0;
    for page in known.iter().filter(|k| pages.is_empty() || pages.contains(&k.path)) {
        let markdown = read(&src.join(&page.path))?;
        let (found, skipped) = examples(&markdown, &page.path);
        let slug: String = page
            .path
            .trim_end_matches(".md")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        all.extend(found.into_iter().enumerate().map(|(i, example)| (format!("{slug}_{}", i + 1), example)));
        unchecked += skipped;
    }

    println!("Building the ocre CLI...");
    run(Command::new("cargo").args(["build", "-q", "-p", "ocre-cli"]).current_dir(&repo))?;
    let ocre = repo.join("target/debug/ocre");
    let work = repo.join("target/docs-examples");
    let name = env::var("OCRE_DOCS_CHECK_APP").unwrap_or_else(|_| "docs-app".to_string());
    let app = work.join(&name);
    if app.exists() {
        fs::remove_dir_all(&app).map_err(|e| format!("{}: {e}", app.display()))?;
    }
    fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    println!("Generating {} (ocre new --starter blog, then {} generators)...", app.display(), FIXTURE.len());
    quiet(
        Command::new(&ocre)
            .args(["new", &name, "--starter", "blog", "--yes", "--json", "--ocre-path"])
            .arg(repo.join("crates/ocre"))
            .current_dir(&work),
    )?;
    for args in FIXTURE {
        quiet(Command::new(&ocre).args(*args).arg("--json").current_dir(&app))?;
    }

    let dir = app.join("src/doc_examples");
    let mut modules =
        String::from("//! Examples from docs/src, compiled by `cargo docs-site check`.\n#![allow(dead_code)]\n\n");
    for (module, example) in &all {
        writeln!(modules, "mod {module}; // {}", example.origin).unwrap();
        write(&dir.join(format!("{module}.rs")), &format!("// {}\n{}", example.origin, example.code))?;
    }
    write(&dir.join("mod.rs"), &modules)?;
    let lib_path = app.join("src/lib.rs");
    let lib = read(&lib_path)?;
    let lib = lib.replacen("// ocre:modules\n", "// ocre:modules\n#[deny(warnings)]\nmod doc_examples;\n", 1);
    write(&lib_path, &lib)?;

    println!("Checking {} examples (cargo check --target wasm32-unknown-unknown)...", all.len());
    let status = Command::new("cargo")
        .args(["check", "-q", "--target", "wasm32-unknown-unknown", "--message-format", "short"])
        .env("CARGO_TARGET_DIR", work.join("target"))
        .current_dir(&app)
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err(format!(
            "examples do not compile; src/doc_examples/<page>_<n>.rs is the n-th `rust,check` block of \
             docs/src/<page>.md (first line: its source line). App: {}",
            app.display()
        ));
    }
    println!("{} examples compile; {unchecked} Rust blocks are not marked `check` (fragments).", all.len());
    Ok(())
}

// ---------------------------------------------------------------- helpers

fn read(path: &Path) -> Res<String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, content: &str) -> Res<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))
}

fn copy_dir(from: &Path, to: &Path) -> Res<()> {
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name == ".lock" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(&name))?;
        } else {
            fs::copy(&path, to.join(&name)).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn run(command: &mut Command) -> Res<()> {
    let status = command.status().map_err(|e| format!("{:?}: {e}", command.get_program()))?;
    status.success().then_some(()).ok_or_else(|| format!("{command:?} failed ({status})"))
}

/// Runs a command, showing its output only when it fails.
fn quiet(command: &mut Command) -> Res<()> {
    let output = command.output().map_err(|e| format!("{:?}: {e}", command.get_program()))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{command:?} failed ({}):\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

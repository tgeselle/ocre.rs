//! `ocre i18n missing`, and the locale-file check `ocre dev` and `ocre
//! deploy` run first. Files are read with the same parser the Worker uses
//! (`ocre::i18n::check`), so what passes here loads there.

use std::path::Path;

use ocre::i18n::Problem;

use crate::{
    CliResult,
    output::{CliError, Report},
    project::Project,
};

/// Start of the declaration in src/lib.rs: `ocre::locales!("en", "fr")`.
pub const LOCALES_MACRO: &str = "ocre::locales!(";

/// The codes declared in src/lib.rs, default first; `None` without a declaration.
pub fn declared(lib: &str) -> Option<Vec<String>> {
    let start = lib.find(LOCALES_MACRO)? + LOCALES_MACRO.len();
    let args = &lib[start..start + lib[start..].find(')')?];
    Some(args.split('"').skip(1).step_by(2).map(str::to_owned).collect())
}

/// The locale files of an app.
struct LocaleFiles {
    /// (code, file contents) for every declared locale that has a file, default first.
    sources: Vec<(String, String)>,
    /// Declared in src/lib.rs, but the file does not exist (a compile error).
    absent: Vec<String>,
    /// Files in locales/ that src/lib.rs does not declare (never loaded).
    undeclared: Vec<String>,
}

/// `None` when the app has no locales.
fn read(root: &Path) -> Result<Option<LocaleFiles>, CliError> {
    let lib = std::fs::read_to_string(root.join("src/lib.rs"))?;
    let Some(codes) = declared(&lib) else { return Ok(None) };
    let mut files =
        LocaleFiles { sources: Vec::with_capacity(codes.len()), absent: Vec::new(), undeclared: Vec::new() };
    for code in &codes {
        match std::fs::read_to_string(root.join(format!("locales/{code}.yml"))) {
            Ok(text) => files.sources.push((code.clone(), text)),
            Err(_) => files.absent.push(format!("locales/{code}.yml: declared in src/lib.rs but missing")),
        }
    }
    let mut present: Vec<String> = match std::fs::read_dir(root.join("locales")) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.strip_suffix(".yml").map(str::to_owned))
            .collect(),
        Err(_) => Vec::new(),
    };
    present.sort();
    for file in present.into_iter().filter(|file| !codes.contains(file)) {
        files
            .undeclared
            .push(format!("locales/{file}.yml: not declared; add \"{file}\" to {LOCALES_MACRO}...) in src/lib.rs"));
    }
    Ok(Some(files))
}

fn check(sources: &[(String, String)]) -> Vec<Problem> {
    let borrowed: Vec<(&str, &str)> = sources.iter().map(|(code, text)| (code.as_str(), text.as_str())).collect();
    ocre::i18n::check(&borrowed)
}

/// Fails on locale files the Worker could not load (syntax errors, a
/// declared file that does not exist). Missing keys are not errors here.
pub fn check_syntax(root: &Path) -> Result<(), CliError> {
    let Some(LocaleFiles { sources, mut absent, .. }) = read(root)? else { return Ok(()) };
    absent.extend(
        check(&sources).into_iter().filter(|problem| matches!(problem, Problem::Invalid { .. })).map(|p| p.to_string()),
    );
    if absent.is_empty() {
        return Ok(());
    }
    Err(CliError::new(format!("invalid locale files:\n  {}", absent.join("\n  "))).hint(
        "fix each line named above (quote values with \"...\" when in doubt); `ocre i18n missing` checks them again",
    ))
}

/// `ocre i18n missing`: every key of the default locale must exist in every
/// other locale, with the plural forms its language needs.
pub fn missing(project: &Project) -> CliResult {
    let LocaleFiles { sources, absent, undeclared } = read(&project.root)?.ok_or_else(|| {
        CliError::new(format!("src/lib.rs declares no locales ({LOCALES_MACRO}...) not found)"))
            .hint("run `ocre g locale en` to set up translations, then `ocre g locale fr` for each other language")
    })?;
    let mut problems = absent;
    problems.extend(undeclared);
    problems.extend(check(&sources).iter().map(ToString::to_string));
    let codes: Vec<&str> = sources.iter().map(|(code, _)| code.as_str()).collect();
    if problems.is_empty() {
        let default = codes.first().copied().unwrap_or_default();
        let summary = format!("every locale ({}) has every key of locales/{default}.yml", codes.join(", "));
        return Ok(Report { ran: vec![summary], ..Report::new("i18n missing") });
    }
    Err(CliError::new(format!("{} locale problems:\n  {}", problems.len(), problems.join("\n  "))).hint(format!(
        "add each missing key at the same path as in locales/{}.yml, translated (plural keys need the forms listed); \
         then run `ocre i18n missing` again",
        codes.first().copied().unwrap_or("en")
    )))
}

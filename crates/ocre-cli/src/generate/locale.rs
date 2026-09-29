//! `ocre generate locale <code>...`: `locales/<code>.yml` for each code,
//! declared in `ocre::locales!(...)` in src/lib.rs. The first run sets up
//! translations: the first code becomes the default locale, and `routes()`
//! gets the layer the `I18n` extractor needs.

use super::{Edits, ROUTES_MARKER};
use crate::{
    CliResult,
    i18n::{LOCALES_MACRO, declared},
    output::CliError,
    project::Project,
};

const LAYER: &str = ".layer(ocre::i18n::layer(&LOCALES))";

fn locales_static(code: &str) -> String {
    format!(
        "\n/// Translations in locales/<code>.yml, compiled into the Worker; the first code\n\
         /// is the default locale. `ocre g locale <code>` adds one.\n\
         static LOCALES: ocre::i18n::Locales = {LOCALES_MACRO}\"{code}\");\n"
    )
}

fn default_file(code: &str) -> String {
    format!(
        "# Default locale ({code}). Nested keys, used as i18n.t(\"app.welcome\") in handlers\n\
         # and {{{{ i18n.t(\"app.welcome\") }}}} in templates. Quote values that start with\n\
         # `%` or contain `: `, e.g. \"%{{count}} posts\"; `%{{name}}` takes .arg(\"name\", value).\n\
         # Plurals: a key with one/other children (plus few/many... for some languages),\n\
         # picked by .count(n). `ocre i18n missing` checks the other locales against this file.\n\
         {code}:\n  app:\n    welcome: \"Welcome\"\n"
    )
}

fn other_file(code: &str, default: &str) -> String {
    format!(
        "# Locale {code}. Translate every key of locales/{default}.yml at the same path;\n\
         # `ocre i18n missing` lists the keys still to add.\n\
         {code}:\n"
    )
}

/// BCP 47-style codes: `en`, `fr`, `pt-BR`, `zh-Hant`.
fn valid_code(code: &str) -> bool {
    let mut parts = code.split('-');
    let language = parts.next().unwrap_or_default();
    (2..=3).contains(&language.len())
        && language.chars().all(|c| c.is_ascii_lowercase())
        && parts.all(|part| (2..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Adds the layer as the last call of the `Router` chain that holds the
/// `// ocre:routes` marker (just before the closing `}` of `routes()`), so it
/// wraps the routes merged so far; later generators insert theirs above it,
/// right after the marker.
fn with_layer_last(lib: &str) -> Option<String> {
    let lines: Vec<&str> = lib.lines().collect();
    let marker = lines.iter().position(|line| line.trim() == ROUTES_MARKER)?;
    let end = marker + lines[marker..].iter().position(|line| line.trim() == "}")?;
    let indent = &lines[marker][..lines[marker].len() - lines[marker].trim_start().len()];
    let mut out = String::with_capacity(lib.len() + LAYER.len() + indent.len() + 1);
    for (index, line) in lines.iter().enumerate() {
        if index == end {
            out.push_str(indent);
            out.push_str(LAYER);
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

pub fn locale(project: &Project, codes: &[String]) -> CliResult {
    if let Some(code) = codes.iter().find(|code| !valid_code(code)) {
        return Err(CliError::new(format!("invalid locale code `{code}`"))
            .hint("use a language code, optionally with a region or script: en, fr, pt-BR, zh-Hant"));
    }
    let mut edits = Edits::new(project);
    let mut lib = edits.read("src/lib.rs")?.unwrap_or_default();
    let mut declared_codes = match declared(&lib) {
        Some(existing) if existing.is_empty() => {
            return Err(CliError::new(format!("{LOCALES_MACRO}) in src/lib.rs lists no locale"))
                .hint(format!("put the default locale in it: {LOCALES_MACRO}\"en\")")));
        }
        Some(existing) => existing,
        None => {
            lib = with_layer_last(&lib).ok_or_else(|| {
                CliError::new("src/lib.rs is missing the `// ocre:routes` marker")
                    .hint("put `// ocre:routes` on its own line at the end of the `Router::new()` chain in routes()")
            })?;
            lib.push_str(&locales_static(&codes[0]));
            Vec::new()
        }
    };
    let default = declared_codes.first().unwrap_or(&codes[0]).clone();
    for code in codes {
        if declared_codes.contains(code) {
            return Err(CliError::new(format!("locale `{code}` is already declared in src/lib.rs"))
                .hint(format!("edit locales/{code}.yml; `ocre i18n missing` lists keys to translate")));
        }
        declared_codes.push(code.clone());
        let contents = if *code == default { default_file(code) } else { other_file(code, &default) };
        edits.create(&format!("locales/{code}.yml"), contents)?;
    }
    let call = format!(
        "{LOCALES_MACRO}{})",
        declared_codes.iter().map(|code| format!("\"{code}\"")).collect::<Vec<_>>().join(", ")
    );
    let start = lib.find(LOCALES_MACRO).expect("declared above");
    let end = start + lib[start..].find(')').expect("declared() found the closing parenthesis") + 1;
    lib.replace_range(start..end, &call);
    edits.update("src/lib.rs", lib);
    let mut report = edits.apply("generate locale")?;
    report.next = vec![
        "add keys to the locale files; take `i18n: ocre::i18n::I18n` in a handler and call i18n.t(\"key\")".to_owned(),
        "ocre i18n missing".to_owned(),
    ];
    Ok(report)
}

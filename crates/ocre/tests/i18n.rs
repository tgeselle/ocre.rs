use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use tower_service::Service;

use super::*;
use crate::support::{block_on, body_text};

const EN: &str = r#"en:
  hello: "Hello %{name}"
  only_en: English only
  posts:
    count:
      zero: "No posts"
      one: "One post"
      other: "%{count} posts"
    buttons:
      one: First
      save: Save
"#;

const FR: &str = r#"fr:
  hello: "Bonjour %{name}"
  posts:
    count:
      one: "%{count} article"
      other: "%{count} articles"
"#;

const RU: &str =
    "ru:\n  hello: Привет\n  posts:\n    count:\n      one: \"%{count} пост\"\n      many: \"%{count} постов\"\n";

static LOCALES: Locales = LazyLock::new(|| Catalog::load(&[("en", EN), ("fr", FR), ("pt-BR", "pt-BR:\n"), ("ru", RU)]));

/// Formats without the debug-build strictness, as release builds do.
struct Release<'a>(Translation<'a>);

impl fmt::Display for Release<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write(f, false)
    }
}

#[test]
fn loads_every_locale_default_first() {
    assert!(LOCALES.errors().is_empty(), "{:?}", LOCALES.errors());
    assert_eq!(LOCALES.default_locale(), "en");
    assert_eq!(LOCALES.codes().collect::<Vec<_>>(), ["en", "fr", "pt-BR", "ru"]);
    assert_eq!(LOCALES.locale("PT-br").locale(), "pt-BR");
    assert_eq!(LOCALES.locale("de").locale(), "en", "unknown codes use the default");
    assert_eq!(LOCALES.locale("fr").codes().count(), 4);
    assert_eq!(format!("{:?}", LOCALES.locale("fr")), "I18n { locale: \"fr\" }");
}

#[test]
fn invalid_files_are_empty_and_reported() {
    static INVALID: Locales = LazyLock::new(|| Catalog::load(&[("en", "en:\n  a: b\n"), ("fr", "fr:\n  a: [b]\n")]));
    let catalog: &'static Catalog = &INVALID;
    assert_eq!(catalog.errors(), ["locales/fr.yml line 2: quote values starting with `[`, e.g. \"[b]\""]);
    assert_eq!(Release(catalog.locale("fr").t("a")).to_string(), "b", "falls back to the default locale");
    let empty = Catalog::load(&[]);
    assert_eq!(empty.default_locale(), "en");
    assert_eq!(empty.errors().len(), 1);
}

#[test]
fn translates_with_interpolation() {
    let fr = LOCALES.locale("fr");
    assert_eq!(fr.t("hello").arg("name", "Ada").to_string(), "Bonjour Ada");
    assert_eq!(fr.t("hello").to_string(), "Bonjour %{name}", "a missing value stays visible");
    assert_eq!(LOCALES.locale("en").t("only_en").arg("unused", 1).to_string(), "English only");
    static OPEN: Locales = LazyLock::new(|| Catalog::load(&[("en", "en:\n  a: \"%{x} %{open\"\n")]));
    let catalog: &'static Catalog = &OPEN;
    assert_eq!(catalog.locale("en").t("a").arg("x", 1).to_string(), "1 %{open");
}

#[test]
fn plurals_follow_the_locale_rule() {
    let (en, fr, ru) = (LOCALES.locale("en"), LOCALES.locale("fr"), LOCALES.locale("ru"));
    assert_eq!(en.t("posts.count").count(0).to_string(), "No posts", "zero wins when given");
    assert_eq!(en.t("posts.count").count(1).to_string(), "One post");
    let seven: &u8 = &7;
    assert_eq!(en.t("posts.count").count(seven).to_string(), "7 posts", "askama passes references");
    assert_eq!(en.t("posts.count").count(7_usize).to_string(), "7 posts");
    assert_eq!(en.t("posts.count").count(u64::MAX).to_string(), format!("{} posts", i64::MAX));
    assert_eq!(fr.t("posts.count").count(0).to_string(), "0 article");
    assert_eq!(fr.t("posts.count").count(2).to_string(), "2 articles");
    assert_eq!(ru.t("posts.count").count(5).to_string(), "5 постов");
    assert_eq!(ru.t("posts.count").count(3).to_string(), "translation missing: ru.posts.count", "no few, no other");
    assert_eq!(fr.t("posts.count").to_string(), "%{count} articles", "no count: the other form");
    assert_eq!(en.t("hello").count(2).arg("name", "Ada").to_string(), "Hello Ada", "count on a plain text");
    assert_eq!(en.t("posts.buttons.one").to_string(), "First", "a mapping with other keys is not plural");
    assert_eq!(en.t("posts.buttons").count(1).to_string(), "translation missing: en.posts.buttons");
}

#[test]
fn missing_keys_fall_back_in_release_and_show_in_debug() {
    let fr = LOCALES.locale("fr");
    assert_eq!(fr.t("only_en").to_string(), "translation missing: fr.only_en");
    assert_eq!(Release(fr.t("only_en")).to_string(), "English only");
    assert_eq!(Release(fr.t("posts.count").count(0)).to_string(), "0 article", "own plural first");
    assert_eq!(Release(fr.t("nowhere")).to_string(), "translation missing: fr.nowhere");
    assert_eq!(Release(LOCALES.locale("en").t("nowhere")).to_string(), "translation missing: en.nowhere");
    let ru = LOCALES.locale("ru");
    assert_eq!(Release(ru.t("posts.count").count(1)).to_string(), "1 пост");
    assert_eq!(Release(ru.t("posts.count").count(3)).to_string(), "3 posts", "the default's other form");
}

#[test]
fn negotiates_accept_language() {
    let pick = |header: &str| LOCALES.negotiate(header).map(|index| LOCALES.tables[index].code);
    assert_eq!(pick("fr-CH, fr;q=0.9, en;q=0.8"), Some("fr"), "same language");
    assert_eq!(pick("de, en;q=0.5, fr;q=0.7"), Some("fr"), "by quality");
    assert_eq!(pick("pt"), Some("pt-BR"), "language to region");
    assert_eq!(pick("PT-br"), Some("pt-BR"));
    assert_eq!(pick("fr;q=0, en;q=bad, *"), None, "q=0, invalid q and * are skipped");
    assert_eq!(pick(""), None);
}

fn select(path: Option<&str>, headers: &[(&str, &str)]) -> Result<&'static str> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        map.append(axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(), value.parse().unwrap());
    }
    I18n::select(&LOCALES, path, &map).map(|i18n| i18n.locale())
}

#[test]
fn selects_path_then_cookie_then_accept_language() {
    assert_eq!(select(Some("fr"), &[("cookie", "locale=ru")]).unwrap(), "fr");
    assert!(matches!(select(Some("xx"), &[]), Err(Error::NotFound)));
    assert_eq!(select(None, &[("cookie", "a=1; locale=ru"), ("accept-language", "fr")]).unwrap(), "ru");
    assert_eq!(select(None, &[("cookie", "locales=ru; locale=xx"), ("accept-language", "fr")]).unwrap(), "fr");
    assert_eq!(select(None, &[("cookie", "x=1"), ("cookie", "locale=fr")]).unwrap(), "fr");
    assert_eq!(select(None, &[("accept-language", "de")]).unwrap(), "en");
    assert_eq!(select(None, &[]).unwrap(), "en");
}

#[test]
fn cookie_remembers_the_locale() {
    assert_eq!(LOCALES.locale("fr").cookie(), "locale=fr; Path=/; Max-Age=31536000; SameSite=Lax");
}

async fn hello(i18n: I18n) -> String {
    i18n.t("hello").arg("name", "Ada").to_string()
}

fn call(router: &mut Router, uri: &str, accept_language: &str) -> (StatusCode, String) {
    let request = Request::get(uri).header("accept-language", accept_language).body(Body::empty()).unwrap();
    let response = block_on(router.call(request)).unwrap();
    (response.status(), body_text(response))
}

#[test]
fn extractor_reads_the_layer_and_the_locale_segment() {
    let routes = Router::new().route("/", get(hello)).route("/{locale}/hello", get(hello));
    let mut app = routes.clone().layer(layer(&LOCALES));
    assert_eq!(call(&mut app, "/", "fr"), (StatusCode::OK, "Bonjour Ada".to_owned()));
    assert_eq!(call(&mut app, "/fr/hello", "en"), (StatusCode::OK, "Bonjour Ada".to_owned()));
    assert_eq!(call(&mut app, "/xx/hello", "fr").0, StatusCode::NOT_FOUND);
    let (status, _) = call(&mut routes.clone(), "/", "fr");
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "without the layer");
}

#[test]
fn check_reports_syntax_and_missing_keys() {
    let problems = check(&[
        ("en", EN),
        ("fr", FR),
        ("ja", "ja:\n  hello: こんにちは\n  posts:\n    count: \"%{count}件\"\n"),
        ("ru", RU),
        ("de", "de:\n  x: [\n"),
    ]);
    let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
    assert_eq!(
        lines,
        [
            "locales/de.yml:2: quote values starting with `[`, e.g. \"[\"",
            "locales/fr.yml: missing only_en",
            "locales/fr.yml: missing posts.buttons.one",
            "locales/fr.yml: missing posts.buttons.save",
            "locales/ja.yml: missing only_en",
            "locales/ja.yml: missing posts.buttons.one",
            "locales/ja.yml: missing posts.buttons.save",
            "locales/ru.yml: missing only_en",
            "locales/ru.yml: missing posts.buttons.one",
            "locales/ru.yml: missing posts.buttons.save",
            "locales/ru.yml: missing posts.count.few",
            "locales/ru.yml: missing posts.count.other",
        ]
    );
    assert_eq!(
        check(&[("en", "en:\n  a: [\n"), ("fr", "fr:\n")]),
        [Problem::Invalid {
            locale: "en".into(),
            line: 2,
            message: "quote values starting with `[`, e.g. \"[\"".into()
        }]
    );
    assert_eq!(check(&[("en", EN)]), []);
}

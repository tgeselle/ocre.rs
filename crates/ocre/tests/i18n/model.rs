use std::sync::LazyLock;

use crate::{
    FieldError, Validator,
    i18n::{Catalog, Locales},
};

const EN: &str = r#"en:
  models:
    post: Article
  errors:
    format: "%{attribute}: %{message}"
    messages:
      too_long:
        one: "is too long (1 character max)"
        other: "is too long (%{count} characters max)"
"#;

const FR: &str = r#"fr:
  models:
    post:
      one: Article
      other: Articles
  attributes:
    password: Mot de passe
    post:
      title: Titre
  errors:
    models:
      post:
        blank: "manque pour un %{model}"
    attributes:
      body:
        blank: "est vide"
"#;

static LOCALES: Locales = LazyLock::new(|| Catalog::load(&[("en", EN), ("fr", FR), ("ru", "ru:\n")]));

fn errors(checks: impl FnOnce(&mut Validator)) -> Vec<FieldError> {
    let mut v = Validator::new();
    checks(&mut v);
    v.errors().to_vec()
}

#[test]
fn names_models_and_attributes() {
    let fr = LOCALES.locale("fr");
    assert_eq!(fr.model_name("post", 2), "Articles");
    assert_eq!(fr.model_name("comment", 2), "Comment");
    assert_eq!(LOCALES.locale("ru").model_name("post", 5), "Article", "the default locale's text");
    assert_eq!(fr.attribute("post", "title"), "Titre");
    assert_eq!(fr.attribute("user", "password"), "Mot de passe");
    assert_eq!(fr.attribute("user", "published_at"), "Published at");
}

#[test]
fn translates_errors_from_the_most_specific_key() {
    let fr = LOCALES.locale("fr");
    let found = errors(|v| {
        v.required("title", "").required("body", "").required("summary", "");
    });
    let messages: Vec<String> = found.iter().map(|error| fr.error_message("post", error)).collect();
    assert_eq!(
        messages,
        ["manque pour un Article", "manque pour un Article", "manque pour un Article"],
        "model key first"
    );
    let messages: Vec<String> = found.iter().map(|error| fr.error_message("comment", error)).collect();
    assert_eq!(messages, ["doit être rempli(e)", "est vide", "doit être rempli(e)"], "then attribute, then built-in");
}

#[test]
fn interpolates_counts_and_attributes() {
    let fr = LOCALES.locale("fr");
    let found = errors(|v| {
        v.range("rating", 9, 1..=5)
            .greater_than("ends_on", "2026-01-01", "2026-02-01")
            .confirmation("password", "secret", "typo");
    });
    let messages: Vec<String> = found.iter().map(|error| fr.error_message("user", error)).collect();
    assert_eq!(
        messages,
        ["doit être inférieur ou égal à 5", "doit être supérieur à 2026-02-01", "ne concorde pas avec Mot de passe"]
    );
    let en = LOCALES.locale("en");
    let too_long = errors(|v| {
        v.max_length("title", "ab", 1).max_length("body", "abc", 2);
    });
    assert_eq!(en.error_message("post", &too_long[0]), "is too long (1 character max)");
    assert_eq!(en.error_message("post", &too_long[1]), "is too long (2 characters max)");
}

#[test]
fn keeps_custom_messages_and_translates_known_english_ones() {
    let fr = LOCALES.locale("fr");
    let found = errors(|v| {
        v.check("ends_at", true, "must be after the start")
            .check("email", true, "has already been taken")
            .required("title", "")
            .message("needs a title");
    });
    let messages: Vec<String> = found.iter().map(|error| fr.error_message("user", error)).collect();
    assert_eq!(messages, ["must be after the start", "n'est pas disponible", "needs a title"]);
    assert_eq!(found[2].key(), None, "message() drops the key");
    assert_eq!(LOCALES.locale("ru").error_message("user", &found[1]), "has already been taken");
}

#[test]
fn full_messages_follow_errors_format() {
    let found = errors(|v| {
        v.required("title", "");
    });
    assert_eq!(LOCALES.locale("fr").full_message("post", &found[0]), "Titre manque pour un Article");
    assert_eq!(LOCALES.locale("en").full_message("post", &found[0]), "Title: can't be blank");
}

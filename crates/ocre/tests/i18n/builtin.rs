use super::*;

#[test]
fn every_language_has_every_english_key() {
    let english: Vec<&str> = EN.iter().map(|(key, _)| *key).collect();
    for (language, entries) in LANGUAGES {
        let keys: Vec<&str> = entries.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, english, "{language}");
        for (key, text) in entries {
            if key.ends_with("_names") {
                let expected = if key.contains("month") { 12 } else { 7 };
                assert_eq!(text.split(',').count(), expected, "{language}.{key}");
            }
        }
    }
}

#[test]
fn picks_plural_forms_with_the_language_rule() {
    let key = "datetime.distance_in_words.x_days";
    assert_eq!(get("en", key, Some(1)), Some("1 day"));
    assert_eq!(get("en", key, Some(0)), Some("%{count} days"));
    assert_eq!(get("fr", key, Some(0)), Some("1 jour"), "French: 0 is `one`");
    assert_eq!(get("pt-BR", key, Some(4)), Some("%{count} dias"), "the language of a region code");
    assert_eq!(get("en", key, None), Some("%{count} days"), "`other` without a count");
    assert_eq!(get("EN", "errors.messages.blank", Some(3)), Some("can't be blank"), "plain text for any count");
    assert_eq!(get("en", "errors.messages", None), None);
    assert_eq!(get("en", "datetime.distance_in_words.x_days.one.more", Some(1)), None);
    assert_eq!(get("ru", "errors.messages.blank", None), None, "no built-in Russian");
}

#[test]
fn finds_the_key_of_an_english_message() {
    assert_eq!(english_key("has already been taken"), Some("taken"));
    assert_eq!(english_key("can't be blank"), Some("blank"));
    assert_eq!(english_key("%{count} days"), None, "only error messages");
    assert_eq!(english_key("is odd"), None);
}

use super::*;

#[test]
fn build_dependencies_are_added_once_in_their_section() {
    let bare = "[package]\nname = \"a\"\n\n[dependencies]\nserde = \"1\"\n";
    let added = with_build_dependency(bare, PULLDOWN_CMARK);
    assert!(added.ends_with(&format!("[build-dependencies]\n{PULLDOWN_CMARK}\n")), "{added}");
    assert_eq!(with_build_dependency(&added, PULLDOWN_CMARK), added, "already there");
    let existing = "[build-dependencies]\ncc = \"1\"\n";
    assert_eq!(
        with_build_dependency(existing, PULLDOWN_CMARK),
        format!("[build-dependencies]\n{PULLDOWN_CMARK}\ncc = \"1\"\n")
    );
}

#[test]
fn the_module_follows_the_apps_urls() {
    let plain = blog_rs(Urls::Plain);
    assert!(plain.contains("async fn index() -> Result"), "{plain}");
    assert!(plain.contains("    pages\n}"), "{plain}");
    assert!(!plain.contains("I18n") && !plain.contains("__"), "{plain}");
    let prefixed = blog_rs(Urls::Prefixed);
    assert!(prefixed.contains("Router::new().nest(\"/{locale}\", pages)"));
    assert!(prefixed.contains("async fn show(Path(ShowParams { slug }): Path<ShowParams>, i18n: I18n)"));
    let unprefixed = blog_rs(Urls::UnprefixedDefault);
    assert!(unprefixed.contains("ocre::i18n::localized(pages)") && !unprefixed.contains("__"));
}

#[test]
fn only_the_default_locales_sample_carries_the_date() {
    let default = sample(true);
    let date = default.lines().find_map(|line| line.strip_prefix("published: ")).unwrap();
    assert!(date.len() == 10 && date.starts_with("20"), "{date}");
    let other = sample(false);
    assert!(!other.contains("published") && other.contains("Translate this file"));
    assert!(now_days() > 20_000);
}

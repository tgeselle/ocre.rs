use super::*;

#[test]
fn derives_names_from_any_casing() {
    for input in ["BlogPost", "blog_post", "blog-post", "Blog Post"] {
        let names = ModelNames::parse(input).unwrap();
        assert_eq!(names.model, "BlogPost");
        assert_eq!(names.singular, "blog_post");
        assert_eq!(names.plural, "blog_posts");
        assert_eq!(names.human_plural, "Blog posts");
    }
}

#[test]
fn keeps_acronyms_as_one_word() {
    assert_eq!(ModelNames::parse("HTTPLog").unwrap().plural, "httplogs");
    assert_eq!(ModelNames::parse("ApiKey").unwrap().plural, "api_keys");
}

#[test]
fn pluralizes_last_word_only() {
    assert_eq!(ModelNames::parse("Category").unwrap().plural, "categories");
    assert_eq!(ModelNames::parse("Day").unwrap().plural, "days");
    assert_eq!(ModelNames::parse("Box").unwrap().plural, "boxes");
    assert_eq!(ModelNames::parse("Match").unwrap().plural, "matches");
    assert_eq!(ModelNames::parse("Person").unwrap().plural, "people");
    assert_eq!(ModelNames::parse("SalesPerson").unwrap().plural, "sales_people");
}

#[test]
fn rejects_names_that_are_not_identifiers() {
    assert!(ModelNames::parse("").is_err());
    assert!(ModelNames::parse("2Fast").is_err());
    assert!(ModelNames::parse("--").is_err());
}

#[test]
fn singularizes_what_pluralize_makes() {
    for (plural, singular) in
        [("photos", "photo"), ("categories", "category"), ("boxes", "box"), ("keys", "key"), ("people", "person")]
    {
        assert_eq!(singularize(plural).as_deref(), Some(singular), "{plural}");
    }
    for not_plural in ["photo", "s", "data", "series_x"] {
        assert_eq!(singularize(not_plural), None, "{not_plural}");
    }
}

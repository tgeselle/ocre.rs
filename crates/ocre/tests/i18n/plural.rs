use super::*;

fn names(rule: Rule, numbers: &[i64]) -> Vec<&'static str> {
    numbers.iter().map(|&n| rule.category(n).name()).collect()
}

#[test]
fn locales_map_to_their_language_rule() {
    let cases = [
        ("en", Rule::One),
        ("de-AT", Rule::One),
        ("ja", Rule::Other),
        ("fr", Rule::ZeroOne),
        ("pt_BR", Rule::ZeroOne),
        ("RU", Rule::EastSlavic),
        ("pl", Rule::Polish),
        ("cs", Rule::CzechSlovak),
        ("ar", Rule::Arabic),
        ("he", Rule::Hebrew),
    ];
    for (code, rule) in cases {
        assert_eq!(Rule::for_locale(code), rule, "{code}");
    }
}

#[test]
fn categories_follow_cldr_for_whole_numbers() {
    assert_eq!(names(Rule::One, &[0, 1, 2, -1]), ["other", "one", "other", "one"]);
    assert_eq!(names(Rule::Other, &[0, 1, 2]), ["other"; 3]);
    assert_eq!(names(Rule::ZeroOne, &[0, 1, 2]), ["one", "one", "other"]);
    assert_eq!(
        names(Rule::EastSlavic, &[1, 21, 11, 2, 24, 12, 5, 0, 111]),
        ["one", "one", "many", "few", "few", "many", "many", "many", "many"]
    );
    assert_eq!(names(Rule::Polish, &[1, 21, 2, 22, 12, 5]), ["one", "many", "few", "few", "many", "many"]);
    assert_eq!(names(Rule::CzechSlovak, &[1, 2, 4, 5, 22]), ["one", "few", "few", "other", "other"]);
    assert_eq!(names(Rule::Hebrew, &[1, 2, 3]), ["one", "two", "other"]);
    assert_eq!(
        names(Rule::Arabic, &[0, 1, 2, 3, 10, 103, 11, 99, 100, 102]),
        ["zero", "one", "two", "few", "few", "few", "many", "many", "other", "other"]
    );
}

#[test]
fn every_rule_lists_the_categories_it_produces() {
    for rule in [
        Rule::One,
        Rule::Other,
        Rule::ZeroOne,
        Rule::EastSlavic,
        Rule::Polish,
        Rule::CzechSlovak,
        Rule::Hebrew,
        Rule::Arabic,
    ] {
        let listed = rule.categories();
        assert!(listed.contains(&Category::Other));
        for n in 0..=200 {
            assert!(listed.contains(&rule.category(n)), "{rule:?} {n}");
        }
    }
}

#[test]
fn category_names_round_trip() {
    for category in Category::ALL {
        assert_eq!(Category::from_name(category.name()), Some(category));
    }
    assert_eq!(Category::from_name("title"), None);
}

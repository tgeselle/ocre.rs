use askama::Template;

use crate::filters;

#[derive(Template)]
#[template(
    source = r#"{{ n|number_with_delimiter }}|{{ n|number_with_precision(1) }}|{{ n|number_to_currency("€") }}|{{ n|number_to_percentage(0) }}|{{ n|number_to_human_size }}|{{ n|number_to_human }}|{{ recent|time_ago_in_words }}|{{ at|distance_of_time_in_words(0) }}|{{ at|strftime("%Y") }}|{{ text|excerpt("b", 1) }}|{{ text|highlight("b") }}|{{ text|word_wrap(1) }}|{{ crate::helpers::class_names([("tab", true), ("active", active)]) }}"#,
    ext = "html"
)]
struct Everything<'a> {
    n: f64,
    at: i64,
    recent: i64,
    text: &'a str,
    active: bool,
}

#[test]
fn every_filter_renders_its_helper() {
    let html = Everything { n: 2048.0, at: 86_400 * 365, recent: crate::now() - 120, text: "a <b> c", active: true }
        .render()
        .unwrap();
    assert_eq!(
        html,
        "2,048|2048.0|€2,048.00|2048%|2 KB|2.05 Thousand|2 minutes|about 1 year|1971|...&#60;b&#62;...|a &lt;<mark>b</mark>&gt; c|a\n&#60;b&#62;\nc|tab active"
    );
}

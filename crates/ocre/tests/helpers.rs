use super::*;

#[test]
fn number_helpers_format_numbers_and_keep_other_text() {
    let all = |value: &str| {
        [
            number_with_delimiter(value),
            number_with_precision(value, 2),
            number_to_currency(value, "$"),
            number_to_percentage(value, 1),
            number_to_human_size(value),
            number_to_human(value),
        ]
    };
    assert_eq!(
        all("1234567.891"),
        ["1,234,567.891", "1234567.89", "$1,234,567.89", "1234567.9%", "1.2 MB", "1.23 Million"]
    );
    assert_eq!(all("-0.001"), ["-0.001", "-0.00", "$0.00", "-0.0%", "-0.001", "-0"]);
    assert_eq!(all("-42"), ["-42", "-42.00", "-$42.00", "-42.0%", "-42", "-42"]);
    assert_eq!(all("n/a"), ["n/a"; 6]);
    assert_eq!(all("1e3"), ["1e3", "1000.00", "$1,000.00", "1000.0%", "1000 bytes", "1 Thousand"]);
    assert_eq!(number_with_delimiter(" 1000 "), "1,000");
}

#[test]
fn human_numbers_use_three_significant_digits() {
    assert_eq!(human(123.0), "123");
    assert_eq!(human(1234.0), "1.23 Thousand");
    assert_eq!(human(12_345.0), "12.3 Thousand");
    assert_eq!(human(489_939.0), "490 Thousand");
    assert_eq!(human(999_999.0), "1 Million");
    assert_eq!(human(1.5e18), "1500 Quadrillion");
    assert_eq!(human(-2_000_000.0), "-2 Million");
}

#[test]
fn distances_use_rails_wording() {
    let words = |seconds: i64| distance_of_time_in_words(1_000_000_000, 1_000_000_000 + seconds);
    let minute = 60;
    let day = 1440 * minute;
    let year = 525_600 * minute;
    assert_eq!(words(29), "less than a minute");
    assert_eq!(words(30), "1 minute");
    assert_eq!(words(-44 * minute), "44 minutes");
    assert_eq!(words(45 * minute), "about 1 hour");
    assert_eq!(words(90 * minute), "about 2 hours");
    assert_eq!(words(day), "1 day");
    assert_eq!(words(2 * day), "2 days");
    assert_eq!(words(30 * day), "about 1 month");
    assert_eq!(words(59 * day), "about 2 months");
    assert_eq!(words(90 * day), "3 months");
    assert_eq!(words(year), "about 1 year");
    assert_eq!(words(year + 80 * day), "about 1 year");
    assert_eq!(words(year + 200 * day), "over 1 year");
    assert_eq!(words(2 * year - 10 * day), "almost 2 years");
    assert_eq!(distance_of_time_in_words("yesterday", "2026-01-01"), "yesterday");
}

#[test]
fn time_ago_measures_from_now() {
    assert_eq!(time_ago_in_words(crate::now() - 120), "2 minutes");
    assert_eq!(time_ago_in_words("soon"), "soon");
}

#[test]
fn times_parse_from_d1_text_and_unix_seconds() {
    let at = parse_time("2026-09-29 14:05:00").unwrap();
    assert_eq!(format_time(at, "%F %T"), "2026-09-29 14:05:00");
    assert_eq!(parse_time("0"), Some(0));
    assert_eq!(parse_time("1970-01-02"), Some(86_400));
    assert_eq!(parse_time("2026-09-29T14:05:00.123Z"), Some(at));
    assert_eq!(parse_time("2026-09-29T16:05+02:00"), Some(at));
    assert_eq!(parse_time("2026-09-29T12:05:00-02:00"), Some(at));
    assert_eq!(parse_time("1969-12-31 23:59:59"), Some(-1));
    for bad in [
        "",
        "2026-13-01",
        "2026-02-00",
        "2026-9-29",
        "2026-09-29 24:00",
        "2026-09-29 12",
        "2026-09-29 12:00:00:00",
        "2026-09-29 12:00+xx",
        "abcd-ef-gh",
    ] {
        assert_eq!(parse_time(bad), None, "{bad}");
    }
}

#[test]
fn strftime_formats_every_directive() {
    assert_eq!(
        strftime(
            "2026-01-05 09:03:07",
            "%Y %y %m %-m %d %-d %e %H %-H %I %-I %M %S %p %b %B %a %A %j %F %T %% %q %-q %"
        ),
        "2026 26 01 1 05 5  5 09 9 09 9 03 07 AM Jan January Mon Monday 005 2026-01-05 09:03:07 % %q %-q %"
    );
    assert_eq!(strftime("2024-12-31 00:00:00", "%I %p %j %A"), "12 AM 366 Tuesday");
    assert_eq!(strftime("2026-09-29 23:59:59", "%I:%M %p"), "11:59 PM");
    assert_eq!(strftime("1969-12-31 12:00:00", "%F %a"), "1969-12-31 Wed");
    assert_eq!(strftime("later", "%Y"), "later");
}

#[test]
fn excerpts_cut_around_the_first_match() {
    assert_eq!(excerpt("Hello wonderful World", "WORLD", 4), "...ful World");
    assert_eq!(excerpt("Hello wonderful World", "hello", 4), "Hello won...");
    assert_eq!(excerpt("café", "É", 4), "café");
    assert_eq!(excerpt("short", "missing", 4), "");
    assert_eq!(excerpt("short", "much longer than the text", 4), "");
    assert_eq!(excerpt("short", "", 4), "");
}

#[test]
fn highlight_escapes_and_marks_every_match() {
    assert_eq!(highlight(r#"a"b'c & A"#, "a"), "<mark>a</mark>&quot;b&#39;c &amp; <mark>A</mark>");
    assert_eq!(highlight("<b>", ""), "&lt;b&gt;");
    assert_eq!(highlight("<b>", "<"), "<mark>&lt;</mark>b&gt;");
}

#[test]
fn word_wrap_breaks_at_spaces_and_keeps_paragraphs() {
    assert_eq!(word_wrap("The quick brown fox\njumps", 10), "The quick\nbrown fox\njumps");
    assert_eq!(word_wrap("incomprehensibilities ok", 10), "incomprehensibilities\nok");
    assert_eq!(word_wrap("", 10), "");
}

#[test]
fn class_names_keep_the_enabled_ones() {
    assert_eq!(class_names(&[("a", true), ("", true), ("b", false), ("c", true)]), "a c");
    assert_eq!(class_names(&[("a", false)]), "");
}

#[test]
fn current_page_compares_paths_then_query_and_host_when_given() {
    assert!(current_page("/", "https://shop.example.com"));
    assert!(current_page("/posts/1?tab=a", "/posts/1/"));
    assert!(!current_page("/posts/1", "/posts/1?tab=a"));
    assert!(!current_page("/posts", "/posts/1"));
    assert!(current_page("https://Shop.example.com/", "https://shop.example.com/?"), "an empty query matches none");
    assert!(!current_page("https://a.example/posts", "https://b.example/posts"));
    assert_eq!(split_url("http://h?q#f"), (Some("h"), "/", Some("q")));
}

#[test]
fn time_zone_options_mark_the_selected_zone() {
    let options = time_zone_options("Asia/Tokyo");
    assert_eq!(options.matches("<option").count(), TIME_ZONES.len());
    assert!(options.starts_with("<option>UTC</option>"));
    assert!(options.contains("<option selected>Asia/Tokyo</option>") && !options.contains("selected>UTC"));
}

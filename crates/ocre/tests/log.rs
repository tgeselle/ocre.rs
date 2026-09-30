use super::*;

#[test]
fn levels_parse_their_rails_names() {
    assert_eq!(Level::parse(" Debug "), Some(Level::Debug));
    assert_eq!(Level::parse("info"), Some(Level::Info));
    assert_eq!(Level::parse("warn"), Some(Level::Warn));
    assert_eq!(Level::parse("unknown"), Some(Level::Error));
    assert_eq!(Level::parse("error"), Some(Level::Error));
    assert_eq!(Level::parse("off"), None);
    let names: Vec<&str> = [Level::Debug, Level::Info, Level::Warn, Level::Error].map(Level::as_str).to_vec();
    assert_eq!(names, ["debug", "info", "warn", "error"]);
}

#[test]
fn formats_parse_loco_names() {
    assert_eq!(Format::parse("compact"), Some(Format::Text));
    assert_eq!(Format::parse("text"), Some(Format::Text));
    assert_eq!(Format::parse(" json"), Some(Format::Json));
}

#[test]
fn json_lines_start_with_level_and_message_and_filter_secrets_at_any_depth() {
    let log = Logger::new().with("request_id", "abc").with("user", serde_json::json!({"email": "a@b.c", "id": 1}));
    assert_eq!(
        log.line(Level::Debug, "hi \"you\"", Format::Json),
        r#"{"level":"debug","message":"hi \"you\"","request_id":"abc","user":{"email":"[FILTERED]","id":1}}"#
    );
}

#[test]
fn text_lines_quote_values_that_need_it() {
    let log = Logger::new().with("empty", "").with("eq", "a=b").with("n", 1.5).with("ok", "x").with("none", ());
    assert_eq!(log.line(Level::Info, "m", Format::Text), r#"INFO m empty="" eq="a=b" n=1.5 none=null ok=x"#);
}

#[test]
fn with_replaces_a_field_and_unserializable_values_are_null() {
    struct Broken;
    impl Serialize for Broken {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("no"))
        }
    }
    let log = Logger::new().with("a", 1).with("a", 2).with("b", Broken);
    assert_eq!(log.field("a"), Some(&Value::from(2)));
    assert_eq!(log.field("b"), Some(&Value::Null));
}

#[test]
fn panic_lines_name_the_location_when_known() {
    assert_eq!(panic_line("boom", Some(("src/lib.rs", 3, 9))), "panicked at src/lib.rs:3:9: boom");
    assert_eq!(panic_line("boom", None), "panicked: boom");
}

/// The only test that changes the global settings; the others do not read them.
#[test]
fn configure_sets_the_threshold_and_format_and_falls_back_to_the_build_default() {
    let log = Logger::new();
    configure(Some("error"), Some("json"));
    assert_eq!(settings(), (Level::Error as u8, Format::Json));
    assert!(!log.enabled(Level::Warn) && log.enabled(Level::Error));
    log.warn("not written");
    log.error("written");
    for level in [Level::Debug, Level::Info, Level::Warn] {
        log.log(level, &"skipped");
    }
    configure(Some("OFF"), Some("text"));
    assert!(!log.enabled(Level::Error));
    configure(Some("none"), None);
    assert_eq!(settings().0, OFF);
    configure(Some("loud"), Some("yaml"));
    assert_eq!(settings(), (Level::Debug as u8, Format::Text), "tests are debug builds");
    log.debug("written as text");
    log.info(format_args!("{}", 1));
    log.warn("w");
    configure(None, None);
    assert_eq!(settings(), (default_level() as u8, default_format()));
    SETTINGS.store(0, Ordering::Relaxed);
    assert_eq!(settings(), (Level::Debug as u8, Format::Text));
}

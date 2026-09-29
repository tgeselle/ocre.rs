use super::*;

fn messages(v: &mut Validator) -> Vec<String> {
    match v.finish() {
        Ok(()) => vec![],
        Err(Error::Invalid(errors)) => errors.iter().map(FieldError::full_message).collect(),
        Err(other) => panic!("unexpected {other:?}"),
    }
}

#[test]
fn valid_input_passes() {
    let mut v = Validator::new();
    v.required("title", "Dune").max_length("title", "Dune", 10).min_length("title", "Dune", 2);
    v.range("pages", 5, 1..=10).safe_integer("pages", 5).inclusion("status", "draft", &["draft", "live"]);
    v.email("email", "a@b.co").check("slug", false, "is taken");
    assert!(v.is_valid());
    assert_eq!(messages(&mut v), Vec::<String>::new());
}

#[test]
fn numbers_from_form_text() {
    let mut v = Validator::new();
    assert_eq!(v.number::<i64>("pages", " 12 "), Some(12));
    assert_eq!(v.optional_number::<f64>("rating", ""), None);
    assert_eq!(v.optional_number::<f64>("rating", "4.5"), Some(4.5));
    assert!(v.is_valid());
    assert_eq!(v.number::<i64>("pages", "many"), None);
    assert_eq!(v.optional_number::<i64>("stock", "1.5"), None);
    assert_eq!(messages(&mut v), ["Pages is not a number", "Stock is not a number"]);
}

#[test]
fn json_from_form_text() {
    let mut v = Validator::new();
    assert_eq!(v.json("settings", r#" {"theme": "dark"} "#), Some(serde_json::json!({"theme": "dark"})));
    assert_eq!(v.json("settings", "null"), Some(serde_json::Value::Null));
    assert_eq!(v.optional_json("metadata", "  "), None);
    assert_eq!(v.optional_json("metadata", "[1, 2]"), Some(serde_json::json!([1, 2])));
    assert!(v.is_valid());
    assert_eq!(v.json("settings", ""), None);
    assert_eq!(v.optional_json("metadata", "{theme: dark}"), None);
    assert_eq!(messages(&mut v), ["Settings is not valid JSON", "Metadata is not valid JSON"]);
}

#[test]
fn dates_and_datetimes() {
    for ok in ["2024-02-29", "2026-12-31", "2026-04-30"] {
        assert!(Validator::new().date("on", ok).is_valid(), "{ok}");
    }
    for bad in [
        "2025-02-29",
        "1900-02-29",
        "2026-04-31",
        "2026-13-01",
        "2026-00-10",
        "26-01-01",
        "2026-1-01",
        "2026/01/01",
        "",
        "abcd-ef-gh",
    ] {
        assert!(!Validator::new().date("on", bad).is_valid(), "{bad}");
    }
    for ok in ["2026-09-29 10:30", "2026-09-29T10:30", "2026-09-29 23:59:59", "2000-02-29T00:00:00"] {
        assert!(Validator::new().datetime("at", ok).is_valid(), "{ok}");
    }
    for bad in [
        "2026-09-29",
        "2026-09-29 24:00",
        "2026-09-29x10:30",
        "2026-09-29 10:60",
        "2026-09-29 1:30",
        "2026-09-29 10:30:61",
        "2026-09-29 10:30:00:00",
        "éééééééééé 10:30",
    ] {
        assert!(!Validator::new().datetime("at", bad).is_valid(), "{bad}");
    }
    let mut v = Validator::new();
    v.date("on", "x").datetime("at", "y");
    assert_eq!(messages(&mut v), ["On is not a valid date", "At is not a valid date and time"]);
}

#[test]
fn times_uuids_and_decimals() {
    for ok in ["00:00", "23:59", "09:30:15"] {
        assert!(Validator::new().time("at", ok).is_valid(), "{ok}");
    }
    for bad in ["24:00", "9:30", "09:60", "09:30:60", "", "09:30:00:00"] {
        assert!(!Validator::new().time("at", bad).is_valid(), "{bad}");
    }
    for ok in ["67e55044-10b1-426f-9247-bb680e5fe0c8", "67E55044-10B1-426F-9247-BB680E5FE0C8"] {
        assert!(Validator::new().uuid("id", ok).is_valid(), "{ok}");
    }
    for bad in
        ["", "67e55044-10b1-426f-9247", "67e5504410b1426f9247bb680e5fe0c8", "g7e55044-10b1-426f-9247-bb680e5fe0c8"]
    {
        assert!(!Validator::new().uuid("id", bad).is_valid(), "{bad}");
    }
    for ok in ["0", "19.99", "-3", "+0.5", "0012.000"] {
        assert!(Validator::new().decimal("n", ok).is_valid(), "{ok}");
    }
    for bad in ["", "-", "1.", ".5", "1e3", "1,5", " 1", "1.2.3", "--1"] {
        assert!(!Validator::new().decimal("n", bad).is_valid(), "{bad}");
    }
    let mut v = Validator::new();
    v.time("at", "x").uuid("id", "x").decimal("n", "x");
    assert_eq!(messages(&mut v), ["At is not a valid time", "Id is not a valid UUID", "N is not a decimal number"]);
}

#[test]
fn every_failure_is_collected_with_rails_messages() {
    let mut v = Validator::new();
    v.required("title", "  ")
        .max_length("summary", "héllo", 4)
        .min_length("code", "ab", 3)
        .range("pages", 0, 1..=10)
        .range("rating", 6.5, 0.0..=5.0)
        .safe_integer("big", i64::MAX)
        .inclusion("status", "gone", &["draft"])
        .check("slug", true, "has already been taken");
    assert!(!v.is_valid());
    assert_eq!(
        messages(&mut v),
        [
            "Title can't be blank",
            "Summary is too long (maximum is 4 characters)",
            "Code is too short (minimum is 3 characters)",
            "Pages must be greater than or equal to 1",
            "Rating must be less than or equal to 5",
            "Big must be less than or equal to 9007199254740991",
            "Status is not included in the list",
            "Slug has already been taken",
        ]
    );
    assert!(v.is_valid(), "finish drains the errors");
}

#[test]
fn email_shape() {
    let bad =
        ["", "a", "@b.co", "a@", "a@b", "a@@b.co", "a@.co", "a@b.", "a b@c.co", "a@c.co\r\nBcc: x@y.z", "<a@b.co>"];
    for bad in bad {
        let mut v = Validator::new();
        assert!(!v.email("email", bad).is_valid(), "{bad}");
    }
}

#[test]
fn field_errors_display_and_serialize() {
    let error = FieldError::new("published_at", "is invalid");
    assert_eq!(error.to_string(), "Published at is invalid");
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        serde_json::json!({"field": "published_at", "message": "is invalid"})
    );
}

#[test]
fn merges_errors_from_another_validator() {
    let mut parsed = Validator::new();
    parsed.number::<i64>("pages", "x");
    let mut model = Validator::new();
    model.required("title", "");
    let err = parsed.merge(model).finish().unwrap_err();
    let Error::Invalid(fields) = err else { panic!("expected Invalid") };
    assert_eq!(fields.iter().map(|f| f.field.as_str()).collect::<Vec<_>>(), ["pages", "title"]);
}

#[test]
fn rails_style_checks() {
    let mut v = Validator::new();
    v.exclusion("username", "ada", &["admin"]).length("zip", "75001", 5).acceptance("terms", true);
    v.greater_than("n", 2, 1).greater_than_or_equal_to("n", 1, 1).less_than("n", 1, 2).less_than_or_equal_to("n", 2, 2);
    v.other_than("n", 1, 2)
        .confirmation("password", "a", "a")
        .absence("nickname", "")
        .format("slug", "a-1", |c| c != ' ');
    assert_eq!(messages(&mut v), Vec::<String>::new());
    v.exclusion("username", "admin", &["admin"]).length("zip", "7500", 5).acceptance("terms", false);
    v.greater_than("n", 1, 1).greater_than_or_equal_to("n", 0, 1).less_than("n", 2, 2).less_than_or_equal_to("n", 3, 2);
    v.other_than("n", 2, 2)
        .confirmation("password", "a", "b")
        .absence("nickname", "x")
        .format("slug", "a b", |c| c != ' ');
    assert_eq!(
        messages(&mut v),
        [
            "Username is reserved",
            "Zip is the wrong length (should be 5 characters)",
            "Terms must be accepted",
            "N must be greater than 1",
            "N must be greater than or equal to 1",
            "N must be less than 2",
            "N must be less than or equal to 2",
            "N must be other than 2",
            "Password confirmation doesn't match Password",
            "Nickname must be blank",
            "Slug is invalid",
        ]
    );
}

#[test]
fn custom_messages_replace_only_the_last_failed_check() {
    let mut v = Validator::new();
    v.required("title", "").message("needs a title");
    v.required("body", "text").message("ignored: the check passed");
    v.range("pages", 0, 1..=5).message("must be positive");
    v.required("summary", "ok").message("not applied: the last check passed");
    assert_eq!(v.errors().len(), 2);
    assert_eq!(messages(&mut v), ["Title needs a title", "Pages must be positive"]);
    assert!(v.errors().is_empty());
    // The first bound that fails is the only error of a range check.
    v.range("pages", 9, 1..=5);
    assert_eq!(messages(&mut v), ["Pages must be less than or equal to 5"]);
}

#[test]
fn enum_values_from_form_text() {
    let mut v = Validator::new();
    assert_eq!(v.one_of::<bool>("flag", "true"), Some(true));
    assert_eq!(v.optional_one_of::<bool>("flag", ""), None);
    assert_eq!(v.optional_one_of::<bool>("flag", "false"), Some(false));
    assert!(v.is_valid());
    assert_eq!(v.optional_one_of::<bool>("flag", "maybe"), None);
    assert_eq!(messages(&mut v), ["Flag is not included in the list"]);
}

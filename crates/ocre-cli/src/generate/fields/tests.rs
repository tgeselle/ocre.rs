use super::*;

#[test]
fn parses_types_and_modifiers() {
    let field = Field::parse("title:string^").unwrap();
    assert_eq!(
        (field.name.as_str(), field.ty, field.optional, field.unique),
        ("title", FieldType::String, false, true)
    );
    let field = Field::parse("summary:text?").unwrap();
    assert_eq!((field.optional, field.unique), (true, false));
    let field = Field::parse("code:string?^").unwrap();
    assert_eq!((field.optional, field.unique), (true, true));
    let field = Field::parse("author:references").unwrap();
    assert_eq!(field.name, "author_id");
    assert_eq!(field.target.unwrap().plural, "authors");
}

#[test]
fn rejects_bad_fields() {
    let error = |spec: &str| Field::parse(spec).unwrap_err().message;
    assert_eq!(error("title"), "field `title` has no type");
    assert_eq!(error("title:varchar"), "unknown field type `varchar` for `title`");
    assert_eq!(error("Title:string"), "invalid field name `Title`");
    assert_eq!(error("type:string"), "field name `type` is reserved");
    assert_eq!(error("order:integer"), "field name `order` is reserved");
    assert_eq!(error("id:integer"), "field name `id` is reserved");
    assert_eq!(error("done:boolean?"), "boolean field `done` cannot be optional");
    assert_eq!(error("2x:references"), "invalid field name `2x`");
    assert!(parse_fields(&["a:string".into(), "a:text".into()]).is_err(), "duplicate");
}

#[test]
fn sql_columns() {
    let column = |spec: &str| Field::parse(spec).unwrap().sql_column();
    assert_eq!(column("title:string"), "title TEXT NOT NULL");
    assert_eq!(column("summary:text?"), "summary TEXT");
    assert_eq!(column("pages:integer"), "pages INTEGER NOT NULL");
    assert_eq!(column("rating:float?"), "rating REAL");
    assert_eq!(column("done:boolean"), "done INTEGER NOT NULL DEFAULT 0");
    assert_eq!(column("on:date"), "on TEXT NOT NULL");
    assert_eq!(column("at:datetime?"), "at TEXT");
    assert_eq!(column("author:references"), "author_id INTEGER NOT NULL REFERENCES authors(id) ON DELETE CASCADE");
    assert_eq!(column("editor:references?"), "editor_id INTEGER REFERENCES editors(id) ON DELETE CASCADE");
}

#[test]
fn rust_types_labels_and_kinds() {
    let field = |spec: &str| Field::parse(spec).unwrap();
    assert_eq!(field("rating:float?").column_type(), "Option<f64>");
    assert_eq!(field("done:boolean").column_type(), "bool");
    assert_eq!(field("author:references").column_type(), "i64");
    assert_eq!(field("author:references").label(), "Author");
    assert_eq!(field("owner_id:integer").label(), "Owner id");
    assert!(field("on:date").ty.is_textual() && !field("on:date").ty.is_numeric());
    assert!(field("author:references").ty.is_numeric());
}

#[test]
fn checks_per_type() {
    let checks = |spec: &str| Field::parse(spec).unwrap().checks("x");
    assert_eq!(checks("title:string"), ["v.required(\"title\", x);"]);
    assert!(checks("title:string?").is_empty());
    assert_eq!(checks("on:date"), ["v.date(\"on\", x);"]);
    assert_eq!(checks("at:datetime?"), ["v.datetime(\"at\", x);"]);
    assert_eq!(checks("pages:integer"), ["v.safe_integer(\"pages\", x);"]);
    assert!(checks("rating:float").is_empty() && checks("done:boolean").is_empty());
}

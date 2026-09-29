use super::*;

fn fields(specs: &[&str]) -> Vec<Field> {
    parse_fields(&specs.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap()
}

#[test]
fn create_table_names() {
    let sql = infer("create_tags", &fields(&["label:string^"])).unwrap();
    assert!(
        sql.starts_with("CREATE TABLE tags (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    label TEXT NOT NULL,\n")
    );
    assert!(sql.ends_with("CREATE UNIQUE INDEX index_tags_on_label ON tags (label);\n"));
}

#[test]
fn add_columns_give_existing_rows_a_value() {
    let sql = infer(
        "add_details_to_posts",
        &fields(&["slug:string^", "views:integer", "note:text?", "live:boolean", "editor:references?"]),
    )
    .unwrap();
    assert_eq!(
        sql,
        "ALTER TABLE posts ADD COLUMN slug TEXT NOT NULL DEFAULT '';\n\
         ALTER TABLE posts ADD COLUMN views INTEGER NOT NULL DEFAULT 0;\n\
         ALTER TABLE posts ADD COLUMN note TEXT;\n\
         ALTER TABLE posts ADD COLUMN live INTEGER NOT NULL DEFAULT 0;\n\
         ALTER TABLE posts ADD COLUMN editor_id INTEGER REFERENCES editors(id) ON DELETE CASCADE;\n\
         CREATE UNIQUE INDEX index_posts_on_slug ON posts (slug);\n\
         CREATE INDEX index_posts_on_editor_id ON posts (editor_id);\n"
    );
    assert_eq!(infer("add_x_to_posts", &[]).unwrap_err().message, "add_..._to_... needs the columns to add");
    assert_eq!(
        infer("add_author_to_posts", &fields(&["author:references"])).unwrap_err().message,
        "`author_id` must be optional when added to an existing table"
    );
}

#[test]
fn remove_columns_from_the_name_or_the_fields() {
    assert_eq!(infer("remove_slug_from_posts", &[]).unwrap(), "ALTER TABLE posts DROP COLUMN slug;\n");
    assert_eq!(
        infer("remove_old_from_posts", &fields(&["a:string", "b:integer"])).unwrap(),
        "ALTER TABLE posts DROP COLUMN a;\nALTER TABLE posts DROP COLUMN b;\n"
    );
}

#[test]
fn other_names_are_empty_unless_fields_are_given() {
    assert_eq!(infer("backfill_slugs", &[]).unwrap(), "");
    assert_eq!(
        infer("backfill_slugs", &fields(&["slug:string"])).unwrap_err().message,
        "cannot tell which table `backfill_slugs` changes"
    );
}

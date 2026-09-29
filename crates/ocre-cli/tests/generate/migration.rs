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
         ALTER TABLE posts ADD COLUMN editor_id INTEGER REFERENCES editors(id) ON DELETE SET NULL;\n\
         CREATE UNIQUE INDEX index_posts_on_slug ON posts (slug);\n\
         CREATE INDEX index_posts_on_editor_id ON posts (editor_id);\n"
    );
    assert_eq!(infer("add_x_to_posts", &[]).unwrap_err().message, "add_..._to_... needs the columns to add");
    assert_eq!(
        infer("add_author_to_posts", &fields(&["author:references"])).unwrap_err().message,
        "`author_id` must be optional when added to an existing table"
    );
    let error = infer("add_cover_to_posts", &fields(&["cover:attachment"])).unwrap_err();
    assert_eq!(error.message, "`cover` must be optional when added to an existing table");
    assert_eq!(error.hint.as_deref(), Some("existing rows have no file: use `name:attachment?`"));
    assert_eq!(
        infer("add_cover_to_posts", &fields(&["cover:attachment?"])).unwrap(),
        "ALTER TABLE posts ADD COLUMN cover_key TEXT;\nALTER TABLE posts ADD COLUMN cover_filename TEXT;\n\
         ALTER TABLE posts ADD COLUMN cover_content_type TEXT;\nALTER TABLE posts ADD COLUMN cover_size INTEGER;\n"
    );
    assert_eq!(
        infer("add_settings_to_posts", &fields(&["settings:json", "extra:json?"])).unwrap(),
        "ALTER TABLE posts ADD COLUMN settings TEXT NOT NULL CHECK (json_valid(settings)) DEFAULT '{}';\n\
         ALTER TABLE posts ADD COLUMN extra TEXT CHECK (json_valid(extra));\n"
    );
}

#[test]
fn remove_columns_from_the_name_or_the_fields() {
    assert_eq!(
        infer("remove_slug_from_posts", &[]).unwrap(),
        "DROP INDEX IF EXISTS index_posts_on_slug;\nALTER TABLE posts DROP COLUMN slug;\n"
    );
    assert_eq!(
        infer("remove_old_from_posts", &fields(&["a:string^", "b:integer", "author:references?"])).unwrap(),
        "DROP INDEX IF EXISTS index_posts_on_a;\nDROP INDEX IF EXISTS index_posts_on_author_id;\n\
         ALTER TABLE posts DROP COLUMN a;\nALTER TABLE posts DROP COLUMN b;\nALTER TABLE posts DROP COLUMN author_id;\n"
    );
    assert_eq!(
        infer("remove_cover_from_posts", &fields(&["cover:attachment?"])).unwrap(),
        "ALTER TABLE posts DROP COLUMN cover_key;\nALTER TABLE posts DROP COLUMN cover_filename;\n\
         ALTER TABLE posts DROP COLUMN cover_content_type;\nALTER TABLE posts DROP COLUMN cover_size;\n"
    );
}

fn specs(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

fn structural_sql(name: &str, columns: &[&str]) -> Result<Option<(String, bool)>, CliError> {
    structural(name, &specs(columns), &|| Ok(None))
}

#[test]
fn index_migrations_take_column_names() {
    assert_eq!(
        structural_sql("add_index_to_posts", &["author_id", "created_at"]).unwrap(),
        Some((
            "CREATE INDEX index_posts_on_author_id_and_created_at ON posts (author_id, created_at);\n".to_owned(),
            false
        ))
    );
    assert_eq!(
        structural_sql("add_unique_index_to_posts", &["slug"]).unwrap().unwrap().0,
        "CREATE UNIQUE INDEX index_posts_on_slug ON posts (slug);\n"
    );
    assert_eq!(
        structural_sql("remove_index_from_posts", &["author_id", "created_at"]).unwrap().unwrap().0,
        "DROP INDEX IF EXISTS index_posts_on_author_id_and_created_at;\n"
    );
    for columns in [&[][..], &["title:string"][..]] {
        let error = structural_sql("add_index_to_posts", columns).unwrap_err();
        assert_eq!(error.message, "`add_index_to_posts` needs the indexed column names");
    }
    assert_eq!(structural_sql("add_slug_to_posts", &["slug:string"]).unwrap(), None);
}

#[test]
fn renames_and_drops() {
    let sql = |name: &str| structural_sql(name, &[]).unwrap().unwrap();
    assert_eq!(
        sql("rename_title_to_headline_in_posts"),
        ("ALTER TABLE posts RENAME COLUMN title TO headline;\n".to_owned(), true)
    );
    assert_eq!(sql("rename_posts_to_articles").0, "ALTER TABLE posts RENAME TO articles;\n");
    assert_eq!(sql("drop_tags").0, "DROP TABLE tags;\n");
    let error = structural_sql("rename_posts", &[]).unwrap_err();
    assert_eq!(error.message, "cannot tell what `rename_posts` renames");
    let error = structural_sql("drop_tags", &["label:string"]).unwrap_err();
    assert_eq!(error.message, "`drop_tags` takes no fields");
}

const SCHEMA_SQL: &str = "-- Schema of the local database.\n\n\
CREATE TABLE authors (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    name TEXT NOT NULL\n);\n\n\
CREATE TABLE posts (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    title TEXT NOT NULL CHECK (length(title) > 0),\n    \
rating REAL DEFAULT (0.5),\n    note TEXT DEFAULT 'a, b',\n    author_id INTEGER REFERENCES authors(id) ON DELETE SET NULL,\n    \
UNIQUE (title, author_id)\n);\n\n\
CREATE INDEX index_posts_on_author_id ON posts (author_id);\n\n\
CREATE UNIQUE INDEX index_authors_on_name ON authors (name);\n";

#[test]
fn rebuilds_copy_the_current_definition() {
    let (sql, model) = structural("rebuild_posts", &[], &|| Ok(Some(SCHEMA_SQL.to_owned()))).unwrap().unwrap();
    assert!(model);
    assert_eq!(
        sql,
        "-- Rebuilds `posts` to change what ALTER TABLE cannot (a column's type, NOT NULL,\n\
         -- DEFAULT, CHECK or REFERENCES): edit the CREATE TABLE below, and keep both\n\
         -- column lists of the INSERT in step with it.\n\
         PRAGMA defer_foreign_keys = true;\n\
         CREATE TABLE posts_new (\n    id INTEGER PRIMARY KEY AUTOINCREMENT,\n    title TEXT NOT NULL CHECK (length(title) > 0),\n    \
         rating REAL DEFAULT (0.5),\n    note TEXT DEFAULT 'a, b',\n    author_id INTEGER REFERENCES authors(id) ON DELETE SET NULL,\n    \
         UNIQUE (title, author_id)\n);\n\
         INSERT INTO posts_new (id, title, rating, note, author_id) SELECT id, title, rating, note, author_id FROM posts;\n\
         DROP TABLE posts;\n\
         ALTER TABLE posts_new RENAME TO posts;\n\
         CREATE INDEX index_posts_on_author_id ON posts (author_id);\n\
         PRAGMA defer_foreign_keys = false;\n"
    );
}

#[test]
fn rebuilds_refuse_missing_schemas_tables_and_referenced_tables() {
    let error = structural("rebuild_posts", &[], &|| Ok(None)).unwrap_err();
    assert_eq!(error.message, "db/schema.sql not found");
    let error = structural("rebuild_tags", &[], &|| Ok(Some(SCHEMA_SQL.to_owned()))).unwrap_err();
    assert_eq!(error.message, "table `tags` is not in db/schema.sql");
    let error = structural("rebuild_authors", &[], &|| Ok(Some(SCHEMA_SQL.to_owned()))).unwrap_err();
    assert_eq!(error.message, "`posts` references `authors`: rebuilding it would delete or clear their rows");
}

#[test]
fn other_names_are_empty_unless_fields_are_given() {
    assert_eq!(infer("backfill_slugs", &[]).unwrap(), "");
    assert_eq!(
        infer("backfill_slugs", &fields(&["slug:string"])).unwrap_err().message,
        "cannot tell which table `backfill_slugs` changes"
    );
}

#[test]
fn added_enums_default_to_their_first_value() {
    assert_eq!(
        infer("add_status_to_posts", &fields(&["status:enum:draft,live", "mood:enum:ok?"])).unwrap(),
        "ALTER TABLE posts ADD COLUMN status TEXT NOT NULL CHECK (status IN ('draft', 'live')) DEFAULT 'draft';\n\
         ALTER TABLE posts ADD COLUMN mood TEXT CHECK (mood IN ('ok'));\n"
    );
}

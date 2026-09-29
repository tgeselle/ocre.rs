use super::*;

#[test]
fn parses_fields_and_rejects_bad_ones() {
    assert_eq!(Field::parse("pages:integer").unwrap(), Field { name: "pages".into(), ty: FieldType::Integer });
    assert!(Field::parse("title").is_err(), "missing type");
    assert!(Field::parse("title:varchar").is_err(), "unknown type");
    assert!(Field::parse("Title:string").is_err(), "not snake_case");
    assert!(Field::parse("type:string").is_err(), "Rust keyword");
    assert!(Field::parse("order:integer").is_err(), "SQL keyword");
    assert!(Field::parse("id:integer").is_err(), "generated column");
    assert!(parse_fields(&["a:string".into(), "a:text".into()]).is_err(), "duplicate");
}

#[test]
fn registers_module_after_markers_keeping_indent() {
    let lib = "use x;\n// ocre:modules\n\nfn routes() {\n    Router::new()\n        .route(\"/\", get(home))\n        // ocre:routes\n}\n";
    let out = register_module(lib, "books").unwrap();
    assert!(out.contains("// ocre:modules\nmod books;\n"));
    assert!(out.contains("        // ocre:routes\n        .merge(books::routes())\n}"));
    let again = register_module(&out, "authors").unwrap();
    assert!(again.contains("// ocre:modules\nmod authors;\nmod books;\n"));
}

#[test]
fn refuses_lib_without_markers() {
    assert!(register_module("fn main() {}\n", "books").is_err());
}

#[test]
fn numbers_migrations_after_the_highest_existing() {
    let dir = std::env::temp_dir().join(format!("ocre-migrations-{}", std::process::id()));
    let migrations = dir.join("migrations");
    std::fs::create_dir_all(&migrations).unwrap();
    assert!(next_migration_path(&dir, "a").unwrap().ends_with("0001_a.sql"));
    std::fs::write(migrations.join("0001_a.sql"), "").unwrap();
    std::fs::write(migrations.join("0009_b.sql"), "").unwrap();
    std::fs::write(migrations.join(".gitkeep"), "").unwrap();
    assert!(next_migration_path(&dir, "c").unwrap().ends_with("0010_c.sql"));
    std::fs::remove_dir_all(dir).unwrap();
}

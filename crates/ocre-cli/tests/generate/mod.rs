use super::*;

fn project(name: &str) -> Project {
    let root = std::env::temp_dir().join(format!("ocre-edits-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("migrations")).unwrap();
    Project { root, database_name: "app".into(), api_only: false, generate: Default::default() }
}

#[test]
fn inserts_after_markers_with_their_indentation() {
    let text = "a\n    // m\nb\n";
    assert_eq!(insert_after_marker(text, "// m", "x").unwrap(), "a\n    // m\n    x\nb\n");
    assert_eq!(insert_after_marker(text, "// m", "    y\n\n    z").unwrap(), "a\n    // m\n    y\n\n    z\nb\n");
    assert_eq!(insert_after_marker("// m\n// m\n", "// m", "x").unwrap(), "// m\nx\n// m\n", "first marker only");
    assert!(insert_after_marker(text, "// missing", "x").is_none());
}

#[test]
fn edits_are_pending_until_applied() {
    let project = project("pending");
    std::fs::write(project.root.join("existing.txt"), "old").unwrap();
    let mut edits = Edits::new(&project);
    edits.create("new.txt", "1".into()).unwrap();
    assert!(edits.exists("new.txt") && !project.root.join("new.txt").exists());
    assert_eq!(edits.read("new.txt").unwrap().unwrap(), "1");
    edits.update("new.txt", "2".into());
    edits.update("existing.txt", "new".into());
    assert_eq!(edits.read("existing.txt").unwrap().unwrap(), "new");
    assert_eq!(edits.read("absent.txt").unwrap(), None);
    assert_eq!(edits.create("existing.txt", String::new()).unwrap_err().message, "existing.txt already exists");
    assert_eq!(edits.create("new.txt", String::new()).unwrap_err().message, "new.txt already exists");
    let report = edits.apply("test").unwrap();
    assert_eq!((report.created, report.updated), (vec!["new.txt".to_owned()], vec!["existing.txt".to_owned()]));
    assert_eq!(std::fs::read_to_string(project.root.join("new.txt")).unwrap(), "2");
    std::fs::remove_dir_all(&project.root).unwrap();
}

#[test]
fn unreadable_files_are_errors() {
    let project = project("unreadable");
    std::fs::create_dir(project.root.join("dir.txt")).unwrap();
    assert!(Edits::new(&project).read("dir.txt").is_err());
    std::fs::remove_dir_all(&project.root).unwrap();
}

#[test]
fn numbers_migrations_after_disk_and_pending_files() {
    let project = project("numbers");
    let mut edits = Edits::new(&project);
    assert_eq!(next_migration_path(&edits, "a").unwrap(), "migrations/0001_a.sql");
    std::fs::write(project.root.join("migrations/0009_create_posts.sql"), "").unwrap();
    std::fs::write(project.root.join("migrations/.gitkeep"), "").unwrap();
    edits.create("migrations/0010_create_tags.sql", String::new()).unwrap();
    assert_eq!(next_migration_path(&edits, "b").unwrap(), "migrations/0011_b.sql");
    assert!(edits.has_create_migration("posts").unwrap() && edits.has_create_migration("tags").unwrap());
    assert!(!edits.has_create_migration("users").unwrap());
    std::fs::remove_dir_all(&project.root).unwrap();
    assert_eq!(next_migration_path(&Edits::new(&project), "c").unwrap(), "migrations/0001_c.sql", "no migrations dir");
}

#[test]
fn routes_need_lib_rs_and_both_markers() {
    let project = project("routes");
    let message = |edits: &mut Edits| register_routes(edits, "posts").unwrap_err().message;
    let expected = "src/lib.rs is missing the `// ocre:modules` or `// ocre:routes` marker";
    assert_eq!(message(&mut Edits::new(&project)), expected, "no lib.rs");
    std::fs::create_dir(project.root.join("src")).unwrap();
    std::fs::write(project.root.join("src/lib.rs"), "// ocre:modules\n").unwrap();
    assert_eq!(message(&mut Edits::new(&project)), expected, "no routes marker");
    std::fs::remove_dir_all(&project.root).unwrap();
}

#[test]
fn features_go_on_the_dependency_not_the_test_one() {
    let cargo = "[dependencies]\nocre = { path = \"x\" }\n\n[dev-dependencies]\nocre = { path = \"x\", features = [\"testing\"] }\n";
    assert_eq!(
        with_ocre_feature(cargo, "realtime").unwrap(),
        "[dependencies]\nocre = { path = \"x\", features = [\"realtime\"] }\n\n[dev-dependencies]\nocre = { path = \"x\", features = [\"testing\"] }\n"
    );
}

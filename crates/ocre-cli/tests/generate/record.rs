use super::*;

fn hunk(after: Option<&str>, removed: &[&str], added: &[&str]) -> Hunk {
    let owned = |lines: &[&str]| lines.iter().map(|l| (*l).to_owned()).collect();
    Hunk { after: after.map(str::to_owned), removed: owned(removed), added: owned(added) }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|a| (*a).to_owned()).collect()
}

#[test]
fn diff_finds_insertions_replacements_and_removals() {
    assert_eq!(diff("a\nb\n", "a\nb\n"), vec![]);
    assert_eq!(diff("a\nc\n", "a\nb\nc\n"), vec![hunk(Some("a"), &[], &["b"])]);
    assert_eq!(diff("a\n", "x\na\n"), vec![hunk(None, &[], &["x"])]);
    assert_eq!(diff("a\nold\nc\n", "a\nnew\nc\n"), vec![hunk(Some("a"), &["old"], &["new"])]);
    assert_eq!(diff("a\nb\nc\n", "a\nc\n"), vec![hunk(Some("a"), &["b"], &[])]);
    assert_eq!(
        diff("m\n1\nn\n2\n", "m\nx\n1\nn\ny\n2\n"),
        vec![hunk(Some("m"), &[], &["x"]), hunk(Some("n"), &[], &["y"])],
        "separate hunks keep their own context"
    );
}

#[test]
fn revert_undoes_each_kind_of_hunk() {
    for (old, new) in [("a\nc\n", "a\nb\nc\n"), ("a\n", "x\na\n"), ("a\nold\nc", "a\nnew\nc"), ("a\nb\nc\n", "a\nc\n")]
    {
        let mut text = new.to_owned();
        for h in diff(old, new).iter().rev() {
            text = revert(&text, h).unwrap();
        }
        assert_eq!(text, old, "{new:?} back to {old:?}");
    }
    assert_eq!(revert("b\n", &hunk(None, &["a"], &["b"])).unwrap(), "a\n");
    assert_eq!(revert("b", &hunk(None, &[], &["b"])).unwrap(), "", "nothing left");
}

#[test]
fn revert_finds_moved_lines_only_when_unambiguous() {
    let inserted = hunk(Some("// marker"), &[], &["mod posts;"]);
    let later = "// marker\nmod tags;\nmod posts;\n";
    assert_eq!(revert(later, &inserted).unwrap(), "// marker\nmod tags;\n");
    assert_eq!(revert("// marker\nmod tags;\n", &inserted), None, "gone");
    let twice = "mod posts;\n// marker\nmod tags;\nmod posts;\n";
    assert_eq!(revert(twice, &inserted), None, "ambiguous");
    assert_eq!(revert("x\n", &hunk(Some("// marker"), &["gone"], &[])), None, "no context for a removal");
    assert_eq!(revert("", &hunk(None, &[], &["b"])), None);
}

#[test]
fn records_name_the_typed_command() {
    let record = Record::new("generate api", &args(&["g", "--pretend", "scaffold", "Post", "title:string", "--json"]));
    assert_eq!((record.generator.as_str(), record.name.as_str()), ("scaffold", "Post"));
    assert_eq!(record.command, "ocre g --pretend scaffold Post title:string");
    let record = Record::new("generate schedule", &args(&["generate", "schedule", "nightly", "0 3 * * *", "it's"]));
    assert_eq!(record.command, "ocre generate schedule nightly '0 3 * * *' 'it'\\''s'");
    let record = Record::new("generate cache", &[]);
    assert_eq!(
        (record.command.as_str(), record.generator.as_str(), record.name.as_str()),
        ("ocre generate cache", "cache", "")
    );
    assert!(record.is_empty());
    let mut record = Record::new("generate x", &args(&["g", "x", "Blog Post!"]));
    record.created("a", "");
    let root = std::env::temp_dir().join(format!("ocre-record-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(record.save(&root).unwrap(), ".ocre/generated/0001_x_blog_post_.json");
    assert_eq!(Record::new("generate y", &[]).save(&root).unwrap(), ".ocre/generated/0002_y.json");
    let loaded = load_all(&root).unwrap();
    assert_eq!(loaded[0].2, record);
    assert_eq!(quote(""), "''");
    std::fs::remove_dir_all(&root).unwrap();
}

use super::*;

fn statements(json: &str) -> Vec<Statement> {
    serde_json::from_str(json).unwrap()
}

#[test]
fn rows_render_as_a_table_in_column_order() {
    let json =
        r#"[{"results":[{"zeta":1.5,"name":"a","id":1},{"zeta":null,"name":"longer name","id":22}],"success":true}]"#;
    assert_eq!(
        render(&statements(json)),
        "zeta | name        | id\n\
         -----+-------------+---\n\
         1.5  | a           | 1\n\
         NULL | longer name | 22\n\
         (2 rows)\n"
    );
}

#[test]
fn each_statement_gets_its_own_table_and_count() {
    let json = r#"[{"results":[],"success":true},{"results":[{"ok":true}]},{"success":true}]"#;
    assert_eq!(render(&statements(json)), "(0 rows)\n\nok\n----\ntrue\n(1 row)\n\n(0 rows)\n");
}

#[test]
fn a_row_must_be_an_object() {
    let err = serde_json::from_str::<Vec<Statement>>(r#"[{"results":[1]}]"#).err().unwrap();
    assert!(err.to_string().contains("expected a result row object"), "{err}");
}

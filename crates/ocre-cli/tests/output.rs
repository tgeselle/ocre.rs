use super::*;

#[test]
fn one_line_errors_append_the_hint_when_there_is_one() {
    assert_eq!(CliError::new("bad").to_string_with_hint(), "bad");
    assert_eq!(CliError::new("bad").hint("fix it").to_string_with_hint(), "bad (fix it)");
}

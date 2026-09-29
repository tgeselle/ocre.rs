use super::*;

#[test]
fn app_names_follow_worker_rules() {
    for ok in ["a", "my-blog", "app2", &"a".repeat(63)] {
        assert!(validate_app_name(ok).is_ok(), "{ok}");
    }
    for bad in ["", "My-app", "2app", "-app", "app-", "my_app", "my app", &"a".repeat(64)] {
        assert!(validate_app_name(bad).is_err(), "{bad}");
    }
}

#[test]
fn account_id_goes_after_the_name_line() {
    let toml = with_account_id("name = \"x\"\nmain = \"y\"\n", "abc");
    assert_eq!(toml, "name = \"x\"\naccount_id = \"abc\"\nmain = \"y\"\n");
    let parsed: toml::Table = toml.parse().unwrap();
    assert_eq!(parsed["account_id"].as_str(), Some("abc"));
}

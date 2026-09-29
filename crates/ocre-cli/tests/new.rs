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

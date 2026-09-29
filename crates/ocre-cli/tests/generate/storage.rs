use super::*;

#[test]
fn bucket_names_fit_r2_limits() {
    assert_eq!(bucket_name("blog"), "blog-storage");
    let long = format!("{}-x", "a".repeat(54));
    let name = bucket_name(&long);
    assert_eq!(name, format!("{}-storage", "a".repeat(54)), "no dash before the suffix");
    assert!(bucket_name(&"b".repeat(63)).len() <= 63);
}

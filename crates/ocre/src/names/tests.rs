use super::*;

#[test]
fn humanizes_like_rails() {
    assert_eq!(humanize("published_at"), "Published at");
    assert_eq!(humanize("blog_author_id"), "Blog author");
    assert_eq!(humanize("_id"), " id");
    assert_eq!(humanize("id"), "Id");
    assert_eq!(humanize(""), "");
}

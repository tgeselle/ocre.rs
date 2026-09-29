use super::*;
use crate::params;

#[derive(Debug)]
struct Post;

fn posts() -> Query<Post> {
    Query::table("posts")
}

#[test]
fn every_condition_binds_its_values_in_order() {
    let stmt = posts()
        .eq("a", 1)
        .ne("b", 2)
        .gt("c", 3)
        .gte("d", 4)
        .lt("e", 5)
        .lte("f", 6)
        .between("g", 7, 8)
        .like("h", "x%")
        .not_like("i", "y%")
        .is_null("j")
        .is_not_null("k")
        .to_statement();
    assert_eq!(
        stmt.sql,
        "SELECT * FROM posts WHERE a = ?1 AND b != ?2 AND c > ?3 AND d >= ?4 AND e < ?5 AND f <= ?6 \
         AND g BETWEEN ?7 AND ?8 AND h LIKE ?9 AND i NOT LIKE ?10 AND j IS NULL AND k IS NOT NULL"
    );
    assert_eq!(stmt.params, params![1, 2, 3, 4, 5, 6, 7, 8, "x%", "y%"]);
}

#[test]
fn lists_and_their_empty_forms() {
    let stmt = posts().is_in("id", [1, 2]).not_in("status", ["draft"]).to_statement();
    assert_eq!(stmt.sql, "SELECT * FROM posts WHERE id IN (?1, ?2) AND status NOT IN (?3)");
    assert_eq!(stmt.params, params![1, 2, "draft"]);
    // `IN ()` matches nothing; `NOT IN ()` excludes nothing.
    let empty = posts().is_in("id", Vec::<i64>::new()).not_in("id", Vec::<i64>::new()).to_statement();
    assert_eq!(empty.sql, "SELECT * FROM posts WHERE 0");
    assert_eq!(posts().none().count_statement().sql, "SELECT COUNT(*) AS count FROM posts WHERE 0");
}

#[test]
fn text_matches_escape_like_wildcards() {
    let stmt = posts().contains("title", "5%_").starts_with("slug", "a\\").ends_with("email", "@x.io").to_statement();
    assert_eq!(
        stmt.sql,
        "SELECT * FROM posts WHERE title LIKE ?1 ESCAPE '\\' AND slug LIKE ?2 ESCAPE '\\' AND email LIKE ?3 ESCAPE '\\'"
    );
    assert_eq!(stmt.params, params!["%5\\%\\_%", "a\\\\%", "%@x.io"]);
    assert_eq!(escape_like("plain"), "plain");
}

#[test]
fn groups_combine_with_or_and_not() {
    let stmt = posts()
        .any(|q| q.eq("a", 1).any(|inner| inner.eq("b", 2).eq("c", 3)))
        .not(|q| q.eq("d", 4))
        .any(|q| q)
        .not(|q| q.limit(3))
        .to_statement();
    assert_eq!(stmt.sql, "SELECT * FROM posts WHERE (a = ?1 OR (b = ?2 OR c = ?3)) AND NOT (d = ?4)");
    assert_eq!(stmt.params, params![1, 2, 3, 4]);
}

#[test]
fn raw_fragments_are_parenthesized_and_numbered_outside_quotes() {
    let stmt = posts()
        .eq("x", 0)
        .where_sql("title = '?' OR note = \"a?\" OR body = ?", params!["b"])
        .where_sql("id = ?1", params![9])
        .to_statement();
    assert_eq!(
        stmt.sql,
        "SELECT * FROM posts WHERE x = ?1 AND (title = '?' OR note = \"a?\" OR body = ?2) AND (id = ?1)"
    );
}

#[test]
fn select_distinct_join_group_having_order_and_limits() {
    let query: Query<()> = posts()
        .scope(|q| q.eq("published", true))
        .select("posts.author_id, COUNT(*) AS n")
        .distinct()
        .join("JOIN authors ON authors.id = posts.author_id")
        .group_by("posts.author_id")
        .having("COUNT(*) > ?", params![2])
        .order_in("status", ["a", "b"])
        .order_asc("n")
        .order_sql("lower(name)")
        .page(Page { limit: 10, offset: 20 });
    let stmt = query.to_statement();
    assert_eq!(
        stmt.sql,
        "SELECT DISTINCT posts.author_id, COUNT(*) AS n FROM posts JOIN authors ON authors.id = posts.author_id \
         WHERE published = ?1 GROUP BY posts.author_id HAVING (COUNT(*) > ?2) \
         ORDER BY CASE status WHEN ?3 THEN 0 WHEN ?4 THEN 1 ELSE 2 END, n ASC, lower(name) LIMIT ?5 OFFSET ?6"
    );
    assert_eq!(stmt.params, params![true, 2, "a", "b", 10, 20]);
    assert_eq!(
        query.count_statement().sql,
        "SELECT COUNT(*) AS count FROM (SELECT DISTINCT posts.author_id, COUNT(*) AS n FROM posts \
         JOIN authors ON authors.id = posts.author_id WHERE published = ?1 GROUP BY posts.author_id HAVING (COUNT(*) > ?2))"
    );
    assert_eq!(query.count_statement().params, params![true, 2]);
    let grouped = posts().group_by("author_id");
    assert_eq!(grouped.count_statement().sql, "SELECT COUNT(*) AS count FROM (SELECT * FROM posts GROUP BY author_id)");
}

#[test]
fn reorder_limit_and_offset_alone() {
    let stmt = posts().order_desc("id").order_in("s", ["x"]).reorder().order_by("title", Direction::Asc).limit(5);
    assert_eq!(stmt.to_statement().sql, "SELECT * FROM posts ORDER BY title ASC LIMIT ?1");
    assert_eq!(stmt.to_statement().params, params![5]);
    assert_eq!(posts().offset(3).to_statement().sql, "SELECT * FROM posts LIMIT -1 OFFSET ?1");
}

#[test]
fn statements_for_count_exists_values_update_and_delete() {
    let query = posts().eq("author_id", 7).order_desc("id").limit(2);
    assert_eq!(query.count_statement().sql, "SELECT COUNT(*) AS count FROM posts WHERE author_id = ?1");
    let exists = posts().eq("a", 1).group_by("a").having("COUNT(*) > ?", params![1]).exists_statement();
    assert_eq!(exists.sql, "SELECT 1 FROM posts WHERE a = ?1 GROUP BY a HAVING (COUNT(*) > ?2) LIMIT 1");
    let pluck = query.value_statement("id");
    assert_eq!(pluck.sql, "SELECT id AS value FROM posts WHERE author_id = ?1 ORDER BY id DESC LIMIT ?2");
    assert_eq!(pluck.params, params![7, 2]);
    assert_eq!(
        query.aggregate_statement("SUM(views)").sql,
        "SELECT SUM(views) AS value FROM posts WHERE author_id = ?1"
    );
    let update = query.update_statement(vec![("title", "x".into_param()), ("views", 0.into_param())]);
    assert_eq!(update.sql, "UPDATE posts SET title = ?1, views = ?2 WHERE author_id = ?3");
    assert_eq!(update.params, params!["x", 0, 7]);
    assert_eq!(query.delete_statement().sql, "DELETE FROM posts WHERE author_id = ?1");
    assert_eq!(posts().delete_statement().sql, "DELETE FROM posts");
    assert_eq!(posts().update_statement(vec![("views", 0.into_param())]).sql, "UPDATE posts SET views = ?1");
}

#[test]
fn clone_and_debug_show_the_statement() {
    let query = posts().eq("id", 1);
    let copy = query.clone().limit(1);
    assert_eq!(query.to_statement().sql, "SELECT * FROM posts WHERE id = ?1");
    assert_eq!(copy.to_statement().sql, "SELECT * FROM posts WHERE id = ?1 LIMIT ?2");
    let debug = format!("{query:?}");
    assert!(debug.starts_with("Query { sql: \"SELECT * FROM posts WHERE id = ?1\", params: ["), "{debug}");
}

#[test]
fn directions_read_from_query_strings() {
    for (text, direction) in [("\"asc\"", Direction::Asc), ("\"DESC\"", Direction::Desc), ("\"Desc\"", Direction::Desc)]
    {
        assert_eq!(serde_json::from_str::<Direction>(text).unwrap(), direction);
    }
    assert!(serde_json::from_str::<Direction>("\"up\"").is_err());
    assert_eq!((Direction::Asc.as_sql(), Direction::Desc.as_sql()), ("ASC", "DESC"));
}

#[test]
fn paginated_navigation() {
    let first = Paginated { items: vec![1, 2], total: 5, limit: 2, offset: 0 };
    assert_eq!((first.current_page(), first.total_pages()), (1, 3));
    assert!(first.has_next() && !first.has_previous());
    assert_eq!(first.next_page(), Some(Page { limit: 2, offset: 2 }));
    assert_eq!(first.previous_page(), None);
    let last = Paginated { items: vec![5], total: 5, limit: 2, offset: 4 };
    assert_eq!((last.current_page(), last.next_page()), (3, None));
    assert_eq!(last.previous_page(), Some(Page { limit: 2, offset: 2 }));
    let empty = Paginated::<i32> { items: vec![], total: 0, limit: 10, offset: 0 };
    assert_eq!(empty.total_pages(), 1);
    let shifted = Paginated::<i32> { items: vec![], total: 30, limit: 10, offset: 5 };
    assert_eq!(shifted.previous_page(), Some(Page { limit: 10, offset: 0 }));
    assert_eq!(first.map(|n| n.to_string()).items, ["1", "2"]);
}

use super::*;

#[test]
fn timings_sum_every_statement_and_keep_the_first_hundred() {
    let timings = Timings::default();
    assert_eq!(timings.server_timing(5.0), "db;dur=0;desc=\"0 queries\", total;dur=5");
    timings.clone().record("SELECT 1", 2.0);
    assert_eq!(timings.server_timing(5.0), "db;dur=2;desc=\"1 query\", total;dur=5");
    for _ in 0..MAX_KEPT {
        timings.record("SELECT 2", 1.0);
    }
    assert_eq!(timings.totals(), (101, 102.0));
    let statements = timings.statements();
    assert_eq!(statements.len(), MAX_KEPT);
    assert_eq!(statements[0], Timing { sql: "SELECT 1".into(), ms: 2.0 });
    assert_eq!(timings.summary("GET", "/posts", 200, 150.0), "GET /posts 200 in 150 ms (db: 101 queries, 102 ms)");
}

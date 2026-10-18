use rusqlite::{Connection, params};

use super::*;

/// The table, in an in-memory SQLite database (what D1 runs).
fn db() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(TABLE_SQL).unwrap();
    db
}

/// Records an alert at `now` as `record` does, and returns its status.
fn insert(db: &Connection, reason: &str, key: Option<&str>, now: i64, recipient: bool) -> String {
    db.query_row(
        INSERT_SQL,
        params![reason, key, Option::<&str>::None, now, now - DEDUP_MINUTES * 60, now - 3600, HOURLY_CAP, recipient],
        |row| row.get::<_, String>(1),
    )
    .unwrap()
}

#[test]
fn repeats_are_deduplicated_per_reason_and_key() {
    let db = db();
    let t = 1_790_000_000;
    assert_eq!(insert(&db, "payment.failed", Some("user:1"), t, true), "sent");
    assert_eq!(insert(&db, "payment.failed", Some("user:1"), t + 60, true), "deduped");
    assert_eq!(insert(&db, "payment.failed", Some("user:2"), t + 60, true), "sent", "another key");
    assert_eq!(insert(&db, "upload.failed", Some("user:1"), t + 60, true), "sent", "another reason");
    assert_eq!(insert(&db, "server.error", None, t, true), "sent");
    assert_eq!(insert(&db, "server.error", None, t + 1, true), "deduped", "no key is a key too");
    assert_eq!(insert(&db, "payment.failed", Some("user:1"), t + DEDUP_MINUTES * 60 + 1, true), "sent", "window over");
    assert_eq!(insert(&db, "payment.failed", Some("user:9"), t, false), "unsent", "no recipient");
}

#[test]
fn past_the_hourly_cap_alerts_wait_for_the_digest() {
    let db = db();
    let t = 1_790_000_000;
    for n in 0..HOURLY_CAP {
        assert_eq!(insert(&db, "job.failed", Some(&n.to_string()), t + n, true), "sent");
    }
    assert_eq!(insert(&db, "job.failed", Some("late"), t + 100, true), "overflow");
    assert_eq!(insert(&db, "job.failed", Some("next hour"), t + 3600 + 30, true), "sent");

    let mut claim = db.prepare(CLAIM_SQL).unwrap();
    let held: Vec<Held> = claim
        .query_map([], |row| {
            Ok(Held { id: row.get(0)?, reason: row.get(1)?, dedup_key: row.get(2)?, created_at: row.get(3)? })
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].dedup_key.as_deref(), Some("late"));
    let ids = serde_json::json!([held[0].id]).to_string();
    assert_eq!(db.execute(UNCLAIM_SQL, params![ids]).unwrap(), 1, "back to overflow when the email fails");
    assert_eq!(db.execute(CLEANUP_SQL, params![t + 50]).unwrap(), 20, "older alerts forgotten");
}

#[test]
fn emails_say_what_happened_and_the_rules() {
    let alert = Alert::new("payment.failed").key(42).detail("card declined");
    let email = alert_email("owner@example.com", &alert, 1_790_000_000);
    assert_eq!(email.to, ["owner@example.com"]);
    assert_eq!(email.subject, "Alert: payment.failed");
    assert!(
        email.text.starts_with("payment.failed\n\nWhen: 2026-09-21 14:13 UTC\nAbout: 42\n\ncard declined\n"),
        "{}",
        email.text
    );
    assert!(email.text.contains("once per 30 minutes"));
    let bare = alert_email("o@example.com", &Alert::new("x"), 0);
    assert!(!bare.text.contains("About:") && bare.text.contains("1970-01-01 00:00 UTC"));
}

#[test]
fn digests_group_held_alerts_by_reason() {
    let held = |id, reason: &str, key: Option<&str>, at| Held {
        id,
        reason: reason.into(),
        dedup_key: key.map(Into::into),
        created_at: at,
    };
    let rows = [
        held(1, "job.failed", Some("a"), 120),
        held(2, "server.error", None, 60),
        held(3, "job.failed", Some("b"), 0),
        held(4, "job.failed", Some("a"), 3600),
        held(5, "auth.failed", Some("c"), 60),
    ];
    let email = digest_email("o@example.com", &rows);
    assert_eq!(email.subject, "Alert digest: 5 alerts held back");
    assert_eq!(
        email.text,
        "5 alerts were held back by the hourly limit of 20 emails:\n\n\
         - job.failed: 3 (2 distinct), 1970-01-01 00:00 UTC to 1970-01-01 01:00 UTC\n\
         - auth.failed: 1 (1 distinct), 1970-01-01 00:01 UTC to 1970-01-01 00:01 UTC\n\
         - server.error: 1 (0 distinct), 1970-01-01 00:01 UTC to 1970-01-01 00:01 UTC\n",
        "same count: by reason"
    );
}

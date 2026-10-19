use std::{cell::RefCell, time::Duration};

use super::*;
use crate::{cache::is_read_query, http::Request, support::block_on};

/// D1 messages as the Worker logs show them, through both error shapes of the `worker` crate.
const OVERLOADED: &str = "D1_ERROR: D1 DB is overloaded. Requests queued for too long.";
const LOST: &str = "D1: D1Error { cause: JsValue(Error: Network connection lost. at D1DatabaseSession._sendOrThrow";

#[test]
fn d1_messages_cloudflare_says_to_retry_are_transient() {
    for message in [
        OVERLOADED,
        "D1_ERROR: D1 DB is overloaded. Too many requests queued.",
        "D1_ERROR: Cannot resolve D1 DB due to transient issue on remote node.",
        LOST,
        "D1_ERROR: Network connection lost.",
        "D1_ERROR: Replica disconnected from primary.",
        "D1_ERROR: Internal error in D1 DB storage caused object to be reset.",
        "D1_ERROR: Internal error while starting up D1 DB storage caused object to be reset.",
        "D1_ERROR: D1 DB reset because its code was updated.",
    ] {
        assert!(d1_retryable(message, true), "{message}");
    }
}

#[test]
fn d1_errors_the_app_must_fix_are_not_retried() {
    for message in [
        "D1_ERROR: no such table: posts: SQLITE_ERROR",
        "D1_ERROR: UNIQUE constraint failed: users.email: SQLITE_CONSTRAINT",
        "D1_ERROR: D1 DB storage operation exceeded timeout which caused object to be reset.",
        "D1_ERROR: D1 DB's isolate exceeded its memory limit and was reset.",
        "D1_ERROR: D1 DB exceeded its CPU time limit and was reset.",
        "D1_ERROR: Exceeded maximum DB size.",
        "D1_ERROR: Can't read from request stream because client disconnected.",
        "D1_ERROR: Your account has exceeded D1's free tier daily row read limit.",
    ] {
        assert!(!d1_retryable(message, true), "{message}");
        assert!(!d1_retryable(message, false), "{message}");
    }
}

#[test]
fn writes_are_retried_only_when_they_never_ran() {
    assert!(d1_retryable(OVERLOADED, false));
    assert!(d1_retryable("D1_ERROR: Cannot resolve D1 DB due to transient issue on remote node.", false));
    for message in [
        LOST,
        "D1_ERROR: Replica disconnected from primary.",
        "D1_ERROR: Internal error in D1 DB storage caused object to be reset.",
        "D1_ERROR: D1 DB reset because its code was updated.",
    ] {
        assert!(!d1_retryable(message, false), "{message}");
    }
}

#[test]
fn reads_are_the_statements_the_query_cache_serves() {
    let read = |sql: &str| d1_retryable(LOST, is_read_query(sql));
    assert!(read("SELECT * FROM users WHERE id = ?1"));
    assert!(read("  -- the job\n select v.*, j.id AS job_id FROM videos v"));
    assert!(!read("INSERT INTO posts (title) VALUES (?1) RETURNING *"));
    assert!(!read("WITH x AS (SELECT 1) DELETE FROM posts"));
    assert!(!read("UPDATE users SET seen_at = ?1"));
}

#[test]
fn r2_internal_and_unavailable_errors_are_transient() {
    assert!(r2_retryable("get: We encountered an internal error. Please try again. (10001)"));
    assert!(r2_retryable("head: Service is temporarily unavailable. (10043)"));
    assert!(!r2_retryable("put: Your metadata headers exceed the maximum allowed metadata size. (10012)"));
    assert!(!r2_retryable("completeMultipartUpload: The specified multipart upload does not exist. (10024)"));
}

#[test]
fn s3_calls_are_retried_on_server_errors_and_lost_connections() {
    let request = Request::get("https://example.r2.cloudflarestorage.com/bucket/key?uploadId=1");
    for status in [500, 502, 503, 504] {
        assert!(s3_retryable(&Ok(crate::http::Response::of(&request, status, Default::default(), Vec::new()))));
    }
    for status in [200, 400, 403, 404, 429, 501] {
        assert!(!s3_retryable(&Ok(crate::http::Response::of(&request, status, Default::default(), Vec::new()))));
    }
    assert!(s3_retryable(&Err(request.error("Network connection lost."))));
    assert!(!s3_retryable(&Err(request.error(format!("{} 30000 ms", crate::http::TIMED_OUT)))));
}

#[test]
fn backoff_is_about_100_then_400_ms() {
    assert_eq!(backoff(1, 0), Duration::from_millis(50));
    assert_eq!(backoff(1, 128), Duration::from_millis(100));
    assert_eq!(backoff(1, 255), Duration::from_millis(149));
    assert_eq!(backoff(2, 0), Duration::from_millis(200));
    assert_eq!(backoff(2, 128), Duration::from_millis(400));
    assert_eq!(backoff(2, 255), Duration::from_millis(598));
}

/// Runs `with_retries` over `outcomes` (one per attempt), and returns the result, the calls made and the waits.
fn run(outcomes: &[Result<u8, &'static str>]) -> (Result<u8, &'static str>, usize, Vec<Duration>, Vec<u32>) {
    let calls = RefCell::new(0);
    let waits = RefCell::new(Vec::new());
    let mut retries = Vec::new();
    let result = block_on(with_retries(
        || {
            let outcome = outcomes[*calls.borrow()];
            *calls.borrow_mut() += 1;
            async move { outcome }
        },
        |outcome: &Result<u8, &str>| outcome.is_err_and(|err| err.starts_with("transient")),
        |retry, _| retries.push(retry),
        |wait| {
            waits.borrow_mut().push(wait);
            async {}
        },
    ));
    (result, calls.into_inner(), waits.into_inner(), retries)
}

#[test]
fn transient_failures_are_retried_twice_at_most() {
    let (result, calls, waits, retries) = run(&[Err("transient"), Ok(7)]);
    assert_eq!((result, calls, retries), (Ok(7), 2, vec![1]));
    assert!(waits[0] >= Duration::from_millis(50) && waits[0] < Duration::from_millis(150), "{waits:?}");

    let (result, calls, waits, retries) = run(&[Err("transient 1"), Err("transient 2"), Err("transient 3"), Ok(7)]);
    assert_eq!((result, calls, retries), (Err("transient 3"), 3, vec![1, 2]));
    assert!(waits[1] >= Duration::from_millis(200) && waits[1] < Duration::from_millis(600), "{waits:?}");
}

#[test]
fn other_failures_and_successes_return_at_once() {
    assert_eq!(run(&[Ok(1)]).0, Ok(1));
    let (result, calls, waits, _) = run(&[Err("no such table"), Ok(7)]);
    assert_eq!((result, calls, waits.len()), (Err("no such table"), 1, 0));
}

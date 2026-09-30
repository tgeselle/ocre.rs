use std::collections::HashMap;

use serde_json::json;

use super::*;

/// The shape `ocre g job` generates: an externally tagged, snake_case enum.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Job {
    SendWelcome(SendWelcome),
    Cleanup,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct SendWelcome {
    user_id: i64,
}

fn internal(err: Error) -> String {
    match err {
        Error::Internal(message) => message,
        other => panic!("expected an internal error, got {other:?}"),
    }
}

#[test]
fn a_job_round_trips_through_the_message_text() {
    let job = Job::SendWelcome(SendWelcome { user_id: 7 });
    let text = encode(job_payload(&job).unwrap(), 1_700_000_000).unwrap();
    assert_eq!(text, r#"{"at":1700000000,"job":{"send_welcome":{"user_id":7}}}"#);
    let envelope = decode(Some(text)).unwrap();
    assert_eq!(envelope.at, 1_700_000_000);
    let Payload::Job(value) = envelope.payload else { panic!("a job") };
    assert_eq!(job_name(&value), "send_welcome");
    assert_eq!(decode_job::<Job>(value).unwrap(), job);

    // Unit variants serialize as a string.
    let Payload::Job(value) = job_payload(&Job::Cleanup).unwrap() else { panic!("a job") };
    assert_eq!(job_name(&value), "cleanup");
    assert_eq!(decode_job::<Job>(value).unwrap(), Job::Cleanup);
    // Anything else is named generically.
    assert_eq!(job_name(&json!([1, 2])), "job");
    assert_eq!(job_name(&json!({"a": 1, "b": 2})), "job");
}

#[test]
fn an_email_travels_as_mail() {
    let email = crate::mail::Email::new("ada@example.com", "Hi", "Hello").html("<p>Hello</p>");
    let text = encode(Payload::Mail(Box::new(email.clone())), 5).unwrap();
    assert_eq!(
        text,
        r#"{"at":5,"mail":{"to":["ada@example.com"],"subject":"Hi","text":"Hello","html":"<p>Hello</p>","reply_to":null}}"#
    );
    assert_eq!(decode(Some(text)).unwrap(), Envelope { at: 5, payload: Payload::Mail(Box::new(email.clone())) });
    // Messages queued when `to` was one address still decode.
    let old = r#"{"at":5,"mail":{"to":"ada@example.com","subject":"Hi","text":"Hello","html":"<p>Hello</p>","reply_to":null}}"#;
    assert_eq!(decode(Some(old.to_owned())).unwrap(), Envelope { at: 5, payload: Payload::Mail(Box::new(email)) });
}

#[test]
fn undecodable_messages_explain_why_they_are_dropped() {
    let not_text = decode(None).unwrap_err();
    assert!(not_text.starts_with("the body is not text"), "{not_text}");
    let not_json = decode(Some("hello".to_owned())).unwrap_err();
    assert!(not_json.starts_with("not an Ocre job message (") && not_json.ends_with("): hello"), "{not_json}");
    let long = decode(Some("x".repeat(500))).unwrap_err();
    assert!(long.ends_with(&format!(": {}...", "x".repeat(200))), "{long}");
    let unknown = decode_job::<Job>(json!({"send_goodbye": {"user_id": 1}})).unwrap_err();
    assert!(unknown.starts_with(r#"{"send_goodbye":{"user_id":1}} does not match the app's jobs (unknown variant"#));
    assert!(unknown.ends_with("Fix: keep accepting the old form in src/jobs/mod.rs"), "{unknown}");
}

#[test]
fn enqueue_errors_name_the_fix() {
    // JSON objects need string keys.
    let bad: HashMap<(i32, i32), i32> = HashMap::from([((1, 2), 3)]);
    let message = internal(job_payload(&bad).unwrap_err());
    assert!(message.starts_with("cannot enqueue the job: it does not serialize to JSON ("), "{message}");

    let big = Payload::Job(json!({"import": {"csv": "x".repeat(MAX_MESSAGE_BYTES)}}));
    let message = internal(encode(big, 0).unwrap_err());
    assert!(message.contains("messages hold 128 KB at most. Fix: store large data in D1 or R2"), "{message}");

    let message = internal(missing_queue("JOBS", "Binding `JOBS` is undefined."));
    assert!(
        message.starts_with("the queue binding `JOBS` is missing (Binding `JOBS` is undefined.). Fix: run `ocre g job")
    );
    let message = internal(missing_queue("JOBS_LOW_PRIORITY", "undefined"));
    assert!(message.contains("Fix: run `ocre g job <Name> --queue low-priority`; it adds"), "{message}");
}

#[test]
fn queue_names_map_to_bindings() {
    assert_eq!(binding(DEFAULT_QUEUE).unwrap(), "JOBS");
    assert_eq!(binding("urgent").unwrap(), "JOBS_URGENT");
    assert_eq!(binding("low-priority2").unwrap(), "JOBS_LOW_PRIORITY2");
    for bad in ["", "Urgent", "-a", "a-", "a_b", "a b"] {
        let message = internal(binding(bad).unwrap_err());
        assert!(message.starts_with(&format!("invalid queue name {bad:?}. Fix:")), "{message}");
    }
}

#[test]
fn bulk_enqueue_splits_at_100_messages_and_256_kb() {
    assert!(batches(vec![]).is_empty());
    let small: Vec<String> = (0..250).map(|i| i.to_string()).collect();
    let split = batches(small.clone());
    assert_eq!(split.iter().map(Vec::len).collect::<Vec<_>>(), [100, 100, 50]);
    assert_eq!(split.concat(), small, "order kept");
    let big: Vec<String> = (0..5).map(|_| "x".repeat(100_000)).collect();
    assert_eq!(batches(big).iter().map(Vec::len).collect::<Vec<_>>(), [2, 2, 1]);
}

#[test]
fn only_errors_a_retry_cannot_fix_are_discarded() {
    for err in [
        Error::NotFound,
        Error::bad_request("x"),
        Error::Unauthorized,
        Error::Forbidden,
        Error::Invalid(vec![]),
        Error::PayloadTooLarge("x".into()),
        Error::Conflict("x".into()),
    ] {
        assert!(discards(&err), "{err:?}");
    }
    assert!(!discards(&Error::internal("D1 is down")));
    assert!(!discards(&Error::TooManyRequests));
}

#[test]
fn delays_stop_at_24_hours() {
    assert_eq!(delay_seconds(Duration::ZERO).unwrap(), 0);
    assert_eq!(delay_seconds(Duration::from_millis(90_500)).unwrap(), 90);
    assert_eq!(delay_seconds(MAX_DELAY).unwrap(), 86_400);
    let message = internal(delay_seconds(MAX_DELAY + Duration::from_secs(1)).unwrap_err());
    assert!(message.starts_with("cannot delay a job by 86401 s"), "{message}");
    assert!(message.contains("`ocre g schedule`"), "{message}");
}

#[test]
fn retries_back_off_from_the_due_time() {
    let at = 1_000;
    // First failure right away: 30 s; then twice the time waited.
    assert_eq!(retry_delay(at, at), 30);
    assert_eq!(retry_delay(at + 31, at), 62);
    assert_eq!(retry_delay(at + 93, at), 186);
    // Clock skew (due in the future) and huge waits stay in range.
    assert_eq!(retry_delay(at - 50, at), 30);
    assert_eq!(retry_delay(at + 50_000, at), 86_400);
    assert_eq!(retry_delay(i64::MAX, i64::MIN), 86_400);
}

#[test]
fn dev_routes_list_recent_jobs_and_runs() {
    use axum::http::Request;
    use tower_service::Service;

    for n in 0..55 {
        record_enqueued("default", &Payload::Job(json!({"send_welcome": {"user_id": n}})));
        record_performed("send_welcome", if n % 2 == 0 { "done" } else { "retried" });
    }
    let email = Email::new("ada@example.com", "Hi", "Hello");
    record_enqueued("default", &Payload::Mail(Box::new(email)));
    let mut app = dev_routes::<()>();
    let request = Request::get("/ocre/dev/jobs.json").body(axum::body::Body::empty()).unwrap();
    let response = crate::support::block_on(app.call(request)).unwrap();
    let jobs: serde_json::Value = serde_json::from_str(&crate::support::body_text(response)).unwrap();
    let (enqueued, performed) = (jobs["enqueued"].as_array().unwrap(), jobs["performed"].as_array().unwrap());
    assert_eq!((enqueued.len(), performed.len()), (50, 50), "the last 50 of each; emails are not jobs");
    assert_eq!(enqueued.last().unwrap()["job"], json!({"send_welcome": {"user_id": 54}}));
    assert_eq!(enqueued.last().unwrap()["queue"], "default");
    assert_eq!(performed.last().unwrap()["outcome"], "done");
}

#[test]
fn budgets_take_only_what_is_left() {
    let budget = Budget::new(5);
    assert!(budget.take(3) && !budget.take(3) && budget.take(2));
    assert_eq!(budget.left(), 0);
}

/// One step: done at 3, an error at 99, the next cursor otherwise. A plain
/// function, so every call below shares one `run_steps` instantiation.
fn stepper(cursor: u32) -> std::pin::Pin<Box<dyn Future<Output = Result<Step<u32>>>>> {
    Box::pin(async move {
        match cursor {
            3 => Ok(Step::Done),
            99 => Err(Error::internal("boom")),
            n => Ok(Step::Next(n + 1)),
        }
    })
}

#[test]
fn steps_run_until_done_or_out_of_budget() {
    let run = |calls, start| crate::support::block_on(run_steps(&Budget::new(calls), 2, start, stepper));
    assert_eq!(run(100, 0).unwrap(), None, "done at 3");
    assert_eq!(run(4, 0).unwrap(), Some(2), "two steps of 2 calls, then the cursor to continue from");
    assert_eq!(run(1, 0).unwrap(), Some(0), "not even one step");
    assert!(run(10, 99).is_err());
}

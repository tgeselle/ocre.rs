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
    let text = encode(Payload::Mail(email.clone()), 5).unwrap();
    assert_eq!(
        text,
        r#"{"at":5,"mail":{"to":"ada@example.com","subject":"Hi","text":"Hello","html":"<p>Hello</p>","reply_to":null}}"#
    );
    assert_eq!(decode(Some(text)).unwrap(), Envelope { at: 5, payload: Payload::Mail(email) });
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

    let message = internal(missing_queue("Binding `JOBS` is undefined."));
    assert!(
        message.starts_with("the queue binding `JOBS` is missing (Binding `JOBS` is undefined.). Fix: run `ocre g job")
    );
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

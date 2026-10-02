//! [`once`]: a webhook event's effect runs once, however many times it is delivered.

use crate::{Db, Result, params};

/// A delivery left `processing` this long (seconds) is taken over by the
/// next one: the invocation that claimed it died before finishing.
pub const STALE_AFTER: i64 = 300;

/// What [`once`] did with a delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery<T> {
    /// The effect ran (first delivery, or a retry after a failure) and returned this.
    Processed(T),
    /// Already processed, or being processed by another invocation: nothing ran.
    Duplicate,
}

/// Runs `effect` for the event `event_id` of `source` unless it already ran.
///
/// The delivery is claimed with one insert into `webhook_events` (unique on
/// `source, event_id`), with the raw `payload` kept for the log. When the
/// effect succeeds the event is marked `processed`, and later deliveries of
/// the same id get [`Delivery::Duplicate`]: answer them 200 so the provider
/// stops retrying. When it fails, the event is marked `failed` with the
/// error and the error is returned, so the handler answers 500 and the
/// provider's retry runs the effect again. A delivery still `processing`
/// after [`STALE_AFTER`] seconds is taken over.
///
/// Two to three D1 writes per delivery. Recording and effect are separate
/// statements: an effect made of D1 writes should be idempotent itself
/// (`INSERT ... ON CONFLICT DO NOTHING`, `UPDATE ... WHERE status = 'pending'`)
/// to stay correct if the invocation dies between them.
///
/// # Errors
///
/// The effect's error, or a D1 error (the table is missing: run
/// `ocre g webhook` and `ocre migrate`).
pub async fn once<T, F, Fut>(db: &Db, source: &str, event_id: &str, payload: &[u8], effect: F) -> Result<Delivery<T>>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let now = crate::now();
    let claimed = db
        .execute(
            "INSERT INTO webhook_events (source, event_id, payload, status, received_at) \
             VALUES (?1, ?2, ?3, 'processing', ?4) \
             ON CONFLICT (source, event_id) DO UPDATE SET status = 'processing', attempts = attempts + 1, \
             received_at = ?4 \
             WHERE webhook_events.status = 'failed' \
             OR (webhook_events.status = 'processing' AND webhook_events.received_at < ?5)",
            params![source, event_id, String::from_utf8_lossy(payload).into_owned(), now, now - STALE_AFTER],
        )
        .await?;
    if claimed == 0 {
        return Ok(Delivery::Duplicate);
    }
    match effect().await {
        Ok(value) => {
            db.execute(
                "UPDATE webhook_events SET status = 'processed', error = NULL, processed_at = ?3 \
                 WHERE source = ?1 AND event_id = ?2",
                params![source, event_id, crate::now()],
            )
            .await?;
            Ok(Delivery::Processed(value))
        }
        Err(err) => {
            db.execute(
                "UPDATE webhook_events SET status = 'failed', error = ?3 WHERE source = ?1 AND event_id = ?2",
                params![source, event_id, err.to_string()],
            )
            .await?;
            Err(err)
        }
    }
}

/// The answer of [`post_signed`]: status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// HTTP status.
    pub status: u16,
    /// Body, as text.
    pub text: String,
}

/// POSTs `body` as JSON to `url`, signed: the HMAC-SHA256 of the body with
/// `secret` in `X-Signature` (lowercase hex, see [`sign`](crate::webhooks::sign)),
/// and `Authorization: Bearer <bearer>` when given (API keys of services
/// such as RunPod). One subrequest. Any status is an answer: check
/// `status` (a service's 4xx is not an error of the call).
///
/// # Errors
///
/// [`Error::Internal`](crate::Error::Internal) when the request cannot be
/// built or the service cannot be reached.
pub fn post_signed(
    url: &str,
    secret: &[u8],
    bearer: Option<&str>,
    body: &serde_json::Value,
) -> impl Future<Output = Result<Answer>> + Send + use<> {
    let url = url.to_owned();
    let body = body.to_string();
    let signature = crate::webhooks::sign(secret, body.as_bytes());
    let bearer = bearer.map(str::to_owned);
    worker::send::SendFuture::new(async move {
        let headers = worker::Headers::new();
        headers.set("Content-Type", "application/json")?;
        headers.set("X-Signature", &signature)?;
        if let Some(bearer) = bearer {
            headers.set("Authorization", &format!("Bearer {bearer}"))?;
        }
        let mut init = worker::RequestInit::new();
        init.with_method(worker::Method::Post).with_headers(headers).with_body(Some(body.into()));
        let mut response = worker::Fetch::Request(worker::Request::new_with_init(&url, &init)?).send().await?;
        Ok(Answer { status: response.status_code(), text: response.text().await? })
    })
}

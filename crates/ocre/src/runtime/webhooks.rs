//! [`once`]: a webhook event's effect runs once, however many times it is delivered.

use crate::{Db, Error, Result, config::Environment, params, webhooks::UrlHost};

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

/// Checks that `url` may receive webhooks: `http(s)`, no credentials, and a host that is not (and does not resolve to) a private address.
///
/// Call it when a user saves an endpoint URL, to answer the form with the
/// reason; [`deliver`] calls it again before each delivery in release
/// builds, since a name can point elsewhere later. A host name is
/// resolved with DNS over HTTPS (Cloudflare's resolver, `A` and `AAAA`
/// records: two subrequests), as a Worker has no resolver of its own. A
/// DNS change between the check and the delivery (DNS rebinding) is not
/// caught; Workers cannot reach private networks anyway, so the check
/// mainly keeps the app from calling itself or its own services.
///
/// # Errors
///
/// [`Error::BadRequest`](crate::Error::BadRequest) (400) with the reason
/// ("must start with https:// or http://", "is a private address", "was
/// not found"), or [`Error::Internal`](crate::Error::Internal) when the
/// resolver cannot be reached.
///
/// # Examples
///
/// ```no_run
/// use ocre::{Result, webhooks};
///
/// async fn save_endpoint(url: &str) -> Result<()> {
///     webhooks::check_public_url(url).await?; // 400 "The webhook URL's host 10.0.0.5 is a private address"
///     Ok(())
/// }
/// ```
pub fn check_public_url(url: &str) -> impl Future<Output = Result<()>> + Send + use<> {
    let url = url.to_owned();
    async move {
        let name = match crate::webhooks::url_host(&url)? {
            UrlHost::Ip(ip) => {
                let shown = ip.to_string();
                return crate::webhooks::refused_addresses(&shown, &[ip]).map_or(Ok(()), Err);
            }
            UrlHost::Name(name) => name,
        };
        let mut addresses = Vec::new();
        for kind in ["A", "AAAA"] {
            let answer = crate::http::Request::get("https://cloudflare-dns.com/dns-query")
                .query(&[("name", &name), ("type", kind)])
                .header("accept", "application/dns-json")
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await?
                .error_for_status()?;
            addresses.extend(crate::webhooks::dns_addresses(&answer.json()?));
        }
        crate::webhooks::refused_addresses(&name, &addresses).map_or(Ok(()), Err)
    }
}

/// POSTs `payload` as JSON to the endpoint `url`, signed as a [Standard Webhooks](https://www.standardwebhooks.com) delivery, and returns the answer.
///
/// Headers: `webhook-id` (`id`, the event's id, the same on every retry so
/// receivers can deduplicate), `webhook-timestamp` (now) and
/// `webhook-signature` (`v1,<base64>` with `secret`, see
/// [`sign_standard`](crate::webhooks::sign_standard)); receivers verify it
/// with the Standard Webhooks libraries or [`verify_standard`](crate::webhooks::verify_standard).
/// Waits at most [`DELIVERY_TIMEOUT`](crate::webhooks::DELIVERY_TIMEOUT)
/// and never follows a redirect (a 3xx is the answer). In release builds
/// the URL is checked first ([`check_public_url`]); debug builds deliver to
/// any host, so request tests can run a receiver on `127.0.0.1`.
///
/// Any status is an answer: retry on a non-2xx from a job. One subrequest,
/// plus two DNS lookups in release builds for a host name.
///
/// # Errors
///
/// [`Error::BadRequest`](crate::Error::BadRequest) when the URL is refused;
/// [`Error::Internal`](crate::Error::Internal) when `secret` is not a
/// `whsec_` secret, `payload` is not serializable, or the endpoint cannot
/// be reached or does not answer in time.
pub fn deliver(
    url: &str,
    secret: &str,
    id: &str,
    payload: &serde_json::Value,
) -> impl Future<Output = Result<crate::http::Response>> + Send + use<> {
    let body = serde_json::to_vec(payload).map_err(|err| Error::internal(format!("webhook payload: {err}")));
    let request = body.and_then(|body| crate::webhooks::delivery_request(url, secret, id, body, crate::now()));
    let url = url.to_owned();
    async move {
        let request = request?;
        if !Environment::current().is_development() {
            check_public_url(&url).await?;
        }
        request.send().await
    }
}

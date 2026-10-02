//! [`send`]: one web push message to one subscription.

use worker::{Fetch, Headers, Method, Request, RequestInit, send::SendFuture};

use super::Ctx;
use crate::{
    Error, Result,
    push::{Subscription, VAPID_PRIVATE_KEY, VAPID_PUBLIC_KEY, VAPID_SUBJECT, VapidKeys, encrypt, vapid_authorization},
};

/// What the push service did with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// Accepted (201): the browser gets it when it is online, within the TTL.
    Delivered,
    /// The subscription no longer exists (404 or 410: the user unsubscribed
    /// or the browser dropped it): delete it.
    Gone,
}

/// Encrypts `message` (JSON, e.g. [`message`](crate::push::message)) for
/// `subscription` and posts it to its push service, signed with the app's
/// VAPID keys. The push service keeps it up to `ttl` seconds while the
/// browser is offline. One subrequest.
///
/// # Errors
///
/// - [`Error::BadRequest`] when the subscription's keys are invalid or the
///   message is over [`MAX_PAYLOAD`](crate::push::MAX_PAYLOAD) bytes.
/// - [`Error::Internal`] when a `VAPID_*` setting is missing or invalid,
///   or the push service refuses the message (other than 404/410), with its
///   status and answer.
///
/// # Examples
///
/// ```no_run
/// use ocre::push::{self, Sent, Subscription};
/// use ocre::{Ctx, Result};
///
/// async fn notify(ctx: &Ctx, subscription: &Subscription) -> Result<()> {
///     let message = push::message("Your video is ready", "Download it now.", "/videos/42");
///     if push::send(ctx, subscription, &message, 24 * 3600).await? == Sent::Gone {
///         // delete the subscription
///     }
///     Ok(())
/// }
/// # let _ = notify;
/// ```
pub fn send(
    ctx: &Ctx,
    subscription: &Subscription,
    message: &serde_json::Value,
    ttl: u32,
) -> impl Future<Output = Result<Sent>> + Send + use<> {
    let env = ctx.env().clone();
    let subscription = subscription.clone();
    let payload = message.to_string();
    SendFuture::new(async move {
        let var = |name: &str| env.var(name).ok().map(|value| value.to_string()).filter(|v| !v.trim().is_empty());
        let missing = |name: &str| {
            Error::internal(format!(
                "web push needs {name} (not set). Fix: `ocre g push` writes a VAPID key pair to .dev.vars; in \
                 production set VAPID_PUBLIC_KEY and VAPID_SUBJECT in worker.env and push VAPID_PRIVATE_KEY with \
                 `ocre secrets push VAPID_PRIVATE_KEY --file .prod.vars`"
            ))
        };
        let private_key = super::secrets::lookup(&env, VAPID_PRIVATE_KEY).await?.filter(|v| !v.trim().is_empty());
        let keys = VapidKeys {
            public_key: var(VAPID_PUBLIC_KEY).ok_or_else(|| missing(VAPID_PUBLIC_KEY))?,
            private_key: private_key.ok_or_else(|| missing(VAPID_PRIVATE_KEY))?,
        };
        let subject = var(VAPID_SUBJECT).ok_or_else(|| missing(VAPID_SUBJECT))?;
        let body = encrypt(&subscription.keys, payload.as_bytes())?;
        let authorization = vapid_authorization(&subscription.endpoint, &subject, &keys, crate::now())?;
        let headers = Headers::new();
        headers.set("Authorization", &authorization)?;
        headers.set("Content-Encoding", "aes128gcm")?;
        headers.set("Content-Type", "application/octet-stream")?;
        headers.set("TTL", &ttl.to_string())?;
        let mut init = RequestInit::new();
        let array = worker::js_sys::Uint8Array::from(body.as_slice());
        init.with_method(Method::Post).with_headers(headers).with_body(Some(array.into()));
        let mut response = Fetch::Request(Request::new_with_init(&subscription.endpoint, &init)?).send().await?;
        match response.status_code() {
            200..=299 => Ok(Sent::Delivered),
            404 | 410 => Ok(Sent::Gone),
            status => {
                let text = response.text().await.unwrap_or_default();
                Err(Error::internal(format!(
                    "the push service answered {status}: {}",
                    text.chars().take(200).collect::<String>()
                )))
            }
        }
    })
}

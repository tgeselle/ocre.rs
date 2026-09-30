//! Sends the pending error reports and events of a request, job batch or
//! cron run to the registered subscribers (`ocre::errors`, `ocre::events`), with `fetch`.

use worker::{Env, Fetch, Headers, Method, Request, RequestInit, wasm_bindgen::JsValue};

use crate::errors::{Delivery, deliveries};

/// Delivers everything reported so far; a failed send is logged, not retried.
pub(crate) async fn flush(ctx: &super::Ctx) {
    let (reports, events) = (ctx.errors().take(), ctx.events().take());
    if reports.is_empty() && events.is_empty() {
        return;
    }
    let env = ctx.env();
    let vars = |name: &str| lookup(env, name);
    for (subscriber, delivery) in deliveries(&reports, &vars) {
        if let Err(err) = send(delivery).await {
            ctx.log().warn(format_args!("[ocre] error report to {subscriber} not sent: {err}"));
        }
    }
    for (subscriber, delivery) in crate::events::deliveries(&events, &vars) {
        if let Err(err) = send(delivery).await {
            ctx.log().warn(format_args!("[ocre] event to {subscriber} not sent: {err}"));
        }
    }
}

/// A Worker variable, else a secret, by name.
pub(crate) fn lookup(env: &Env, name: &str) -> Option<String> {
    env.var(name).ok().map(|var| var.to_string()).or_else(|| env.secret(name).ok().map(|secret| secret.to_string()))
}

async fn send(delivery: Delivery) -> worker::Result<()> {
    let headers = Headers::new();
    for (name, value) in &delivery.headers {
        headers.set(name, value)?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_headers(headers).with_body(Some(JsValue::from_str(&delivery.body)));
    let response = Fetch::Request(Request::new_with_init(&delivery.url, &init)?).send().await?;
    match response.status_code() {
        200..=299 => Ok(()),
        status => Err(worker::Error::RustError(format!("HTTP {status}"))),
    }
}

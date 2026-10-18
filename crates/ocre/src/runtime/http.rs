//! [`Request::send`]: an [`http::Request`](crate::http::Request) through the Workers `fetch`.

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use worker::{AbortSignal, Fetch, Headers, Method, Request as JsRequest, RequestInit, RequestRedirect, web_sys};

use crate::{
    Result,
    http::{Request, Response},
};

impl Request {
    /// Sends the request and returns the answer, whatever its status.
    ///
    /// The returned future is `Send`: await it in a handler, a job or a
    /// scheduled task. Only runs on Workers (natively it panics, like every
    /// binding); request tests reach it through the app.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](crate::Error::Internal) (500) when the body could
    /// not be serialized, the URL or a header is invalid, the service cannot
    /// be reached, or no answer came within the timeout.
    pub fn send(self) -> impl Future<Output = Result<Response>> + Send + use<> {
        send(self)
    }
}

/// Sends `request` with `fetch`, giving up after its timeout.
fn send(request: Request) -> impl Future<Output = Result<Response>> + Send + use<> {
    worker::send::SendFuture::new(async move {
        if let Some(reason) = request.invalid_reason() {
            return Err(request.error(reason));
        }
        let headers = Headers::new();
        for (name, value) in request.headers() {
            headers.set(name, value).map_err(|err| request.error(format!("header {name}: {err}")))?;
        }
        let method = Method::from(request.method().to_owned());
        let mut init = RequestInit::new();
        init.with_method(method).with_headers(headers).with_redirect(if request.follows_redirects() {
            RequestRedirect::Follow
        } else {
            RequestRedirect::Manual
        });
        if let Some(body) = request.body_bytes() {
            init.with_body(Some(worker::js_sys::Uint8Array::from(body).into()));
        }
        let js_request = JsRequest::new_with_init(request.url(), &init).map_err(|err| request.error(err))?;
        let millis = u32::try_from(request.timeout_duration().as_millis()).unwrap_or(u32::MAX);
        let signal = web_sys::AbortSignal::timeout_with_u32(millis);
        let signal = AbortSignal::from(signal);
        let mut response = match Fetch::Request(js_request).send_with_signal(&signal).await {
            Ok(response) => response,
            Err(_) if signal.aborted() => {
                return Err(request.error(format!("no answer within {} ms", request.timeout_duration().as_millis())));
            }
            Err(err) => return Err(request.error(err)),
        };
        let mut map = HeaderMap::new();
        for (name, value) in response.headers().entries() {
            if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::try_from(value)) {
                map.append(name, value);
            }
        }
        let status = response.status_code();
        let body = response.bytes().await.map_err(|err| request.error(format!("reading the body: {err}")))?;
        Ok(Response::of(&request, status, map, body))
    })
}

//! [`FakeServer`]: a stand-in for another service's API, on `127.0.0.1`.

use std::{
    collections::BTreeMap,
    io::{BufReader, Write as _},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

use serde::de::DeserializeOwned;
use serde_json::Value;

/// A request the [`FakeServer`] received.
///
/// # Examples
///
/// ```
/// use ocre::testing::FakeServer;
///
/// let server = FakeServer::start();
/// let _ = ureq::post(&format!("{}/hook?x=1", server.url())).header("X-Event", "ping").send("{\"a\":1}");
/// let request = server.requests().pop().unwrap();
/// assert_eq!((request.method.as_str(), request.path.as_str()), ("POST", "/hook?x=1"));
/// assert_eq!(request.header("x-event"), Some("ping"));
/// assert_eq!(request.json::<serde_json::Value>()["a"], 1);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedRequest {
    /// `GET`, `POST`...
    pub method: String,
    /// The path and query string (`/v1/jobs?limit=2`).
    pub path: String,
    /// Headers, lowercase names (the last value of a repeated header).
    pub headers: BTreeMap<String, String>,
    /// The body (chunked bodies decoded).
    pub body: Vec<u8>,
}

impl ReceivedRequest {
    /// The value of header `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    /// The body as text (invalid UTF-8 replaced).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body parsed as JSON.
    ///
    /// # Panics
    ///
    /// When the body is not JSON of type `T`.
    #[track_caller]
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap_or_else(|err| {
            panic!("{} {}: the body is not the JSON expected ({err}): {}", self.method, self.path, self.text())
        })
    }
}

/// An answer of the fake: status, content type and body.
#[derive(Debug, Clone)]
struct Answer {
    method: String,
    prefix: String,
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

#[derive(Debug, Default)]
struct State {
    answers: Vec<Answer>,
    requests: Vec<ReceivedRequest>,
}

/// A fake HTTP service on `127.0.0.1` for request tests: canned answers by method and path prefix, and every request it received.
///
/// `ocre test --e2e` runs the app in workerd on the same machine, so the
/// app can call it. Point the app at [`url`](Self::url) for one client's
/// requests with a test header ([`Client::header`](crate::testing::Client::header)
/// and [`http::test_override`](crate::http::test_override)), or a
/// webhook endpoint at it, then assert on [`requests`](Self::requests)
/// (wrap the check in [`eventually`](crate::testing::eventually) when a
/// job makes the call). A request no answer matches gets `404` with
/// `{"error": "no fake answer"}`. The server runs until the test process
/// ends; each request is served on its own thread.
///
/// # Examples
///
/// ```no_run
/// use ocre::{serde_json::json, testing::{Client, FakeServer, eventually}};
///
/// #[test]
/// #[ignore = "request test: run with `ocre test --e2e`"]
/// fn checkout_creates_an_invoice() {
///     let payments = FakeServer::start();
///     payments.answer_json("POST", "/invoices", 201, &json!({"id": "inv_1", "url": "https://pay.example/inv_1"}));
///     let mut client = Client::new().header("x-ocre-test-payments-api", payments.url());
///     client.post("/checkout", &[("plan", "pro")]).assert_redirect_to("https://pay.example/inv_1");
///     let invoice = eventually(|| payments.requests().into_iter().find(|r| r.path == "/invoices"));
///     assert_eq!(invoice.json::<ocre::serde_json::Value>()["amount"], 900);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct FakeServer {
    url: String,
    state: Arc<Mutex<State>>,
}

impl FakeServer {
    /// Starts the server on a free port of `127.0.0.1`.
    ///
    /// # Panics
    ///
    /// When no port can be bound.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("FakeServer: bind 127.0.0.1");
        let url = format!("http://{}", listener.local_addr().expect("FakeServer: local address"));
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &state));
            }
        });
        Self { url, state }
    }

    /// The base URL, `http://127.0.0.1:<port>`, without a trailing slash.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Answers `method` requests (`*` for any) whose path starts with `path_prefix` with `status` and `body` (`text/plain`).
    ///
    /// The newest matching answer wins, so a test can change an answer midway.
    pub fn answer(&self, method: &str, path_prefix: &str, status: u16, body: &str) -> &Self {
        self.add(method, path_prefix, status, "text/plain; charset=utf-8", body.as_bytes().to_vec())
    }

    /// [`answer`](Self::answer) with a JSON body (`application/json`).
    pub fn answer_json(&self, method: &str, path_prefix: &str, status: u16, value: &Value) -> &Self {
        self.add(method, path_prefix, status, "application/json", value.to_string().into_bytes())
    }

    fn add(&self, method: &str, prefix: &str, status: u16, content_type: &str, body: Vec<u8>) -> &Self {
        let answer = Answer {
            method: method.to_ascii_uppercase(),
            prefix: prefix.to_owned(),
            status,
            content_type: content_type.to_owned(),
            body,
        };
        self.state.lock().expect("FakeServer state").answers.push(answer);
        self
    }

    /// Every request received so far, oldest first.
    pub fn requests(&self) -> Vec<ReceivedRequest> {
        self.state.lock().expect("FakeServer state").requests.clone()
    }
}

/// Reads one request from `stream`, records it, and writes the answer.
fn serve(stream: TcpStream, state: &Mutex<State>) {
    let Ok(mut writer) = stream.try_clone() else { return };
    let Some(request) = read_request(BufReader::new(stream)) else { return };
    let answer = {
        let mut state = state.lock().expect("FakeServer state");
        let answer = state
            .answers
            .iter()
            .rev()
            .find(|answer| {
                (answer.method == "*" || answer.method == request.method) && request.path.starts_with(&answer.prefix)
            })
            .cloned();
        state.requests.push(request);
        answer
    };
    let answer = answer.unwrap_or_else(|| Answer {
        method: String::new(),
        prefix: String::new(),
        status: 404,
        content_type: "application/json".to_owned(),
        body: br#"{"error":"no fake answer"}"#.to_vec(),
    });
    let head = format!(
        "HTTP/1.1 {} Fake\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        answer.status,
        answer.content_type,
        answer.body.len()
    );
    let _ = writer.write_all(head.as_bytes()).and_then(|()| writer.write_all(&answer.body));
}

/// Parses an HTTP/1.1 request: request line, headers, then a `Content-Length` or chunked body.
pub(crate) fn read_request(mut reader: impl std::io::BufRead) -> Option<ReceivedRequest> {
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_owned(), parts.next()?.to_owned());
    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let mut body = Vec::new();
    if headers.get("transfer-encoding").is_some_and(|value| value.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).ok()?;
            let size = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
            let mut chunk = vec![0; size + 2]; // the chunk and its CRLF
            reader.read_exact(&mut chunk).ok()?;
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    } else if let Some(length) = headers.get("content-length").and_then(|value| value.parse::<usize>().ok()) {
        body.resize(length, 0);
        reader.read_exact(&mut body).ok()?;
    }
    Some(ReceivedRequest { method, path, headers, body })
}

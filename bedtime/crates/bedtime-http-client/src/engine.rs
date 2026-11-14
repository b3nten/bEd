//! Sending a request: what goes out (`Request`), how far to go (`Options`)
//! and what comes back (`Response`, or an `Error`).
//!
//! `send` blocks, but the exchange runs on a thread of its own: the caller
//! checks `cancelled` while it waits and returns as soon as it's true. The
//! abandoned exchange stops at its next read or when it times out.
use std::{
    fmt,
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use ureq::{Agent, ResponseExt, http};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Method {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    pub const ALL: [Method; 7] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One request, as sent. Headers go out in order, as written; an empty body
/// sends none (no `Content-Length`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    /// Without a scheme (`localhost:3000/users`), `http://` is assumed.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// For the whole exchange: connecting, sending and reading the response.
    pub timeout: Duration,
    /// Bodies longer than this are cut short (`Response::truncated`).
    pub max_body: usize,
    pub follow_redirects: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body: 16 * 1024 * 1024,
            follow_redirects: true,
        }
    }
}

/// What came back. Any status is a response, not an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// The status's standard reason (`Not Found`), if it has one.
    pub reason: Option<&'static str>,
    /// Where the response came from, after any redirects.
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Decompressed if the server gzipped it.
    pub body: Vec<u8>,
    /// The body was longer than `Options::max_body` and was cut short.
    pub truncated: bool,
    /// From sending until the whole body was read.
    pub elapsed: Duration,
}

impl Response {
    /// The first header named `name`, ignoring case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// `cancelled` said to stop.
    Cancelled,
    /// The request couldn't be made: a bad URL or header.
    Invalid(String),
    TimedOut,
    /// Connecting, TLS, or the exchange itself failed.
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Cancelled => f.write_str("Cancelled"),
            Error::Invalid(why) | Error::Failed(why) => f.write_str(why),
            Error::TimedOut => f.write_str("Timed out"),
        }
    }
}

impl std::error::Error for Error {}

/// How often a waiting `send` checks `cancelled`.
const POLL: Duration = Duration::from_millis(25);
/// Bodies are read this much at a time, checking for cancellation between.
const CHUNK: usize = 64 * 1024;

/// Send `request` and read the response. Returns `Error::Cancelled` soon after
/// `cancelled` becomes true.
pub fn send(
    request: &Request,
    options: &Options,
    cancelled: impl Fn() -> bool,
) -> Result<Response, Error> {
    let prepared = prepare(request)?;
    let agent = agent(options);
    let max_body = options.max_body;
    let abandoned = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::sync_channel(1);
    let stop = abandoned.clone();
    thread::Builder::new()
        .name("http-request".into())
        .spawn(move || {
            let _ = sender.send(exchange(&agent, prepared, max_body, &stop));
        })
        .map_err(|e| Error::Failed(e.to_string()))?;
    loop {
        if cancelled() {
            abandoned.store(true, Ordering::Release);
            return Err(Error::Cancelled);
        }
        match receiver.recv_timeout(POLL) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::Failed("The request stopped unexpectedly".into()));
            }
        }
    }
}

fn agent(options: &Options) -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(options.timeout))
        .max_redirects(if options.follow_redirects { 10 } else { 0 })
        // Not following redirects: the redirect is the response.
        .max_redirects_will_error(options.follow_redirects)
        .user_agent(concat!("bedtime-http-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// `request` as an `http::Request`, or why it can't be one.
fn prepare(request: &Request) -> Result<http::Request<Vec<u8>>, Error> {
    let url = request.url.trim();
    if url.is_empty() {
        return Err(Error::Invalid("The URL is empty".into()));
    }
    let url = if url.contains("://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    };
    let uri: http::Uri = url
        .parse()
        .map_err(|e| Error::Invalid(format!("Invalid URL: {e}")))?;
    let mut builder = http::Request::builder()
        .method(request.method.as_str())
        .uri(uri);
    for (name, value) in &request.headers {
        let name = http::HeaderName::from_bytes(name.trim().as_bytes())
            .map_err(|_| Error::Invalid(format!("Invalid header name: {name:?}")))?;
        let value = http::HeaderValue::from_str(value.trim())
            .map_err(|_| Error::Invalid(format!("Invalid value for header {name}")))?;
        builder = builder.header(name, value);
    }
    builder
        .body(request.body.clone())
        .map_err(|e| Error::Invalid(e.to_string()))
}

/// Make the request and read the response. Runs on the request's thread.
fn exchange(
    agent: &Agent,
    request: http::Request<Vec<u8>>,
    max_body: usize,
    abandoned: &AtomicBool,
) -> Result<Response, Error> {
    let started = Instant::now();
    let (parts, body) = request.into_parts();
    // An empty body is no body: `()` sends no Content-Length.
    let response = if body.is_empty() {
        agent.run(http::Request::from_parts(parts, ()))
    } else {
        agent.run(http::Request::from_parts(parts, body.as_slice()))
    }
    .map_err(error)?;

    let status = response.status();
    let url = response.get_uri().to_string();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
            (name.as_str().to_owned(), value)
        })
        .collect();

    let mut reader = response
        .into_body()
        .into_with_config()
        .limit(u64::MAX)
        .reader();
    let mut body = Vec::new();
    let mut chunk = vec![0; CHUNK];
    let mut truncated = false;
    loop {
        if abandoned.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let read = reader
            .read(&mut chunk)
            .map_err(|e| error(ureq::Error::from(e)))?;
        if read == 0 {
            break;
        }
        let room = max_body - body.len();
        body.extend_from_slice(&chunk[..read.min(room)]);
        if read > room {
            truncated = true;
            break;
        }
    }

    Ok(Response {
        status: status.as_u16(),
        reason: status.canonical_reason(),
        url,
        headers,
        body,
        truncated,
        elapsed: started.elapsed(),
    })
}

fn error(error: ureq::Error) -> Error {
    match error {
        ureq::Error::Timeout(_) => Error::TimedOut,
        ureq::Error::BadUri(why) => Error::Invalid(format!("Invalid URL: {why}")),
        ureq::Error::Http(e) => Error::Invalid(e.to_string()),
        ureq::Error::HostNotFound => Error::Failed("Host not found".into()),
        ureq::Error::ConnectionFailed => Error::Failed("Couldn't connect".into()),
        ureq::Error::TooManyRedirects => Error::Failed("Too many redirects".into()),
        ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            Error::Failed("Connection refused".into())
        }
        ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => Error::TimedOut,
        other => Error::Failed(other.to_string()),
    }
}

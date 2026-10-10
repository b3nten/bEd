//! The only network execution boundary for the HTTP module. The UI sends runs
//! and cancellations; the worker returns complete values and never owns UI state.

use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use tokio::{sync::mpsc as async_mpsc, task::JoinSet};

use crate::model::ResolvedRequest;

const TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const BODY_LIMIT: usize = 8 * 1024 * 1024;
const HEX_LIMIT: usize = 4 * 1024;

#[derive(Debug)]
pub(crate) struct Completion {
    pub run: u64,
    pub result: Result<Response, String>,
}

#[derive(Debug)]
pub(crate) struct Response {
    pub status: u16,
    pub version: String,
    pub final_url: String,
    pub elapsed: Duration,
    pub headers: Vec<(String, String)>,
    /// Collected, decompressed bytes; never exceeds BODY_LIMIT.
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub presentation: BodyPresentation,
}

#[derive(Debug)]
pub(crate) enum BodyPresentation {
    Text {
        raw: String,
        pretty_json: Option<String>,
    },
    Binary {
        hex: String,
    },
}

enum Command {
    Run(u64, ResolvedRequest),
    Cancel(u64),
    Shutdown,
}

pub(crate) struct Worker {
    commands: Option<async_mpsc::UnboundedSender<Command>>,
    completions: mpsc::Receiver<Completion>,
    completion_sender: mpsc::Sender<Completion>,
    timeout: Duration,
}

impl Worker {
    pub fn new() -> Self {
        let (completion_sender, completions) = mpsc::channel();
        Self {
            commands: None,
            completions,
            completion_sender,
            timeout: TIMEOUT,
        }
    }

    /// Starting the first run starts the executor. Constructing, restoring or
    /// drawing a viewer never initializes a client or sends a request.
    pub fn start(&mut self, run: u64, request: ResolvedRequest) -> Result<(), String> {
        if self.commands.is_none() {
            let (sender, receiver) = async_mpsc::unbounded_channel();
            let completions = self.completion_sender.clone();
            let timeout = self.timeout;
            thread::Builder::new()
                .name("bed-http".into())
                .spawn(move || execute(receiver, completions, timeout))
                .map_err(|error| format!("Could not start HTTP executor: {error}"))?;
            self.commands = Some(sender);
        }
        self.commands
            .as_ref()
            .expect("executor was started")
            .send(Command::Run(run, request))
            .map_err(|_| "HTTP executor stopped unexpectedly".into())
    }

    pub fn cancel(&self, run: u64) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::Cancel(run));
        }
    }

    pub fn poll(&self) -> Vec<Completion> {
        self.completions.try_iter().collect()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(Command::Shutdown);
        }
        // The thread aborts network tasks itself. Never join an executor from
        // the UI thread, including while a resolver is finishing blocking work.
    }
}

fn execute(
    mut commands: async_mpsc::UnboundedReceiver<Command>,
    completions: mpsc::Sender<Completion>,
    timeout: Duration,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            // Keep the command boundary alive so every submitted run gets an
            // error rather than leaving its viewer permanently in Running.
            while let Some(command) = commands.blocking_recv() {
                match command {
                    Command::Run(run, _) => {
                        let _ = completions.send(Completion {
                            run,
                            result: Err(format!("Could not initialize HTTP executor: {error}")),
                        });
                    }
                    Command::Shutdown => break,
                    Command::Cancel(_) => {}
                }
            }
            return;
        }
    };
    runtime.block_on(async {
        let (client_sender, client) = tokio::sync::watch::channel(None::<Result<reqwest::Client, String>>);
        // One independent setup owner prevents a cancelled or timed-out run
        // from starting additional OS lookups when discovery is stalled.
        tokio::task::spawn_blocking(move || {
            let builder = reqwest::Client::builder()
                .timeout(timeout)
                .redirect(reqwest::redirect::Policy::limited(10))
                .retry(reqwest::retry::never());
            // Loopback tests must not depend on the machine's proxy settings
            // or OS services. Production retains system proxies and verified
            // TLS. No cookie store is installed in either case.
            #[cfg(test)]
            let builder = builder.no_proxy();
            let initialized = builder.build().map_err(|error|
                format!("Could not initialize HTTP client: {error}"));
            let _ = client_sender.send(Some(initialized));
        });
        let mut tasks = JoinSet::new();
        let mut running = HashMap::<u64, tokio::task::AbortHandle>::new();
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Run(run, request)) => {
                        let mut client = client.clone();
                        let abort = tasks.spawn(async move {
                            // OS proxy discovery can block before reqwest's own
                            // deadline begins. Keep it outside the async loop
                            // and include initialization in the run's timeout.
                            let result = tokio::time::timeout(timeout, async {
                                let client = {
                                    let initialized = client.wait_for(Option::is_some).await
                                        .map_err(|_| "HTTP client initialization stopped unexpectedly".to_owned())?;
                                    initialized.as_ref().expect("client initialization completed").clone()
                                };
                                match client {
                                    Ok(client) => send_request(&client, request, timeout).await,
                                    Err(error) => Err(error),
                                }
                            }).await.unwrap_or_else(|_| Err(format!(
                                "Request timed out after {} seconds", timeout.as_secs_f64())));
                            (run, result)
                        });
                        assert!(running.insert(run, abort).is_none(), "HTTP run identifiers must be unique");
                    },
                    Some(Command::Cancel(run)) => {
                        if let Some(task) = running.remove(&run) {
                            task.abort();
                            let _ = completions.send(Completion { run, result: Err("Request cancelled".into()) });
                        }
                    }
                    Some(Command::Shutdown) | None => break,
                },
                completed = tasks.join_next(), if !tasks.is_empty() => match completed {
                    Some(Ok((run, result))) => {
                        // A completed task can still be awaiting collection
                        // when Cancel arrives; its cancelled result wins.
                        if running.remove(&run).is_some() {
                            let _ = completions.send(Completion { run, result });
                        }
                    }
                    Some(Err(error)) => {
                        let run = running.iter()
                            .find_map(|(&run, task)| (task.id() == error.id()).then_some(run));
                        if let Some(run) = run {
                            running.remove(&run);
                            let _ = completions.send(Completion {
                                run,
                                result: Err(format!("HTTP executor task failed: {error}")),
                            });
                        }
                    }
                    None => {}
                },
            }
        }
        tasks.abort_all();
    });
    runtime.shutdown_timeout(Duration::from_millis(100));
}

async fn send_request(
    client: &reqwest::Client,
    request: ResolvedRequest,
    timeout: Duration,
) -> Result<Response, String> {
    let started = Instant::now();
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|error| format!("Invalid HTTP method: {error}"))?;
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in &request.headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("Invalid header name {name:?}: {error}"))?;
        let value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|error| format!("Invalid value for header {name}: {error}"))?;
        headers.append(name, value);
    }
    let mut response = client
        .request(method, &request.url)
        .headers(headers)
        .body(request.body)
        .send()
        .await
        .map_err(|error| network_error(error, timeout))?;
    let status = response.status().as_u16();
    let version = format!("{:?}", response.version());
    let final_url = response.url().to_string();
    let headers: Vec<_> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let mut bytes = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| network_error(error, timeout))?
    {
        let remaining = BODY_LIMIT - bytes.len();
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if chunk.len() > remaining {
            truncated = true;
            break;
        }
        // Read once more when the body exactly fills the limit. This preserves
        // the distinction between a complete 8 MiB body and a truncated body.
    }
    let elapsed = started.elapsed();
    let presentation = present_body(&bytes, &content_type, truncated);
    Ok(Response {
        status,
        version,
        final_url,
        elapsed,
        headers,
        bytes,
        truncated,
        presentation,
    })
}

fn network_error(error: reqwest::Error, timeout: Duration) -> String {
    if error.is_timeout() {
        return format!("Request timed out after {} seconds", timeout.as_secs_f64());
    }
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        let _ = write!(message, ": {cause}");
        source = cause.source();
    }
    message
}

fn present_body(bytes: &[u8], content_type: &str, truncated: bool) -> BodyPresentation {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let textual = media_type.starts_with("text/")
        || media_type.ends_with("+json")
        || media_type.ends_with("+xml")
        || matches!(
            media_type.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-www-form-urlencoded"
        );
    let utf8_text = std::str::from_utf8(bytes).ok().filter(|text| {
        !text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    });
    if textual || (media_type.is_empty() && utf8_text.is_some()) {
        let raw = String::from_utf8_lossy(bytes).into_owned();
        let pretty_json = if truncated {
            None
        } else {
            serde_json::from_slice::<serde_json::Value>(bytes)
                .ok()
                .and_then(|value| {
                    let mut output = BoundedJson(Vec::new());
                    serde_json::to_writer_pretty(&mut output, &value).ok()?;
                    Some(String::from_utf8(output.0).expect("JSON serialization is UTF-8"))
                })
        };
        return BodyPresentation::Text { raw, pretty_json };
    }
    let mut hex = String::new();
    for (line, chunk) in bytes[..bytes.len().min(HEX_LIMIT)].chunks(16).enumerate() {
        let _ = write!(hex, "{:08x}  ", line * 16);
        for index in 0..16 {
            if let Some(byte) = chunk.get(index) {
                let _ = write!(hex, "{byte:02x} ");
            } else {
                hex.push_str("   ");
            }
        }
        hex.push_str(" |");
        for &byte in chunk {
            hex.push(if byte.is_ascii_graphic() || byte == b' ' {
                char::from(byte)
            } else {
                '.'
            });
        }
        hex.push_str("|\n");
    }
    if bytes.len() > HEX_LIMIT {
        hex.push_str("… preview limited to the first 4 KiB\n");
    }
    BodyPresentation::Binary { hex }
}

/// Pretty printing can expand deeply nested JSON substantially. Bound the
/// derived text as well as the downloaded bytes and fall back to raw text.
struct BoundedJson(Vec<u8>);

impl std::io::Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > BODY_LIMIT * 2 {
            return Err(std::io::Error::other(
                "formatted JSON exceeds display limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
    };

    fn request(url: String) -> ResolvedRequest {
        ResolvedRequest {
            method: "GET".into(),
            url,
            headers: vec![],
            body: String::new(),
        }
    }

    fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "connection ended before complete request");
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_text = String::from_utf8_lossy(&bytes[..end]);
                let length = header_text
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    return bytes;
                }
            }
        }
    }

    fn server(
        responses: Vec<Vec<u8>>,
    ) -> (String, mpsc::Receiver<Vec<u8>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let bytes = read_request(&mut stream);
                sender.send(bytes).unwrap();
                // Collection may intentionally close a response at BODY_LIMIT.
                let _ = stream.write_all(&response);
            }
        });
        (url, receiver, thread)
    }

    fn completed(worker: &Worker, run: u64) -> Result<Response, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(completion) = worker.poll().into_iter().next() {
                assert_eq!(completion.run, run);
                return completion.result;
            }
            assert!(Instant::now() < deadline, "HTTP run did not complete");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn sends_headers_and_body_and_keeps_http_error_responses() {
        let (url, received, server) = server(vec![response(
            "422 Unprocessable Entity",
            "Content-Type: application/json\r\nX-Result: first\r\nX-Result: second\r\n",
            br#"{"error":"invalid"}"#,
        )]);
        let mut worker = Worker::new();
        let mut input = request(format!("{url}/submit?q=hello"));
        input.method = "POST".into();
        input.headers = vec![
            ("Authorization".into(), "Bearer token".into()),
            ("X-Repeated".into(), "one".into()),
            ("X-Repeated".into(), "two".into()),
        ];
        input.body = "first\r\nsecond\n".into();
        worker.start(1, input).unwrap();
        let result = completed(&worker, 1).unwrap();
        assert_eq!(result.status, 422);
        assert_eq!(result.version, "HTTP/1.1");
        assert_eq!(
            result
                .headers
                .iter()
                .filter(|(name, _)| name == "x-result")
                .count(),
            2
        );
        assert_eq!(result.bytes, br#"{"error":"invalid"}"#);
        assert!(!result.truncated);
        let BodyPresentation::Text { raw, pretty_json } = result.presentation else {
            panic!("expected text")
        };
        assert_eq!(raw, r#"{"error":"invalid"}"#);
        assert!(pretty_json.unwrap().contains("\n  \"error\""));
        let sent =
            String::from_utf8(received.recv_timeout(Duration::from_secs(3)).unwrap()).unwrap();
        assert!(sent.starts_with("POST /submit?q=hello HTTP/1.1\r\n"));
        assert!(sent.contains("authorization: Bearer token\r\n"));
        assert!(sent.contains("x-repeated: one\r\n"));
        assert!(sent.contains("x-repeated: two\r\n"));
        assert!(sent.ends_with("first\r\nsecond\n"));
        server.join().unwrap();
    }

    #[test]
    fn follows_redirects_and_reports_final_url_without_remembering_cookies() {
        let (url, received, server) = server(vec![
            response(
                "302 Found",
                "Location: /final\r\nSet-Cookie: session=secret\r\n",
                b"",
            ),
            response("200 OK", "Content-Type: text/plain\r\n", b"done"),
        ]);
        let mut worker = Worker::new();
        worker.start(2, request(format!("{url}/start"))).unwrap();
        let result = completed(&worker, 2).unwrap();
        assert_eq!(result.final_url, format!("{url}/final"));
        assert_eq!(result.bytes, b"done");
        let first = received.recv_timeout(Duration::from_secs(3)).unwrap();
        let second = received.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(first.starts_with(b"GET /start "));
        assert!(second.starts_with(b"GET /final "));
        assert!(
            !String::from_utf8_lossy(&second)
                .to_ascii_lowercase()
                .contains("cookie:")
        );
        server.join().unwrap();
    }

    #[test]
    fn caps_response_collection_and_binary_preview() {
        let body = vec![0x89; BODY_LIMIT + 100];
        let (url, _received, server) = server(vec![response(
            "200 OK",
            "Content-Type: application/octet-stream\r\n",
            &body,
        )]);
        let mut worker = Worker::new();
        worker.start(3, request(url)).unwrap();
        let result = completed(&worker, 3).unwrap();
        assert_eq!(result.bytes.len(), BODY_LIMIT);
        assert!(result.truncated);
        let BodyPresentation::Binary { hex } = result.presentation else {
            panic!("expected binary")
        };
        assert!(hex.contains("00000ff0"));
        assert!(!hex.contains("00001000"));
        assert!(hex.contains("first 4 KiB"));
        server.join().unwrap();
    }

    #[test]
    fn body_at_exact_limit_is_complete_and_invalid_json_remains_text() {
        let body = vec![b'x'; BODY_LIMIT];
        let (url, _received, server) = server(vec![response(
            "200 OK",
            "Content-Type: application/json\r\n",
            &body,
        )]);
        let mut worker = Worker::new();
        worker.start(4, request(url)).unwrap();
        let result = completed(&worker, 4).unwrap();
        assert!(!result.truncated);
        let BodyPresentation::Text { raw, pretty_json } = result.presentation else {
            panic!("expected text")
        };
        assert_eq!(raw.len(), BODY_LIMIT);
        assert!(pretty_json.is_none());
        server.join().unwrap();
    }

    #[test]
    fn cancellation_does_not_block_other_runs_and_drop_does_not_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stalled_url = format!("http://{}", listener.local_addr().unwrap());
        let (accepted, accepted_rx) = mpsc::channel();
        let stalled = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                accepted.send(()).unwrap();
                // Both Cancel and dropping the worker must close the active
                // request, rather than waiting for its 30-second deadline.
                assert_eq!(stream.read(&mut [0]).unwrap(), 0);
            }
        });
        let mut worker = Worker::new();
        worker.start(5, request(stalled_url.clone())).unwrap();
        accepted_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let (url, _received, server) = server(vec![response(
            "200 OK",
            "Content-Type: text/plain\r\n",
            b"parallel",
        )]);
        worker.start(6, request(url)).unwrap();
        assert_eq!(completed(&worker, 6).unwrap().bytes, b"parallel");
        worker.cancel(5);
        assert_eq!(completed(&worker, 5).unwrap_err(), "Request cancelled");
        worker.start(9, request(stalled_url)).unwrap();
        accepted_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let before_drop = Instant::now();
        drop(worker);
        assert!(before_drop.elapsed() < Duration::from_millis(100));
        stalled.join().unwrap();
        server.join().unwrap();
    }

    #[test]
    fn times_out_stalled_responses_and_keeps_executor_usable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (release, release_rx) = mpsc::channel();
        let stalled = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial")
                .unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(3));
        });
        let mut worker = Worker::new();
        worker.timeout = Duration::from_millis(150);
        worker.start(7, request(url)).unwrap();
        assert!(completed(&worker, 7).unwrap_err().contains("timed out"));
        release.send(()).unwrap();
        stalled.join().unwrap();
        let (url, _received, server) = server(vec![response(
            "200 OK",
            "Content-Type: text/plain\r\n",
            b"recovered",
        )]);
        worker.start(8, request(url)).unwrap();
        assert_eq!(completed(&worker, 8).unwrap().bytes, b"recovered");
        server.join().unwrap();
    }
}

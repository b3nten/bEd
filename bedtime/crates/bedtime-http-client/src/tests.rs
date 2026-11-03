use crate::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

/// A request as the server read it.
struct Received {
    head: String,
    body: Vec<u8>,
}

impl Received {
    fn request_line(&self) -> &str {
        self.head.lines().next().unwrap()
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().skip(1).find_map(|line| {
            let (n, v) = line.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }
}

/// A server on a free local port that answers each connection with the next
/// of `replies` (raw HTTP), and reports each request it read.
fn serve(replies: Vec<Vec<u8>>) -> (String, mpsc::Receiver<Received>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = sender.send(read_request(&mut stream));
            let _ = stream.write_all(&reply);
        }
    });
    (address, receiver)
}

fn read_request(stream: &mut TcpStream) -> Received {
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        head.push_str(&line);
    }
    let received = Received {
        head,
        body: Vec::new(),
    };
    let length: usize = received
        .header("content-length")
        .map_or(0, |n| n.parse().unwrap());
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Received { body, ..received }
}

fn reply(status: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for header in headers {
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

fn get(url: &str) -> Request {
    Request {
        url: url.into(),
        ..Default::default()
    }
}

fn never() -> bool {
    false
}

#[test]
fn get_reads_status_headers_and_body() {
    let (url, received) = serve(vec![reply(
        "200 OK",
        &["Content-Type: application/json", "X-Thing: one"],
        br#"{"ok":true}"#,
    )]);
    let request = Request {
        headers: vec![("Accept".into(), "application/json".into())],
        ..get(&format!("{url}/users?limit=10"))
    };
    let response = send(&request, &Options::default(), never).unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(response.reason, Some("OK"));
    assert_eq!(response.header("content-type"), Some("application/json"));
    assert_eq!(response.header("x-thing"), Some("one"));
    assert_eq!(response.body, br#"{"ok":true}"#);
    assert!(!response.truncated);
    assert_eq!(response.url, format!("{url}/users?limit=10"));

    let received = received.recv().unwrap();
    assert_eq!(received.request_line(), "GET /users?limit=10 HTTP/1.1");
    assert_eq!(received.header("accept"), Some("application/json"));
    assert!(
        received
            .header("user-agent")
            .unwrap()
            .starts_with("bedtime-http-client/")
    );
    assert_eq!(received.header("content-length"), None);
}

#[test]
fn post_sends_the_body() {
    let (url, received) = serve(vec![reply("201 Created", &[], b"")]);
    let request = Request {
        method: Method::Post,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: br#"{"name":"bed"}"#.to_vec(),
        ..get(&url)
    };
    let response = send(&request, &Options::default(), never).unwrap();
    assert_eq!(response.status, 201);

    let received = received.recv().unwrap();
    assert_eq!(received.request_line(), "POST / HTTP/1.1");
    assert_eq!(received.header("content-type"), Some("application/json"));
    assert_eq!(received.body, br#"{"name":"bed"}"#);
}

#[test]
fn a_get_can_have_a_body() {
    let (url, received) = serve(vec![reply("200 OK", &[], b"")]);
    let request = Request {
        body: b"query".to_vec(),
        ..get(&url)
    };
    send(&request, &Options::default(), never).unwrap();
    assert_eq!(received.recv().unwrap().body, b"query");
}

#[test]
fn a_user_agent_header_replaces_the_default() {
    let (url, received) = serve(vec![reply("200 OK", &[], b"")]);
    let request = Request {
        headers: vec![("User-Agent".into(), "mine".into())],
        ..get(&url)
    };
    send(&request, &Options::default(), never).unwrap();
    let received = received.recv().unwrap();
    assert_eq!(received.header("user-agent"), Some("mine"));
    assert_eq!(received.head.matches("user-agent").count(), 1);
}

#[test]
fn an_error_status_is_a_response() {
    let (url, _received) = serve(vec![reply("404 Not Found", &[], b"nope")]);
    let response = send(&get(&url), &Options::default(), never).unwrap();
    assert_eq!(response.status, 404);
    assert_eq!(response.reason, Some("Not Found"));
    assert_eq!(response.body, b"nope");
}

#[test]
fn a_url_without_a_scheme_is_http() {
    let (url, received) = serve(vec![reply("200 OK", &[], b"")]);
    let bare = url.strip_prefix("http://").unwrap();
    let response = send(&get(&format!("{bare}/x")), &Options::default(), never).unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(received.recv().unwrap().request_line(), "GET /x HTTP/1.1");
}

#[test]
fn redirects_are_followed_unless_turned_off() {
    let (url, _) = serve(vec![
        reply("302 Found", &["Location: /there"], b""),
        reply("200 OK", &[], b"arrived"),
        reply("302 Found", &["Location: /there"], b""),
    ]);
    let response = send(&get(&url), &Options::default(), never).unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"arrived");
    assert_eq!(response.url, format!("{url}/there"));

    let options = Options {
        follow_redirects: false,
        ..Default::default()
    };
    let response = send(&get(&url), &options, never).unwrap();
    assert_eq!(response.status, 302);
    assert_eq!(response.header("location"), Some("/there"));
}

#[test]
fn a_long_body_is_cut_short() {
    let (url, _) = serve(vec![reply("200 OK", &[], &[b'x'; 1000])]);
    let options = Options {
        max_body: 100,
        ..Default::default()
    };
    let response = send(&get(&url), &options, never).unwrap();
    assert_eq!(response.body.len(), 100);
    assert!(response.truncated);
}

#[test]
fn a_body_exactly_at_the_limit_is_whole() {
    let (url, _) = serve(vec![reply("200 OK", &[], &[b'x'; 100])]);
    let options = Options {
        max_body: 100,
        ..Default::default()
    };
    let response = send(&get(&url), &options, never).unwrap();
    assert_eq!(response.body.len(), 100);
    assert!(!response.truncated);
}

/// A server that accepts a connection and never answers.
fn stall() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_secs(10));
        drop(stream);
    });
    url
}

#[test]
fn cancelling_returns_promptly() {
    let url = stall();
    let started = Instant::now();
    let result = send(&get(&url), &Options::default(), || {
        started.elapsed() > Duration::from_millis(100)
    });
    assert_eq!(result, Err(Error::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_silent_server_times_out() {
    let url = stall();
    let options = Options {
        timeout: Duration::from_millis(200),
        ..Default::default()
    };
    assert_eq!(send(&get(&url), &options, never), Err(Error::TimedOut));
}

#[test]
fn a_closed_port_fails() {
    // Bind and drop to find a port nothing listens on.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let result = send(
        &get(&format!("http://127.0.0.1:{port}")),
        &Options::default(),
        never,
    );
    assert!(matches!(result, Err(Error::Failed(_))), "{result:?}");
}

#[test]
fn bad_requests_are_invalid() {
    let invalid = |request: Request| {
        let result = send(&request, &Options::default(), never);
        assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
    };
    invalid(get(""));
    invalid(get("http://exa mple.com"));
    invalid(Request {
        headers: vec![("Bad Name".into(), "x".into())],
        ..get("http://localhost")
    });
    invalid(Request {
        headers: vec![("X".into(), "line\nbreak".into())],
        ..get("http://localhost")
    });
}

/// Needs the network: `cargo test -p bedtime-http-client -- --ignored`.
#[test]
#[ignore]
fn https_works() {
    let response = send(&get("https://example.com"), &Options::default(), never).unwrap();
    assert_eq!(response.status, 200);
    assert!(!response.body.is_empty());
}

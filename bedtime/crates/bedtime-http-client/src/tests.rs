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

// Saved requests.

fn variables() -> Vec<Field> {
    vec![
        Field::new("base", "http://localhost:3000"),
        Field::new("token", "secret"),
        Field {
            enabled: false,
            ..Field::new("off", "nope")
        },
    ]
}

#[test]
fn variables_are_filled_in() {
    let filled = substitute("{{base}}/users?t={{ token }}", &variables()).unwrap();
    assert_eq!(filled, "http://localhost:3000/users?t=secret");
    // Unclosed braces are left alone.
    assert_eq!(substitute("a {{b", &variables()).unwrap(), "a {{b");
}

#[test]
fn an_unknown_or_disabled_variable_is_an_error() {
    assert_eq!(
        substitute("{{nope}}", &variables()).unwrap_err(),
        "Unknown variable {{nope}}"
    );
    assert!(substitute("{{off}}", &variables()).is_err());
}

#[test]
fn params_are_added_to_the_query_encoded() {
    let params = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    };
    assert_eq!(
        with_query("example.com/s", &params(&[("q", "a b&c"), ("n", "1")])),
        "example.com/s?q=a%20b%26c&n=1"
    );
    assert_eq!(
        with_query("example.com/s?x=1#top", &params(&[("y", "é")])),
        "example.com/s?x=1&y=%C3%A9#top"
    );
    assert_eq!(
        with_query("example.com/s?", &params(&[("y", "2")])),
        "example.com/s?y=2"
    );
    assert_eq!(with_query("example.com", &[]), "example.com");
}

#[test]
fn a_saved_request_resolves_with_only_its_enabled_fields() {
    let request = SavedRequest {
        method: Method::Post,
        url: " {{base}}/users ".into(),
        params: vec![
            Field::new("page", "2"),
            Field {
                enabled: false,
                ..Field::new("skip", "me")
            },
            Field::new("", "unnamed"),
        ],
        headers: vec![Field::new("Authorization", "Bearer {{token}}")],
        body: r#"{"token":"{{token}}"}"#.into(),
        ..Default::default()
    };
    let resolved = request.resolve(&variables()).unwrap();
    assert_eq!(resolved.method, Method::Post);
    assert_eq!(resolved.url, "http://localhost:3000/users?page=2");
    assert_eq!(
        resolved.headers,
        vec![("Authorization".to_owned(), "Bearer secret".to_owned())]
    );
    assert_eq!(resolved.body, br#"{"token":"secret"}"#);
}

#[test]
fn requests_are_added_duplicated_and_removed() {
    let mut collection = Collection::default();
    let a = collection.add(SavedRequest {
        name: "A".into(),
        ..Default::default()
    });
    let b = collection.add(SavedRequest::default());
    assert_ne!(a, b);
    assert_eq!(collection.get(b).unwrap().label(), "Untitled");

    let copy = collection.duplicate(a).unwrap();
    let names: Vec<_> = collection.requests.iter().map(|r| r.label()).collect();
    assert_eq!(names, ["A", "A copy", "Untitled"]);

    // Removing selects what takes its place, else the one before.
    assert_eq!(collection.remove(copy), Some(b));
    assert_eq!(collection.remove(b), Some(a));
    assert_eq!(collection.remove(a), None);
    assert!(collection.requests.is_empty());
}

#[test]
fn json_is_indented_keeping_order_and_strings() {
    let body = br#"{"z":1,"a":[1,2.50,{}],"s":"a, {b}: \"c\"","e":[]}"#;
    assert_eq!(
        pretty_json(body).unwrap(),
        "{\n  \"z\": 1,\n  \"a\": [\n    1,\n    2.50,\n    {}\n  ],\n  \"s\": \"a, {b}: \\\"c\\\"\",\n  \"e\": []\n}"
    );
    assert_eq!(pretty_json(b"not json"), None);
    assert_eq!(pretty_json(b"42").unwrap(), "42");
}

// The panel.

#[cfg(feature = "workbench")]
mod panel {
    use super::*;
    use crate::panel::tests::{TestIntent, intend, settle};
    use bedtime_persistence::{MemoryStore, read_persisted, write_persisted};
    use bedtime_workbench::{InArea, LayoutsWorldExt, WorkbenchCorePlugins};
    use bevy::prelude::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((WorkbenchCorePlugins, HttpClientPlugin));
        app.update();
        app
    }

    fn add(app: &mut App, request: SavedRequest) -> u64 {
        app.world_mut()
            .resource_mut::<HttpRequests>()
            .0
            .add(request)
    }

    /// A panel in a layout of its own.
    fn panel(app: &mut App) -> Entity {
        let world = app.world_mut();
        let layout = world.layouts_mut().create("Test").unwrap();
        let area = world.layouts().area_for(layout, |_, _| false).unwrap();
        world.spawn((HttpClientPanel::default(), InArea(area))).id()
    }

    fn sent(app: &mut App, id: u64) -> Entity {
        let panel = panel(app);
        intend(app, panel, TestIntent::Send(id));
        panel
    }

    #[test]
    fn requests_round_trip_through_the_store() {
        let store = MemoryStore::default();
        let mut requests = HttpRequests::default();
        requests.0.add(SavedRequest {
            name: "Users".into(),
            method: Method::Delete,
            url: "{{base}}/users".into(),
            headers: vec![Field::new("Accept", "application/json")],
            ..Default::default()
        });
        requests.0.variables = variables();
        write_persisted(&store, &requests).unwrap();
        assert!(store.text("http_requests").unwrap().contains("Users"));
        let read: HttpRequests = read_persisted(&store).unwrap().unwrap();
        assert_eq!(read, requests);
    }

    #[test]
    fn sending_shows_the_response() {
        let (address, received) = serve(vec![reply(
            "200 OK",
            &["Content-Type: application/json"],
            br#"{"b":1,"a":2}"#,
        )]);
        let mut app = app();
        app.world_mut().resource_mut::<HttpRequests>().0.variables =
            vec![Field::new("base", &address)];
        let id = add(
            &mut app,
            SavedRequest {
                url: "{{base}}/items".into(),
                params: vec![Field::new("q", "a b")],
                ..Default::default()
            },
        );
        sent(&mut app, id);
        settle(&mut app);

        assert_eq!(
            received.recv().unwrap().request_line(),
            "GET /items?q=a%20b HTTP/1.1"
        );
        let exchanges = app.world().resource::<Exchanges>();
        let Some(Exchange::Done(Ok(got))) = exchanges.get(id) else {
            panic!("no response");
        };
        assert_eq!(got.response.status, 200);
        assert_eq!(got.text.as_ref().unwrap().text, r#"{"b":1,"a":2}"#);
        assert_eq!(
            got.pretty.as_ref().unwrap().text,
            "{\n  \"b\": 1,\n  \"a\": 2\n}"
        );
    }

    #[test]
    fn an_unknown_variable_fails_without_sending() {
        let mut app = app();
        let id = add(
            &mut app,
            SavedRequest {
                url: "{{nowhere}}/x".into(),
                ..Default::default()
            },
        );
        sent(&mut app, id);
        settle(&mut app);
        let exchanges = app.world().resource::<Exchanges>();
        assert!(matches!(
            exchanges.get(id),
            Some(Exchange::Done(Err(Error::Invalid(why)))) if why == "Unknown variable {{nowhere}}"
        ));
    }

    #[test]
    fn cancelling_or_closing_the_panel_stops_a_request() {
        let mut app = app();
        let id = add(
            &mut app,
            SavedRequest {
                url: stall(),
                ..Default::default()
            },
        );
        let panel = sent(&mut app, id);
        app.update();
        intend(&mut app, panel, TestIntent::Cancel(id));
        app.update();
        assert!(matches!(
            app.world().resource::<Exchanges>().get(id),
            Some(Exchange::Done(Err(Error::Cancelled)))
        ));

        let id = add(
            &mut app,
            SavedRequest {
                url: stall(),
                ..Default::default()
            },
        );
        let panel = sent(&mut app, id);
        app.update();
        app.world_mut().despawn(panel);
        settle(&mut app);
        assert!(matches!(
            app.world().resource::<Exchanges>().get(id),
            Some(Exchange::Done(Err(Error::Cancelled)))
        ));
    }

    #[test]
    fn new_duplicate_and_delete_keep_the_selection_sensible() {
        let mut app = app();
        let panel = panel(&mut app);
        let selected = |app: &App| app.world().get::<HttpClientPanel>(panel).unwrap().selected;
        intend(&mut app, panel, TestIntent::New);
        app.update();
        let first = selected(&app).unwrap();
        intend(&mut app, panel, TestIntent::Duplicate(first));
        app.update();
        let copy = selected(&app).unwrap();
        assert_ne!(copy, first);
        intend(&mut app, panel, TestIntent::Delete(copy));
        app.update();
        assert_eq!(selected(&app), Some(first));
        assert_eq!(app.world().resource::<HttpRequests>().0.requests.len(), 1);
    }
}

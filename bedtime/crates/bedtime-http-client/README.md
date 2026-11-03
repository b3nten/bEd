# bedtime-http-client

Compose HTTP requests, send them and read the response. For now, the engine;
the panel (saved requests, a composer and the response) comes next.

```rust,ignore
use bedtime_http_client::{Method, Options, Request, send};

let request = Request {
    method: Method::Post,
    url: "localhost:3000/users".into(), // http:// is assumed
    headers: vec![("Content-Type".into(), "application/json".into())],
    body: br#"{"name":"bed"}"#.to_vec(),
};
let response = send(&request, &Options::default(), || cancelled())?;
println!("{} {:?} in {:?}", response.status, response.reason, response.elapsed);
```

## The engine

`send` blocks, but the exchange runs on its own thread: the caller polls
`cancelled` and returns `Error::Cancelled` as soon as it's true, so a stalled
server never holds up a cancel or a quit. Any method may carry a body; an
empty body sends none. Every status is a `Response`; `Error` is for requests
that couldn't be made (`Invalid`), didn't finish in time (`TimedOut`), or
failed on the way (`Failed`).

`Options` sets one timeout for the whole exchange (30 s), a cap on the body
(16 MB; longer bodies are cut short and marked `truncated`), and whether to
follow redirects (up to 10; otherwise the redirect is the response).

TLS is rustls with Mozilla's roots (`webpki-roots`), so certificates trusted
only by the system keychain (a local development CA) aren't trusted yet.

Tests run against a local server; `cargo test -p bedtime-http-client --
--ignored` also checks HTTPS against example.com.

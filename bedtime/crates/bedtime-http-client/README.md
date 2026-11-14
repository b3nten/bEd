# bedtime-http-client

Compose HTTP requests, send them and read the response: the engine, saved
requests, and (feature `workbench`, on by default) the HTTP client panel.

```rust,ignore
app.add_plugins(HttpClientPlugin);
world.spawn((HttpClientPanel::default(), InArea(area)));
```

Or the engine alone:

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

## Saved requests

A `Collection` is a list of `SavedRequest`s (name, method, URL, query
params, headers, body) and the variables they share. Params, headers and
variables are `Field`s: a name and value that can be switched off without
being deleted. `resolve` makes a saved request a `Request`:

- `{{name}}` anywhere (URL, params, headers, body) becomes the value of the
  enabled variable called `name`; an unknown one is an error, not sent as
  written. Values aren't filled in again.
- Enabled, named params are added to the URL's query (before any
  `#fragment`), percent-encoded; the URL may already have a query.
- Enabled, named headers go out in order. The body is sent as written; its
  `Content-Type` is a header like any other.

## Reading a body

`content::decode(content_type, body)` says what a body is and reads it:

- **Text** for `text/*`, JSON, XML, JavaScript, YAML, TOML, forms and the
  like (`+json`, `+xml` and `+yaml` types too), with the language to colour it
  as. **Image** for `image/*`. Other types are **binary**.
- Without a type, or with `application/octet-stream`, the body decides: an
  image's signature, a BOM, NUL bytes (binary) or valid UTF-8 (text, JSON or
  HTML by how it starts).
- Text is decoded from its charset: a BOM, else the header's `charset`, else
  what an HTML `<meta>` or the `<?xml?>` declaration says, else UTF-8
  (`encoding_rs`, so `ISO-8859-1` reads as `windows-1252`, as in browsers).
  Bytes that aren't valid are replaced with `�` rather than calling the body
  binary; the panel notes the charset and any replacing.

`pretty_json` indents a JSON body two spaces a level, re-indenting the text
rather than parsing and printing it, so keys keep their order and numbers
their digits.

## The panel (feature `workbench`)

`HttpClientPanel` edits the workspace's requests (`HttpRequests`, saved in
`WorkbenchStore` as `http_requests.ron`) and sends them.

- **Requests** run down the side: each its method and name (or URL), with a
  spinner while it's on its way. **+** adds one and puts the cursor in its
  URL; right-click to Duplicate or Delete (confirmed). **Variables** at the
  bottom edits the shared variables.
- **The request:** its name, then method, URL and Send (Cancel while it's
  sending). Enter in the URL, or Cmd/Ctrl+Enter anywhere in the panel, sends
  it. Under them, tabs for Params, Headers and Body.
- **The response:** status (colored by class), time, size and the final URL
  after redirects; then Body and Headers. The body is a `bedtime_ui::CodeView`:
  coloured by its language, selectable, with find (Cmd/Ctrl+F), Wrap and
  Copy. A JSON body is indented, with Raw to see it as it came. At most 1 MB
  is shown. An image (any `image/*`, SVG included) is decoded on the request's
  thread and shown in a `bedtime_ui::image::ImageView` (zoom, pan); JSON can
  show as a `bedtime_ui::data::DataTree` (Tree); other bytes show in a
  `bedtime_ui::hex::HexViewer`. A
  request that couldn't be sent says why.

Requests run on `ManagedWork` tasks owned by the panel, so closing it or
quitting cancels them. Responses (`Exchanges`, one per request) are kept in
memory only. Drawing edits a copy of the requests, written back when it
changed; adding, duplicating, deleting, sending and cancelling are intents
applied in `Update` (`HttpClientSystems`).

Tests run against a local server; `cargo test -p bedtime-http-client --
--ignored` also checks HTTPS against example.com. `tests/automation/http_client.py`
drives the panel in bEd.

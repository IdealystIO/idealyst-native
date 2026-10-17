# Net — cross-platform HTTP

`net` is the framework's networking SDK: an HTTP `Client`, a
`WebSocket`, and a receive-only `EventSource` for Server-Sent Events.
HTTP has one API surface and four transports underneath:

| Target | Backend |
|---|---|
| Linux / Windows / terminal | `reqwest` + rustls, on an I/O runtime thread `net` owns |
| Web (wasm32) | `fetch` via web-glue bindings |
| iOS / macOS / tvOS | `NSURLSession` via `objc2` + `block2` |
| Android | `HttpURLConnection` via JNI on a worker thread |

Each backend uses the platform's *native* HTTP stack. That's a
deliberate choice: it means apps inherit App Transport Security on
iOS, system proxy on Android, certificate pinning hooks where the
OS supplies them, and `fetch`-cache semantics on web. No 2MB+
rustls/reqwest cohort gets dragged into a 100KB wasm bundle.

The crate is independently usable — `net` doesn't depend on
`runtime-core` or the reactive system. Its only framework dependency
is `web-glue` (the browser bindings), and only on wasm. You can drop
it into a plain Rust binary.

## The shape

```rust
use net::{Client, Json};

let client = Client::new();

let user: User = client
    .get("https://api.example.com/users/1")
    .header("Authorization", "Bearer xyz")
    .send()
    .await?
    .json()
    .await?;

let created: User = client
    .post("https://api.example.com/users")
    .body(Json(&CreateUser { name: "Alice".into() }))
    .send()
    .await?
    .json()
    .await?;
```

The HTTP types:

- **`Client`** — cheap-to-clone (`Arc` inside), holds a base URL,
  default headers, default timeout, and a per-process connection
  pool.
- **`ClientBuilder`** — `Client::builder().base_url(...).default_header(...).build()`.
- **`RequestBuilder`** — fluent. Headers, query params, body,
  timeout, cancellation. Returned by `client.get(url)` / `post(url)` / etc.
- **`Response`** — the `await`ed result. Status, headers, body
  (buffered into memory).
- **`Method`** — closed enum: `Get`, `Post`, `Put`, `Patch`,
  `Delete`, `Head`, `Options`.
- **`Headers`** — order-preserving case-insensitive multi-map.
  `Set-Cookie` repeats are kept.

## Bodies are pluggable

`Body` semantics go through two traits:

```rust
pub trait IntoBody {
    fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error>;
}

pub trait FromBody: Sized {
    fn from_body(bytes: Vec<u8>, content_type: Option<&str>) -> Result<Self, Error>;
}
```

Built-in impls: `()`, `Vec<u8>`, `String`, `&'static str`, plus the
feature-gated `Json<T>` and `Form<T>` wrappers.

```rust
client.post(url).body(Json(&payload)).send().await?;
client.post(url).body(Form(&form_data)).send().await?;
client.post(url).body(b"raw bytes".to_vec()).send().await?;
client.post(url).body(()).send().await?;   // no body, no content-type
```

`Json<T>` and `Form<T>` set their content-type defaults — overridden
if you already added a `.header("Content-Type", ...)`.

The body builders return the builder, not a `Result`. A body that
fails to encode is held on the request and returned as
`Error::Serialize` from `.send()`, so one `?` after `.await` covers it.

### Downstream wire formats

The traits are the extensibility point. Server functions (the SDK
on top of `net`) send their arguments as JSON with `net::Json`.

If you want msgpack, CBOR, protobuf, postcard, etc., write a small
wrapper `Foo<T>(pub T)` in your own crate and implement both traits.
No changes to `net` required.

## Convenience methods

`Response` exposes the common decode paths without going through
`FromBody`:

```rust
let resp = client.get(url).send().await?;
let body_bytes: Vec<u8> = resp.bytes().await?;
let body_text: String   = resp.text().await?;
let body_json: User     = resp.json().await?;
```

For request bodies, `.json(&value)` / `.form(&value)` are sugar that
takes `&T` so the caller doesn't move the value:

```rust
client.post(url).json(&payload).send().await?;   // = .body(Json(&payload))
```

## Cancellation

`net` ships its own cancellation primitive so it can be used without
pulling in the framework's reactive system.

```rust
let (handle, token) = net::cancel_token();

let request = client.get(url).cancel_on(token).send();

// later, anywhere:
handle.cancel();
```

When `cancel()` fires:

- If the request is already in flight, the underlying transport
  aborts it (reqwest drops the future, the browser calls
  `AbortController::abort`, iOS sends the task `cancel`, Android
  attaches a fresh JNI thread and calls `HttpURLConnection.disconnect`).
- If the request hasn't even started, it short-circuits before
  hitting the network.
- The future returned by `.send()` resolves with `Err(Error::Cancelled)`.

The `(handle, token)` pair is shaped that way so a single handle
can fire many requests — clone the token, attach it to N
in-flight requests, and `handle.cancel()` aborts the whole fan-out.

### Bridging to other cancel systems

`net::CancelToken` doesn't know about `runtime-core::ResourceCancel`
or anything else. To bridge:

```rust
let (handle, token) = net::cancel_token();
resource_cancel.on_cancel(move || handle.cancel());
```

Whenever the higher-level system cancels, the net side fires.
(Server functions wrap this pattern into `server::with_cancel(resource_cancel, future)`
so authors don't write it by hand — see [Server functions](server-functions).)

## Error type

Single enum, transport-agnostic:

```rust
pub enum Error {
    InvalidUrl(String),
    Network(String),
    Timeout,
    Cancelled,
    Status { code: u16, body: Option<String> },
    Serialize(String),
    Deserialize(String),
    Offline,
    Other(String),
}
```

`Status` is only produced when you opt in via
`Response::error_for_status()` — otherwise 4xx and 5xx come back
as normal `Response` values, and your code decides what to do with
the status code.

## Configuration patterns

### A configured client

```rust
let api = Client::builder()
    .base_url("https://api.example.com")
    .default_header("Authorization", &format!("Bearer {}", token))
    .timeout(Duration::from_secs(30))
    .build();

let user: User = api.get("/users/1").send().await?.json().await?;
```

Relative URLs are joined to `base_url`; absolute URLs always win
(useful for the rare cross-origin call).

### Per-request overrides

Anything set on the client can be overridden per-request:

```rust
api.post("/upload")
    .header("Content-Type", "application/octet-stream")   // wins over Json's default
    .timeout(Duration::from_secs(120))                    // wins over client default
    .body(bytes)
    .send().await?;
```

## Per-platform notes

For the most part, you don't have to think about which transport is
running. A few platform-specific notes for the people who care:

- **Web**: the browser's `fetch`, called through web-glue bindings.
  Every request carries an `AbortController`: a cancel, an expired
  `timeout`, or dropping the `send` future aborts the browser request.
  Bodies cross as raw bytes, never decoded as text.
- **iOS**: Each request gets a `NSURLSessionDataTask` against
  `NSURLSession.sharedSession`. The completion block bridges back
  to Rust via a `futures-channel` oneshot. Cancellation sends
  `[task cancel]`; the resulting `NSURLErrorCancelled` (-999) maps
  to `Error::Cancelled`.
- **Android**: Each request spawns a worker thread that attaches
  to the JavaVM via `ndk_context`. The connection is promoted to
  a JNI `GlobalRef` so a cancel watcher on a separate thread can
  call `disconnect()` mid-flight.
- **macOS / tvOS**: the same `NSURLSession` transport as iOS.
- **Linux / Windows / terminal**: reqwest with rustls, run on one
  process-wide runtime thread `net` owns, so the host doesn't need
  to provide a Tokio runtime. Dropping the future aborts the
  exchange; a `poll_fn` race keeps the cancel-token contract uniform
  across all transports.

## WebSocket and Server-Sent Events

`net::WebSocket` is the bidirectional sibling of `Client`:
`connect(url)`, `send`, `recv`, `close`, and it closes on drop.
Messages are `WsMessage::Text` or `WsMessage::Binary`;
`close_status()` reports the close code and reason the same way on
every target.

```rust
use net::{WebSocket, WsMessage};

let mut ws = WebSocket::connect("wss://echo.example.com").await?;
ws.send(WsMessage::Text("hello".into()))?;
while let Some(msg) = ws.recv().await {
    match msg? {
        WsMessage::Text(t) => println!("{t}"),
        WsMessage::Binary(b) => println!("{} bytes", b.len()),
    }
}
```

`net::EventSource` reads a `text/event-stream` (what a `#[sse]`
server endpoint serves): `connect(url)`, then `recv()` yields each
event's `data:` payload.

The web arm uses the browser's `WebSocket` and `EventSource`. Native
targets run `tungstenite` on an I/O thread for WebSocket; Android
supports `ws://` only (no bundled TLS).

## What's not included

- **Streaming HTTP response bodies.** `Response` bodies are buffered
  in memory; use `EventSource` or `WebSocket` for data that arrives
  over time.
- **Multipart uploads.**
- **Cookies.** Each platform's native HTTP stack manages its own
  cookie jar; there is no cross-platform cookie API.

## Where to read more

- [Server functions](server-functions) — the canonical
  consumer of `net`. Every `#[server]` call site routes through
  this crate on the client side.
- [Async reactivity](async-reactivity) — the higher-level
  reactive primitives that consume `net`'s futures.

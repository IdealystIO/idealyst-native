# Net — cross-platform HTTP

`net` is the framework's HTTP client SDK. One API surface, four
transports underneath:

| Target | Backend |
|---|---|
| Native (macOS / Linux / Windows / wgpu / terminal) | `reqwest` + rustls |
| Web (wasm32) | `fetch` via `gloo-net` |
| iOS / macOS / tvOS | `NSURLSession` via `objc2` + `block2` |
| Android | `HttpURLConnection` via JNI on a worker thread |

Each backend uses the platform's *native* HTTP stack. That's a
deliberate choice: it means apps inherit App Transport Security on
iOS, system proxy on Android, certificate pinning hooks where the
OS supplies them, and `fetch`-cache semantics on web. No 2MB+
rustls/reqwest cohort gets dragged into a 100KB wasm bundle.

The crate is independently usable — `net` doesn't depend on
`runtime-core` or any other framework crate. You can drop it into a
plain Rust binary.

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
    .body(Json(&CreateUser { name: "Alice".into() }))?
    .send()
    .await?
    .json()
    .await?;
```

Six public types:

- **`Client`** — cheap-to-clone (`Arc` inside), holds a base URL,
  default headers, default timeout, and a per-process connection
  pool.
- **`ClientBuilder`** — `Client::builder().base_url(...).default_header(...).build()`.
- **`RequestBuilder`** — fluent. Headers, query params, body,
  timeout, cancellation. Returned by `client.get(url)` / `post(url)` / etc.
- **`Response`** — the `await`ed result. Status, headers, body
  (already buffered into memory in v0).
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
client.post(url).body(Json(&payload))?.send().await?;
client.post(url).body(Form(&form_data))?.send().await?;
client.post(url).body(b"raw bytes".to_vec()).send().await?;
client.post(url).body(()).send().await?;   // no body, no content-type
```

`Json<T>` and `Form<T>` set their content-type defaults — overridden
if you already added a `.header("Content-Type", ...)`.

### Downstream wire formats

The trait is the extensibility point. Server functions (the SDK on
top of `net`) ship a postcard-shaped wrapper for binary RPC by
implementing `IntoBody`/`FromBody` for its own format type. No
changes to `net` required.

If you want msgpack, CBOR, protobuf, etc., the same shape applies:
write a small wrapper `Foo<T>(pub T)`, implement both traits,
done.

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
client.post(url).json(&payload)?.send().await?;   // = .body(Json(&payload))?
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
so authors don't write it by hand — see [Server functions](./16-server-functions.md).)

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

- **Web**: `gloo-net::Request` underneath. The cancel mechanism is
  `web_sys::AbortController`. Binary bodies go through
  `Uint8Array` to avoid UTF-8 lossy decoding.
- **iOS**: Each request gets a `NSURLSessionDataTask` against
  `NSURLSession.sharedSession`. The completion block bridges back
  to Rust via a `futures-channel` oneshot. Cancellation sends
  `[task cancel]`; the resulting `NSURLErrorCancelled` (-999) maps
  to `Error::Cancelled`.
- **Android**: Each request spawns a worker thread that attaches
  to the JavaVM via `ndk_context`. The connection is promoted to
  a JNI `GlobalRef` so a cancel watcher on a separate thread can
  call `disconnect()` mid-flight.
- **Native**: reqwest with rustls. Drop-on-future-drop handles
  cancellation; we wrap that in a `poll_fn` race so the
  cancel-token contract is uniform across all transports.

## What's not in v0

To save people grep-time:

- **Streaming responses.** Bodies are buffered in memory. Streaming
  is on the roadmap; the public API will keep `Response::bytes()` /
  `.text()` / `.json()` and add a `.stream()`.
- **Multipart uploads.** Same — needs a streaming body story first.
- **WebSocket.** Not in `net`. The next SDK in line is a separate
  `ws` crate; it'll share `net`'s cancel and address types.
- **Cookies.** Each platform's native HTTP stack manages its own
  cookie jar; we don't impose a cross-platform abstraction yet.

## Where to read more

- [Server functions](./16-server-functions.md) — the canonical
  consumer of `net`. Every `#[server]` call site routes through
  this crate on the client side.
- [Async reactivity](./14-async-reactivity.md) — the higher-level
  reactive primitives that consume `net`'s futures.

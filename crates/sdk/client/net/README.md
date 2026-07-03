# `net`

Cross-platform **async networking** — HTTP, WebSocket, and Server-Sent
Events over each platform's native stack. One async API
(`Client::get(url).send().await?.json()`) that compiles to `fetch` on web,
`NSURLSession` on iOS/macOS, `HttpURLConnection` on Android, and `reqwest`
on desktop. The platform-agnostic shell — builders, body codecs, header
map, error type, cancellation — lives in the crate; one cfg-gated transport
supplies the substrate per target, so consumer code is identical
everywhere.

This is the foundation the `server` SDK (server functions) is built on, but
it's independently useful for any author hitting an external API.

```rust
use net::{Client, Error};
# #[derive(serde::Deserialize)] struct User { id: u64, name: String }

# async fn demo() -> Result<(), Error> {
let client = Client::new();

let user: User = client
    .get("https://api.example.com/users/1")
    .header("Authorization", "Bearer xyz")
    .send()
    .await?
    .json()
    .await?;
# let _ = user;
# Ok(())
# }
```

## What you get

- **HTTP** — `Client` + a fluent `RequestBuilder` (`get`/`post`/`put`/
  `patch`/`delete`/`head`/`options`, `header`/`query`/`body`/`json`/`form`/
  `timeout`/`cancel_on`), and a buffered `Response` (`status`,
  `error_for_status`, `json`/`text`/`bytes`/`body`). A `ClientBuilder` sets
  a base URL, default headers, and a default timeout.
- **WebSocket** — `WebSocket::connect` with `send`/`recv`/`close`,
  close-on-drop, and a cloneable `WsSender` split so one task can `recv`
  while others `send`.
- **Server-Sent Events** — `EventSource::connect` + `recv` for consuming a
  `text/event-stream` (what a `#[sse]` endpoint serves), with a cloneable
  `EventSourceCloser`.
- **Pluggable bodies** — request/response codecs via the symmetric
  `IntoBody` / `FromBody` traits. Built-ins cover `Vec<u8>`, `String`,
  `&'static str`, `()`, and (default features) the `Json` and `Form`
  wrappers; downstream crates add their own (postcard / protobuf / …)
  without touching this crate.
- **Cancellation** — `cancel_token()` → `(CancelHandle, CancelToken)`;
  attach the token with `RequestBuilder::cancel_on` and fire the handle to
  abort every in-flight request sharing it (`Error::Cancelled`).
  Self-contained — no `tokio` / `runtime-core` dependency.

Every backend delivers the **same shape**: the same `Response`, the same
closed `Error` enum (`InvalidUrl`, `Network`, `Timeout`, `Status`,
`Serialize`, `Deserialize`, `Offline`, `Cancelled`, `Other`). The platforms
diverge in mechanism, not in what you observe.

## Per-platform mechanism

| Target | HTTP | WebSocket | SSE |
| --- | --- | --- | --- |
| macOS / Windows / Linux / terminal | `reqwest` (rustls) | `tungstenite` on an I/O thread (`ws://` + `wss://`) | `reqwest::blocking` on an I/O thread |
| iOS / macOS / tvOS | `NSURLSession` (objc2) | `tungstenite` (shared native arm) | `NSURLSession` + `NSURLSessionDataDelegate` |
| Android | `HttpURLConnection` (JNI) | `tungstenite`, `ws://` only | `HttpURLConnection.getInputStream()` (JNI) |
| Web (wasm32) | `fetch` (gloo-net) | `web_sys::WebSocket` | the browser's `EventSource` |

No async runtime is introduced anywhere (the framework's execution-model
invariant): native arms drive a blocking I/O worker thread and bridge to
`.await` through `futures-channel`; web/Apple/Android use the OS event loop.
TLS on native uses **rustls** (no native-tls, no OpenSSL); Android's HTTP/WS
stay JNI/`tungstenite` to avoid bundling a second TLS stack — so WebSocket
on Android is `ws://`-only, with platform-native (OkHttp /
`URLSessionWebSocketTask`) as the documented future path for `wss://` and OS
proxy/background integration.

## Features

| Feature | Default | Adds |
| --- | --- | --- |
| `json` | yes | `Json<T>` wrapper + `RequestBuilder::json` / `Response::json` (`serde_json`) |
| `form` | yes | `Form<T>` wrapper + `RequestBuilder::form` / `query` (`serde_urlencoded`) |

## Permissions

This SDK declares the capability it needs in its own `Cargo.toml`:

```toml
[package.metadata.idealyst]
capabilities = ["internet"]
```

The CLI walks your app's dependency graph at build time, finds the
declaration, and injects the platform artifact automatically — for `net`
that's Android's

```xml
<uses-permission android:name="android.permission.INTERNET"/>
```

so a local Android build can reach the network without you hand-editing
`AndroidManifest.xml`. No other platform requires a declared permission for
outbound networking, and `internet` shows no user-facing OS prompt, so there
is nothing to add under `[package.metadata.idealyst.app.permissions]`.

## Scope

Responses are **buffered** in memory (streaming bodies are a follow-up
behind the same `Response`). The body-codec traits are deliberately minimal
and unopinionated: the crate establishes the raw request/response capability
and the pluggable codec seam; higher-level SDKs (server functions, resource
hooks) layer the state-binding and typed-RPC opinions on top.

[`reqwest`]: https://crates.io/crates/reqwest

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p net` — body codecs, header map, builder, error mapping
- [ ] `cargo test -p net --test native_transport` — live HTTP / WebSocket / SSE / cancellation integration suite (reqwest + tungstenite arms)
- [ ] `cargo build -p net --target wasm32-unknown-unknown` — web (fetch / `web_sys::WebSocket` / browser `EventSource`)

**Behavior**
- [ ] **Web** — GET/POST to a live endpoint over `fetch`; WebSocket echo over `web_sys::WebSocket`; SSE stream over the browser's `EventSource`; cancel mid-flight aborts (`Error::Cancelled`)
- [ ] **iOS** — same over `NSURLSession` (HTTP + SSE) and the shared `tungstenite` WebSocket arm
- [ ] **Android** — GET/POST over `HttpURLConnection`; SSE over `getInputStream()`; WebSocket echo (`ws://` only — `wss://` is the documented future path); cancel mid-flight aborts
- [ ] **macOS** — HTTP/SSE over `NSURLSession` or `reqwest`; WebSocket echo (`ws://` + `wss://`); cancel mid-flight aborts
- [ ] **Windows / Linux** — HTTP via `reqwest` (rustls TLS); WebSocket echo (`ws://` + `wss://`); SSE via `reqwest::blocking` worker; cancel mid-flight aborts

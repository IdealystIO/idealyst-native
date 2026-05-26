# Server functions

A server function is a Rust async fn whose body runs on a backend
process and whose call sites compile, on the client, into typed
HTTP stubs. You write one function. The framework produces two
artifacts that talk to each other over the wire — no schema file,
no protobuf, no codegen step you run by hand. The types in the
signature are the contract.

```rust
use server::{server, ServerError};

#[server]
async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
    Ok(a + b)
}

// Same call site on both sides:
let sum = add(2, 3).await?;          // == 5
```

On the server, the body runs. On the client, the body is replaced
with a `POST /_srv/add` that ships `[2, 3]` as JSON, awaits the
response, and decodes it back into `Result<i32, ServerError>`.

## The pitch

A normal full-stack workflow looks like:

1. Author API endpoint on the server.
2. Author DTO types in a shared library.
3. Author client wrapper that serialises the request and parses the
   response.
4. Keep three things in sync forever.

Server functions collapse steps 1, 3, and 4 into one declaration.
DTO types (step 2) still exist — they're the args + return — but
nothing else gets to drift.

## The macro split

`#[server]` is an attribute macro. It expands an async fn into two
cfg-gated halves:

**Server build** (`--features server`):

```rust
async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
    Ok(a + b)                                        // original body
}

mod __server_fn_add {
    inventory::submit! {
        server::__private::ServerFnEntry {
            path: "add",
            handler: |body_bytes| Box::pin(async move {
                let (a, b): (i32, i32) =
                    server::__private::decode_args(&body_bytes)?;
                let result: Result<i32, ServerError> = super::add(a, b).await;
                server::__private::encode_result(&result)
            }),
        }
    }
}
```

**Client build** (default):

```rust
async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
    let args = (a, b);
    server::__private::call::<(i32, i32), Result<i32, ServerError>>(
        "add",
        &args,
    ).await
}
```

The `server` cargo feature is the switch. Set it on the server's
binary build; leave it off on the client's build. Both halves see
the same source file; only one half of the expansion ends up in
each compiled artifact.

### Attribute arguments

```rust
#[server]                              // path = function name ("add")
#[server(path = "v1/users/list")]      // explicit path
```

The path appears after `/_srv/` in the URL. Use it to version
endpoints or to scope groups of functions.

### Constraints

- Must be `async`.
- Must declare a return type (no implicit `()`).
- No `self` receivers (server fns are free functions).
- Args must be `Serialize + DeserializeOwned`.
- Return must be `Result<T, E>` where `T: Serialize + DeserializeOwned`
  and `E` implements [`ServerFnReturn`](#errors-and-serverfnreturn).
  The built-in error type `ServerError` already implements it.

## The wire protocol

Deliberately small for v0:

- **Single call:** `POST /_srv/<path>` with JSON body
  `[arg0, arg1, ...]`. Response is JSON `{"Ok": T}` or `{"Err": E}` —
  the standard serde `Result` encoding. Status is always `200` for
  a function that ran (success or domain error); only the
  dispatcher itself returns 4xx/5xx (404 for unknown path, 400 for
  malformed args).

- **Batched calls:** `POST /_srv/_batch` with JSON
  `[{"path": "...", "args": [...]}, ...]`. Response is a same-length
  array of `Result` slots.

The batch route is the same protocol applied N times. The framework
picks single vs batch automatically (see [Batching](#batching)
below).

> **Why JSON?** Easy to debug, universally supported by every
> client transport without extra deps. The `IntoBody` /
> `FromBody` traits in [`net`](./15-net.md) make swapping to
> postcard / msgpack later a downstream wrapper — no changes to
> the macro.

## The build setup

The recommended layout for a real app is three crates:

```
my-app/
├── shared/   # types + #[server] fns + cfg-gated server state
├── server/   # bin, depends on shared with features=["server"]
└── client/   # one or more clients (web wasm, native, mobile);
              # depend on shared with default features
```

`shared/`'s `Cargo.toml` looks like:

```toml
[features]
default = []
server = ["server/server", "dep:diesel", "dep:tokio"]

[dependencies]
server = { workspace = true }
serde = { version = "1", features = ["derive"] }
diesel = { version = "2", features = ["postgres"], optional = true }
tokio = { version = "1", features = ["macros", "rt-multi-thread"], optional = true }
```

The key things:

1. **Server-only deps** (Diesel, Redis, tokio, anything that has no
   business in a wasm bundle) are declared `optional = true` and
   activated only by the `server` feature.
2. The `server` feature forwards to `server/server` so the macro
   expands its server half.
3. **Build commands must be separate per binary** so cargo doesn't
   unify features across them:

   ```sh
   # ✓ correct
   cargo build -p server --features server
   idealyst build --web client

   # ✗ leaks server-only code into the client
   cargo build --all-bins --features server
   ```

If you do that, Diesel never gets compiled into the wasm bundle.
The macro discards server function bodies on the client side
entirely, so the references to Diesel in those bodies never reach
the client compilation. The shape of the imports matters:

```rust
// shared/src/server_fns.rs

// ❌ leaks: `use diesel::*` at module scope is compiled in both
// modes. If diesel isn't in the client's dep graph, this errors.
use diesel::prelude::*;

#[server]
async fn list_todos() -> Result<Vec<Todo>, ServerError> { ... }
```

```rust
// ✅ clean: cfg-gated import, only compiled with the server half
#[cfg(feature = "server")]
use diesel::prelude::*;

#[server]
async fn list_todos() -> Result<Vec<Todo>, ServerError> { ... }
```

## Wiring up the server

The server binary builds a router from the inventory of registered
functions:

```rust
use server_fn_demo::state::AppState;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // App-level state — see Extractors below.
    server::install_state(Arc::new(AppState::new()));

    // server::router() returns an axum::Router with /_srv/* routes.
    // Compose with whatever else your server needs (static files,
    // health checks, custom middleware) before serving.
    let app = server::router()
        .nest_service("/pkg", ServeDir::new("./pkg"))
        .fallback_service(ServeDir::new("."));

    let addr: SocketAddr = "0.0.0.0:3000".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

`server::router()` is just axum. It walks the inventory once and
registers `/_srv/_batch` + `/_srv/*path`. Compose with
`.nest_service`, `.layer`, `.merge` — whatever axum supports.

`server::serve(addr)` is a one-liner shortcut if you don't need
custom composition:

```rust
server::serve("0.0.0.0:3000".parse().unwrap()).await?;
```

## Wiring up the client

```rust
use server::ClientConfig;

server::configure(ClientConfig {
    base_url: "https://api.example.com".into(),
});
```

`configure` is idempotent — call it once at app start, or call it
again to retarget (useful for tests against an ephemeral server).

For web apps, the natural base URL is the page's own origin:

```rust
#[cfg(target_arch = "wasm32")]
{
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_else(|| "http://localhost:3000".to_string());
    server::configure(ClientConfig { base_url: origin });
}
```

Server-fn calls happen against `<base_url>/_srv/...`. The transport
underneath is the [`net`](./15-net.md) SDK — same per-platform HTTP
stack, no separate code path.

## Batching

Three calls fired in the same tick coalesce into one HTTP request:

```rust
let (user, todos, projects) = tokio::join!(
    get_user(uid),
    list_todos(uid),
    list_projects(),
);
```

becomes one `POST /_srv/_batch` with three entries, not three
separate posts. The server unpacks the array, dispatches each
entry against its handler, and returns the results in the same
order.

The mechanism is **inline microtask coalescing**:

1. Each call enqueues into a process-global pending queue.
2. The first caller to observe an empty queue becomes the *flusher*
   for that batch — it yields once (giving sibling calls a chance
   to enqueue on the same tick), then drains the queue and
   dispatches.
3. Solo flushes (only one call in the queue at flush time) keep the
   single-call wire (`POST /_srv/<path>`). Multi-entry flushes go
   through `_batch`.

Authors don't opt in — every server-fn call funnels through this
pipeline. A solo call pays one task-yield of latency, which is one
frame at most.

> **The on-mount fan-out.** This is where batching pays for
> itself. A typical app loads several resources when a page
> mounts (`use_query(get_user)`, `use_query(list_todos)`,
> `use_query(list_projects)`). With batching, that fan-out is one
> request on the wire, not three. Open the network tab in any of
> the example apps to see the `_srv/_batch` line.

## Cancellation

Aborting a server-fn call has two flavours:

### Cancelling a `resource` fetch

When a `resource()`'s deps change, the in-flight fetch should
abort. Wrap the fetcher's future in `server::with_cancel`:

```rust
use runtime_core::resource;
use server::with_cancel;

let user_id = signal(1u64);

let user = resource(user_id, |id, resource_cancel| async move {
    with_cancel(resource_cancel, get_user(id)).await
});
```

`with_cancel` does three things:

1. Creates a fresh `(net::CancelHandle, net::CancelToken)` pair.
2. Registers `resource_cancel.on_cancel(|| handle.cancel())` so a
   resource cancel flows downstream to the HTTP layer.
3. Wraps the inner future in a scope that puts the token into a
   thread-local. The macro's client stub reads it and threads it
   into the underlying `net::RequestBuilder::cancel_on(token)`.

So a single `dep.set(new_value)` cancels:

- The resource's prior fetch (via `resource_cancel`).
- The in-flight HTTP request (via `net::CancelToken`).
- The actual network read on whichever platform (reqwest drops
  the future, browser fetches abort, iOS `NSURLSessionTask::cancel`,
  Android `HttpURLConnection::disconnect`).

### Cancelling explicitly

For UIs that own the cancel decision (a button, a user gesture),
use `server::with_cancel_token` with a token you constructed:

```rust
let (handle, token) = net::cancel_token();

let task = tokio::spawn(server::with_cancel_token(token, do_long_op()));

// later:
handle.cancel();
let result = task.await?;  // → Err(ServerError::Cancelled)
```

### Cancellation + batching interop

If a cancellable call is still queued (not yet flushed), the
flusher *removes it from the batch* before sending. The cancelled
call's awaiter returns `Cancelled` immediately; the rest of the
batch is unaffected.

If the cancel fires after the batch has gone over the wire, the
HTTP request runs to completion (other calls in the batch deserve
their results), but the cancelled call's awaiter still returns
`Cancelled` — its slot in the response is discarded.

For solo calls (queue size 1 at flush time), cancel aborts the
underlying HTTP via `net::RequestBuilder::cancel_on`. The future
returns `Cancelled` and the transport tears the connection down.

## Errors and `ServerFnReturn`

Server fns return `Result<T, E>` where `E` implements
[`ServerFnReturn`](https://docs.rs/server/latest/server/trait.ServerFnReturn.html):

```rust
pub trait ServerFnReturn: Sized {
    /// Construct a Self representing a non-domain failure
    /// (transport, codec, dispatcher-level error).
    fn from_server_error(error: ServerError) -> Self;
}
```

The built-in `ServerError` already implements it. Most apps just
return `Result<T, ServerError>`.

The error type has four variants:

```rust
pub enum ServerError {
    Failed(String),                          // server fn returned Err
    Network(String),                         // transport failure (client only)
    Codec(String),                           // JSON encode/decode failed
    Server { status: u16, message: String }, // dispatcher rejected
    Cancelled,                               // cancel token fired
}
```

The split matters:

- `Failed` is what your code returns when business logic fails
  (`return Err(ServerError::failed("not found"))`). It's encoded
  into a `200` body as `{"Err": {"Failed": "not found"}}`. The
  client sees `Err(ServerError::Failed("not found"))`.
- `Network` is only ever observed on the client; it means the
  request never reached the function.
- `Codec` means the wire bytes didn't decode. Usually a sign of
  schema drift between client and server builds.
- `Server` is the dispatcher's response — `404` (unknown path),
  `400` (malformed args), `500` (handler panic). Distinct from
  `Failed`.
- `Cancelled` is a cancel-token cancellation.

### Custom error types

You can use your own error type. Implement `ServerFnReturn` for
`Result<T, MyError>` if you want it to receive transport failures
in `MyError`'s own error variant. v0 ships only the built-in
`Result<T, ServerError>` impl; the trait is the upgrade path.

## Extractors

On the server side, a handler can read request-scoped context.

### App-level state

`install_state(value)` registers something globally. `use_state::<T>()`
inside a server fn reads it back:

```rust
// In your server bin's main:
server::install_state(Arc::new(Db::connect().await));

// In a server fn:
#[server]
async fn list_todos(user_id: u64) -> Result<Vec<Todo>, ServerError> {
    let db = server::use_state::<Arc<Db>>()
        .ok_or_else(|| ServerError::failed("Db not installed"))?;
    db.query("SELECT * FROM todos WHERE user_id = ?", &[&user_id]).await
}
```

The registry is `TypeId`-keyed, so install one of each type you
want to expose. Wrap heavy state in `Arc` so retrieval is cheap.

Available only when `feature = "server"` is on — the client build
doesn't see any of this.

### Per-request data

Request headers:

```rust
#[server]
async fn whoami() -> Result<String, ServerError> {
    let auth = server::use_request_header("authorization")
        .ok_or_else(|| ServerError::failed("missing Authorization"))?;
    Ok(format!("authenticated as: {auth}"))
}
```

Mechanically: the dispatcher wraps the handler's future in a
`tokio::task_local!` scope carrying the request's `HeaderMap`.
`use_request_header(name)` reads from it. Outside a handler (utility
code, background tasks) the accessor returns `None` rather than
panicking.

`use_request_headers()` returns the full `Arc<HeaderMap>` if you
need to iterate.

## How the pieces connect to async reactivity

Server functions are async fns. They compose with every primitive
on the [Async reactivity](./14-async-reactivity.md) page:

```rust
// load on mount, refresh on dep change
let user = resource(user_id, |id, cancel| async move {
    with_cancel(cancel, get_user(id)).await
});

// fire-and-forget mutation (analytics, etc.)
let log = mutation(|name: String| async move { log_event(name).await });

// the workhorse: mutation that folds response into local state
let create = async_reducer(
    todos,
    |input| async { create_todo(input).await },
    |list, new_todo| list.push(new_todo),
);
```

The shape is always: `#[server]` fn → wrap in the primitive that
fits how the UI uses it.

## Limits / what's not in v0

For grep:

- **Streaming.** No `#[server_stream]` yet. Server-pushed updates
  (subscriptions, SSE, WebSockets) are roadmap.
- **Custom error types in `Result<T, E>`.** The trait is in place
  but only `Result<T, ServerError>` ships with an impl. Custom
  error types need `impl ServerFnReturn for Result<T, MyError>`.
- **Binary wire format.** JSON only in v0. Postcard / msgpack is a
  downstream wrapper using `IntoBody`/`FromBody` on `net`.
- **Schema-drift detection.** Mismatched client/server function
  signatures surface as `Codec` errors at runtime. A compile-time
  hash check is roadmap.
- **CLI integration.** `idealyst dev` doesn't yet orchestrate a
  server-bin alongside the wasm build. You run them separately for
  now (`cargo run --bin server --features server` + `idealyst build --web`).
- **`idealyst new` scaffold.** No template for the three-crate
  layout yet. Today you set it up by hand.
- **Per-call headers / auth.** The client uses one shared
  `net::Client` configured at `server::configure(...)`. Adding a
  per-call `.with_header(...)` shape is a planned ergonomic.

## Where to read more

- [Async reactivity](./14-async-reactivity.md) — `resource`,
  `mutation`, `async_reducer`, and how they consume server-fn
  futures.
- [Net](./15-net.md) — the HTTP client SDK underneath. Same
  cancel-token primitive, same `IntoBody`/`FromBody` traits.
- `examples/server-fn-demo` — runnable full-stack todo app
  exercising every concept on this page.

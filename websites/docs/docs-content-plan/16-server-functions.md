# Server functions

Server functions exist to collapse a particular kind of busywork:
maintaining a client API, a server API, and a DTO crate in lockstep
forever. The point isn't that the framework hands you a slightly
nicer HTTP client — it's that **server logic, including database
queries, can be defined directly inside your app**, and the compiler
figures out where each piece runs.

```rust
use server::{server, ServerError};

#[server]
async fn list_todos(user_id: u64) -> Result<Vec<Todo>, ServerError> {
    let db = server::use_state::<Arc<Db>>()
        .ok_or_else(|| ServerError::failed("Db not installed"))?;
    db.query("SELECT * FROM todos WHERE user_id = $1", &[&user_id]).await
}

// In a UI component in the SAME crate:
let todos = list_todos(current_user.id).await?;
```

The call site reads as if the client were running that body
directly. Under the hood, the `#[server]` macro splits the function
based on the build target:

- **Server build** (`--features server`): the body compiles
  verbatim and a handler gets auto-registered at
  `/_srv/list_todos`. Imports like `diesel::prelude::*` (cfg-gated
  to the server build) are visible to that body.
- **Client build** (default): the body is **discarded**. The call
  site compiles into a typed RPC stub — `list_todos(uid)` becomes
  a `POST /_srv/list_todos` that ships `[uid]` as JSON, awaits the
  response, and decodes it back into `Result<Vec<Todo>, ServerError>`.

The Rust types you wrote in the signature **are** the wire contract.
No IDL, no protobuf, no `cargo generate-types` step you have to
remember. The compiler verifies the contract holds on both sides
of the network boundary because the same source file is the
authority for both.

That last point is the part that compounds. The cost the framework
absorbs isn't "one fewer codegen step" — it's the entire
client-API + server-API + types-in-the-middle ceremony that most
full-stack codebases pay forever. Here the boundary is a
compile-time decision, not a code-organization tax.

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
#[server(strict_version)]              // reject schema-mismatched clients
#[server(tags(admin, limit = "30/min"))] // metadata a dispatch hook reads
```

The path appears after `/_srv/` in the URL. Use it to version
endpoints or to scope groups of functions. `strict_version` makes the
server reject a client whose wire schema hash differs, with
`ServerError::IncompatibleVersion`. `tags(...)` attaches static
metadata a dispatch hook reads with `ctx.has_tag` / `ctx.tag`.

### Constraints

- Must be `async`.
- Must declare a return type (no implicit `()`).
- No `self` receivers (server fns are free functions).
- Args must be `Serialize + DeserializeOwned`.
- Return must be `Result<T, E>` where `T: Serialize + DeserializeOwned`
  and `E` implements [`ServerFnReturn`](#errors-and-serverfnreturn).
  `Result<T, ServerError<E>>` already implements it.

## The wire protocol

Deliberately small:

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
> `FromBody` traits in [`net`](net) are where another body format
> would plug in.

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

server::configure(ClientConfig::new("https://api.example.com"));
```

`configure` is idempotent — call it once at app start, or call it
again to retarget (useful for tests against an ephemeral server).

For web apps, the natural base URL is the page's own origin:

```rust
#[cfg(target_arch = "wasm32")]
{
    let origin = deep_link::origin()
        .unwrap_or_else(|| "http://localhost:3000".to_string());
    server::configure(ClientConfig::new(origin));
}
```

Native apps run by `idealyst dev` get the dev server's address from
`server::dev_base_url()` (it reads the `IDEALYST_SERVER_URL` the CLI
sets); it returns `None` in production and on web.

Server-fn calls happen against `<base_url>/_srv/...`. The transport
underneath is the [`net`](net) SDK — same per-platform HTTP
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
> mounts (three `resource(...)` fetchers calling `get_user`,
> `list_todos` and `list_projects`). With batching, that fan-out is one
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
    /// The domain error `E` of this return type's `ServerError<E>`.
    type Error;

    /// Construct a Self representing a non-domain failure
    /// (transport, codec, version, dispatcher-level error).
    fn from_server_error(error: ServerError<Self::Error>) -> Self;
}
```

`Result<T, ServerError<E>>` already implements it, for any
serializable domain error `E`. `E` defaults to `String`, so most apps
just return `Result<T, ServerError>`.

The error type has six variants:

```rust
pub enum ServerError<E = String> {
    Failed(E),                               // server fn returned Err
    Network(String),                         // transport failure (client only)
    Codec(String),                           // JSON encode/decode failed
    Server { status: u16, message: String }, // dispatcher rejected
    Cancelled,                               // cancel token fired
    IncompatibleVersion {                    // client/server schema drift
        path: String,
        client_schema: u64,
        server_schema: u64,
    },
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
- `IncompatibleVersion` means the client and server disagree on this
  function's wire schema. The macro hashes the arg and return types
  on both sides; when a payload fails to decode and the hashes
  differ, you get this instead of `Codec`. A `strict_version`
  function rejects a hash mismatch up front (HTTP 426).

### Custom error types

Spell the return type `Result<T, ServerError<MyError>>`. `MyError`
rides in `Failed` and is serialized across the wire; transport
failures fold into the same `ServerError<MyError>`. No extra impl is
needed.

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
on the [Async reactivity](async-reactivity) page:

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

## Streaming

Three sibling attributes stream instead of returning once. They share
the cfg split, extractors, middleware and errors with `#[server]`;
only the transport and return shape differ.

- **`#[subscription]`** — server → client over WebSocket. The fn
  returns `impl Stream<Item = T>`; the client stub returns a
  scope-bound `UseSocket<T, ()>` that connects on mount and closes
  on unmount.
- **`#[channel]`** — full duplex over WebSocket. The first param is a
  `Socket<In, Out>`; the client gets the mirrored `UseSocket<Out, In>`
  with `send(msg)` and `incoming()`.
- **`#[sse]`** — server → client over plain HTTP
  (`text/event-stream`). The client stub returns the URL; consume it
  with `server::use_sse::<T>(url)`. Prefer it for one-way streams.

## CLI integration

Declare the server bin in the app metadata and `idealyst dev` runs the
whole stack:

```toml
[package.metadata.idealyst.app]
server_bin = "server"    # → cargo run --bin server --features server
```

On web, the dev loop runs your server bin instead of the static dev
server, so `/_srv/*` works and the bundle and API are served
same-origin. A server in another package is named with
`server_manifest`. Native apps get the address through
`server::dev_base_url()`.

## Per-call credentials

Attach a `CredentialProvider` to the client config and its headers go
on every server-fn request. It is called per request, so a refreshed
token is picked up without reconfiguring:

```rust
server::configure(
    ClientConfig::new(base)
        .with_credentials(server::bearer(|| read_token())),
);
```

`server::credentials_from_fn(|| vec![...])` covers any other header
scheme. Cookie auth needs no provider.

## Limits

- **Wire format is JSON.** There is no binary format option.
- **`idealyst new` scaffold.** No template for the three-crate
  full-stack layout; you set it up by hand.

## Where to read more

- [Async reactivity](async-reactivity) — `resource`,
  `mutation`, `async_reducer`, and how they consume server-fn
  futures.
- [Net](net) — the HTTP client SDK underneath. Same
  cancel-token primitive, same `IntoBody`/`FromBody` traits.
- `crates/api/server/examples/server-fn-demo` — runnable full-stack todo app
  exercising every concept on this page.

+++
title = "Server functions"
order = 80
tags = ["server", "rpc", "fullstack", "net", "async_reducer"]
+++

# Server functions

The `server` crate (`crates/api/server`) is the framework's isomorphic RPC
layer. You write an `async fn` once, annotate it `#[server]`, and call it from
client code like a normal function — the macro generates a typed client stub that
serializes the args, POSTs them to the server, and deserializes the typed result.
The server build runs the real body; the client build ships only the stub.

Add the dep ([[sdks|SDKs guide]]):

```toml
[dependencies]
server = { workspace = true } # or git/rev, matching your runtime-core line
```

## Defining a server function

```rust
use server::ServerError;

#[server]
pub async fn create_todo(input: CreateTodo) -> Result<Todo, ServerError> {
    // This body compiles only into the SERVER binary. On the client
    // build the macro replaces it with a stub that calls over the wire.
    let state = server::use_state::<Arc<AppState>>()
        .ok_or_else(|| ServerError::failed("AppState not installed"))?;
    let todo = Todo { id: next_id(), title: input.title, done: false };
    state.todos.lock().unwrap().push(todo.clone());
    Ok(todo)
}
```

Rules the macro enforces (the provable client/server boundary):

- **Free functions only** — no `self` receiver; a free fn captures no client
  state.
- **Args + return must be `Serialize + DeserializeOwned`** — a `Signal<T>`,
  `Element`, or backend handle cannot cross the wire (compile error, not a lint).
- **Errors are typed**, not stringified — return `Result<T, YourError>` where the
  error is a serializable domain enum. Body errors come back as HTTP 200 with a
  `{"Err": ...}` payload so the client gets the typed variant.

## Request context — extractors

Inside a `#[server]` body, pull request-scoped data with the `use_*` extractors
instead of threading it through every call:

- `server::use_state::<T>()` — shared application state installed at startup.
- `server::use_request_header("authorization")` — a request header (auth, etc.).
- Auth guards: a missing `Auth` extractor makes the call respond `401`.

## Standing up the server

The server binary installs state and mounts the generated router:

```rust
// src/bin/server.rs (built with the `server` feature)
let app: axum::Router = server::router()       // registers every #[server] fn
    .with_state(/* your AppState */);
// serve `app` at /_srv/* with your HTTP host (axum, etc.)
```

`server::router()` walks the inventory of `#[server]` fns and registers
`/_srv/<fn>` plus `/_srv/_batch`. It warns at startup if it finds zero routes —
the usual symptom of a feature-unification miss where the macro stubbed out the
bodies.

## Pointing the client at the server

On the client, configure the origin once before the first call:

```rust
server::configure(server::ClientConfig::new("https://api.example.com"));
```

Then just call the function — `create_todo(input).await` — and it round-trips.

## Pairing with reactive state

Server calls compose with `async_reducer` / `resource` (see [[reactivity]]): a
reducer folds each server response into a `Signal<Vec<T>>`, so a "remove-by-id"
server fn conventionally **echoes the id back** on success and the client's apply
closure drops the matching local entry without re-fetching.

## Crate layout (recommended)

The provable boundary is strongest when server fns live in a crate that does NOT
depend on the UI runtime:

```
api/         #[server] fns + shared DTOs + the domain error enum.
             deps: server, serde (+ db/tokio gated behind a `server` feature).
             Does NOT depend on runtime-core — a body literally cannot name
             `Signal`, so the boundary is a compile error, not a convention.
ui/          The app. Depends on `api` (for the stubs) + the UI runtime.
server-bin/  Depends on `api` with `features = ["server"]`; calls router().
```

The reference implementation is `examples/server-fn-demo`; the full v1 design is
in `crates/api/server/DESIGN.md`.

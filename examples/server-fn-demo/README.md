# server-fn-demo

Full-stack todo app demonstrating the `server` SDK:

- `#[server]` functions (`list_todos`, `create_todo`, `toggle_todo`, `delete_todo`, `whoami`, `slow_op`) compile twice — real bodies on the server, RPC stubs on the client — from the same source.
- The client is an **idealyst UI** built to wasm.
- The server hosts both the API (`/_srv/*`) and the wasm bundle (`/`, `/pkg/*`).
- Open one URL — `http://127.0.0.1:3000/` — and you have a working full-stack app.

## Run

Two commands.

**1. Build the wasm client:**

```sh
idealyst build --web examples/server-fn-demo
```

That populates `examples/server-fn-demo/pkg/` with `server_fn_demo.js` + `server_fn_demo_bg.wasm`. Re-run any time the client code changes.

**2. Start the server:**

```sh
cargo run -p server-fn-demo --bin server --features server
```

Output:

```
server-fn-demo:
  UI       → http://127.0.0.1:3000/
  API      → http://127.0.0.1:3000/_srv/<fn-name>
  pkg/     → .../examples/server-fn-demo/pkg
```

Open the UI URL in a browser.

## What to look for

- The page loads, fetches `list_todos`, shows the (empty) list.
- Click `Add: Buy milk` (or any other add button) — fires `create_todo`, then re-fetches.
- Click a todo's `[ ] title` button — toggles the `done` flag via `toggle_todo`, re-fetches.
- Click `delete` — fires `delete_todo`, re-fetches.
- **Open devtools → Network tab.** You'll see `POST /_srv/<fn>` for solo calls and `POST /_srv/_batch` when multiple calls happen on the same tick (e.g. the on-mount fetch path).

## Layout

```
examples/server-fn-demo/
├── Cargo.toml          # one package, one bin + lib, dual-feature
├── index.html          # loads /pkg/server_fn_demo.js
├── src/
│   ├── lib.rs          # idealyst app() + #[server] fns + cfg-gated AppState
│   └── bin/
│       └── server.rs   # axum: /_srv/* (server::router) + /pkg/* + / (ServeDir)
└── pkg/                # produced by `idealyst build --web`, served at /pkg/
```

In a real production app you'd split this into three crates — `shared/` (types + `#[server]`), `server/` (bin), `client/` (one or more clients). The single-crate two-binary layout used here is concise for a demo but means a careless `cargo build --bins --features server` will compile the wrong macro half for one of them.

## Re-running

The server keeps state in memory. Restart the server to clear the todo list. The client bundle only needs rebuilding when client code changes.

## Macro hygiene

The `#[server]` macro keys off `feature = "server"` to choose between the real fn body and an RPC stub. Build commands must be separated per binary so cargo doesn't unify features:

```sh
# correct — separate invocations
idealyst build --web examples/server-fn-demo                    # client (no server feature)
cargo build -p server-fn-demo --bin server --features server    # server

# wrong — feature unification compiles the server body for the client
cargo build -p server-fn-demo --bins --features server
```

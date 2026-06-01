# server-fn-demo

Full-stack todo app demonstrating the `server` SDK:

- `#[server]` functions (`list_todos`, `create_todo`, `toggle_todo`, `delete_todo`, `whoami`, `slow_op`) compile twice — real bodies on the server, RPC stubs on the client — from the same source.
- A `#[server::sse]` endpoint (`ticks`) streams live Server-Sent Events, consumed on the client by `use_sse` — and on iOS/Android it drives the device's native `net::EventSource` arm.
- The client is an **idealyst UI** built to wasm (and to native via `idealyst dev --ios/--android`).
- The server hosts both the API (`/_srv/*`) and the wasm bundle (`/`, `/pkg/*`).
- Open one URL — `http://127.0.0.1:3000/` — and you have a working full-stack app.

## Run

Two commands.

**1. Build the wasm client:**

```sh
idealyst build --web examples/server-fn-demo
```

That emits a self-contained static bundle at `examples/server-fn-demo/dist/web/` (its own `index.html` plus a `pkg/` subdir with `server_fn_demo.js` + `server_fn_demo_bg.wasm`). The server serves that directory. Re-run any time the client code changes.

**2. Start the server:**

```sh
cargo run -p server-fn-demo --bin server --features server
```

Output:

```
server-fn-demo:
  UI       → http://127.0.0.1:3000/
  API      → http://127.0.0.1:3000/_srv/<fn-name>
  pkg/     → .../examples/server-fn-demo/dist/web/pkg
```

Open the UI URL in a browser.

## What to look for

- The page loads, fetches `list_todos`, shows the (empty) list.
- Click `Add: Buy milk` (or any other add button) — fires `create_todo`, then re-fetches.
- Click a todo's `[ ] title` button — toggles the `done` flag via `toggle_todo`, re-fetches.
- Click `delete` — fires `delete_todo`, re-fetches.
- **The `live SSE — tick #N` line counts up on its own**, ~2/second. That's the `#[sse] ticks` endpoint streaming over Server-Sent Events, consumed by `use_sse::<Tick>(ticks())` — the text re-renders on every event.
- **Open devtools → Network tab.** You'll see `POST /_srv/<fn>` for solo calls, `POST /_srv/_batch` when multiple calls happen on the same tick, and one long-lived `GET /_srv/_sse/ticks` (type `eventsource`) for the live stream.

## Live SSE on iOS / Android

The ticking line is the cross-platform `net::EventSource`. On the web it's the browser's `EventSource`; on iOS it's an `NSURLSession` streaming delegate; on Android it's a streaming `HttpURLConnection` read. To exercise the **native mobile arms** end-to-end:

> **Use `--local`.** In the default runtime-server dev mode the reactive tree (and therefore `use_sse`) runs on the *host* sidecar, so the device's native `EventSource` never runs. `--local` builds the app natively, so `use_sse` runs on the device — which is the whole point.

**1. Start the server on your host** (same as web):

```sh
cargo run -p server-fn-demo --bin server --features server
```

**2. Launch the app on a simulator / emulator:**

```sh
idealyst dev --ios --local examples/server-fn-demo       # iOS simulator
idealyst dev --android --local examples/server-fn-demo   # Android emulator
```

The app points at the host automatically: `http://127.0.0.1:3000` on the iOS simulator (it shares the host loopback) and `http://10.0.2.2:3000` on the Android emulator (its host-loopback alias) — see `configure_server()` in `src/lib.rs`.

Cleartext HTTP to those hosts is allowed because `idealyst dev`/`run` generates a dev-only exception (iOS `NSAllowsLocalNetworking`, Android `usesCleartextTraffic`); release packaging keeps the secure default.

**Physical device:** replace the loopback host in `configure_server()` with your machine's LAN IP (e.g. `http://192.168.1.20:3000`) and make sure the device is on the same network.

## Layout

```
examples/server-fn-demo/
├── Cargo.toml          # one package, one bin + lib, dual-feature
├── index.html          # source page (loads /pkg/server_fn_demo.js)
├── src/
│   ├── lib.rs          # idealyst app() + #[server]/#[sse] fns + cfg-gated AppState
│   └── bin/
│       └── server.rs   # axum: /_srv/* (server::router) + /pkg/* + / (ServeDir)
└── dist/web/           # produced by `idealyst build --web`; served as site root (+ pkg/)
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

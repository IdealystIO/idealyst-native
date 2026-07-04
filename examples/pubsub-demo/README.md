# pubsub-demo

Decentralized WebSocket fan-out on the [`pubsub`](../../crates/sdk/server/pubsub) SDK.

- [`src/lib.rs`](src/lib.rs) — a `#[server] say(from, text)` that publishes a
  `ChatMsg` to the `"room"` topic, a `#[subscription] room_feed()` that bridges
  that topic to a WebSocket (`ROOM.subscribe()`), and the client UI (a live feed
  + a send box). All `pubsub` use is behind `#[cfg(feature = "server")]`, so the
  wasm client never compiles the SDK.
- [`src/bin/server.rs`](src/bin/server.rs) — serves the UI + `/_srv/*` and
  configures the pub/sub backend from the environment.

## Run

```sh
idealyst dev --web examples/pubsub-demo
```

Open two tabs. Type in one → both feeds update (fan-out over the in-process
memory backend).

## Cross-instance (the point)

Uncomment `[pubsub]` in [`dev.toml`](dev.toml) (`backend = "redis"`) and run **two**
server instances on different ports against the same Redis:

```sh
cargo run -p pubsub-demo --bin server --features server   # PORT=3000
IDEALYST_PUBSUB_BACKEND=redis IDEALYST_PUBSUB_URL=redis://127.0.0.1:6379 \
  PORT=3001 cargo run -p pubsub-demo --bin server --features server
```

A client subscribed on `:3001` receives messages `say`-published on `:3000` —
the decentralized WebSocket notification path.

## Verify

The end-to-end fan-out (publish → backend → subscription → WebSocket client) is
covered by a test:

```sh
cargo test -p pubsub-demo --features server
```

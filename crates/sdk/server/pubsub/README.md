# `pubsub` — publish/subscribe SDK (server tier)

The broadcast sibling of `jobs`. Where `jobs` delivers each message to **one**
consumer (a durable work queue), `pubsub` fans a message out to **every** current
subscriber, fire-and-forget. Its headline use is **decentralized WebSocket
notifications**: a client holds a `#[subscription]` socket on one server
instance, an event is produced on another, and a shared backend carries it across
so the right instance delivers it.

```rust
use futures_util::Stream;

// A typed topic (no macro needed — every subscriber is an explicit call site).
const ROOM: pubsub::Topic<Msg> = pubsub::Topic::new("room");

// Bridge the topic to a WebSocket — a subscription body just returns its stream.
#[server::subscription]
async fn feed() -> impl Stream<Item = Msg> {
    ROOM.subscribe()
}

// Publish from anywhere server-side — any instance.
#[server]
async fn say(text: String) -> Result<(), ServerError> {
    ROOM.publish(&Msg { text }).await.map_err(ServerError::failed)?;
    Ok(())
}
```

A client connected to instance A's `feed` receives messages `say`-published on
instance B, because both go through the shared backend. **No changes to
`crates/api/server` are needed** — the `#[subscription]` macro already pumps any
`impl Stream` to the socket.

## Surface

- **`Topic<T>`** — `Topic::new("name")` (`const`) or `Topic::named(String)`
  (dynamic). `publish(&self, &T)` fans out; `subscribe(&self) -> impl Stream<Item = T>`
  drops into a `#[subscription]` body. `subscribe` is infallible-by-design (lazy
  connect, empty stream + log on error, skips undecodable payloads).
- **Free fns** `pubsub::publish::<T>(topic, &msg)` / `pubsub::subscribe::<T>(topic)`
  for runtime topic names.
- **`configure(backend)`** installs the process-wide backend; **`configure_from_env()`**
  reads `IDEALYST_PUBSUB_BACKEND` / `IDEALYST_PUBSUB_URL` (set by `idealyst dev`).
- **`PubSubBackend`** trait — implement your own transport.

## Backends (feature-gated)

| Feature    | Backend | Mechanism |
| ---------- | ------- | --------- |
| `memory`   | `MemoryBackend`   | In-process `tokio::sync::broadcast` per topic (default). **Single instance.** |
| `redis`    | `RedisBackend`    | Redis native Pub/Sub (`PUBLISH` / `SUBSCRIBE`; topic == channel). Cross-instance. |
| `postgres` | `PostgresBackend` | `LISTEN`/`NOTIFY` on one channel (`idealyst_pubsub`) with the topic prefixed into the payload. Cross-instance; NOTIFY payload ≤ ~8000 bytes (publish a reference for larger). |

## Semantics

**Fan-out, at-most-once, ephemeral.** Every *current* subscriber receives each
published message; there is no buffering or replay for a subscriber that isn't
connected at publish time. Durable, once-per-message delivery is what `jobs` is
for. The `memory` backend is per-process — the cross-instance value only appears
with a shared backend (redis/postgres).

## Server-tier only

`pubsub` runs in the server binary. Apps depend on it as an **optional** dep
enabled by their `server` feature, with all `pubsub` references behind
`#[cfg(feature = "server")]`, so the wasm client never compiles it. See
[`examples/pubsub-demo`](./examples/pubsub-demo).

## Testing checklist

`cargo test -p pubsub` runs the memory-backend suite (fan-out, topic isolation,
late-subscriber-misses, typed `Topic` roundtrip).
`crates/sdk/server/pubsub/examples/pubsub-demo` has a full end-to-end WS fan-out test
(`cargo test -p pubsub-demo --features server`).

| Backend    | Tests | Verification |
| ---------- | ----- | ------------ |
| `memory`   | 🧪 unit (fan-out / isolation / late-subscriber / typed topic) · 🔌 e2e WS fan-out (in `pubsub-demo`) | ✅ host-verified |
| `redis`    | 🖥️ host (`tests/redis_backend.rs`, `#[ignore]`) | ✅ **host-verified** against a live Redis |
| `postgres` | 🖥️ host (`tests/postgres_backend.rs`, `#[ignore]`) | ⚠️ compile-checked; ignored host test ready |

```sh
PUBSUB_TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test -p pubsub --features redis -- --ignored
PUBSUB_TEST_PG_URL=postgres://postgres@127.0.0.1/postgres cargo test -p pubsub --features postgres -- --ignored
```

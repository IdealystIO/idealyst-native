# `jobs` — background job / queue SDK (server tier)

Define a unit of deferred work once with `#[job]`; enqueue it with a typed,
compile-checked call site from anywhere server-side; a worker drains the queue
and runs it off the request path. The queue is pluggable (in-memory / Redis /
Postgres / SQS), and job handlers get the **same** `State<T>` / `#[ctx]`
dependency injection as `#[server]` fns (this crate builds on `server`).

```rust
use jobs::{job, JobError};
use server::State;

// 1. Define the job once. `to` is sent over the wire; `mailer` is injected.
#[job]
async fn send_email(to: String, #[ctx] mailer: State<Mailer>) -> Result<(), JobError> {
    mailer.send(&to).await.map_err(JobError::new)?;
    Ok(())
}

// 2. Enqueue it from a #[server] fn (or anywhere server-side).
#[server]
async fn signup(email: String) -> Result<(), ServerError> {
    send_email::enqueue(email).delay(Duration::from_secs(30)).await?;
    Ok(())
}

// 3. Drain it — dedicated worker process…
#[tokio::main]
async fn main() {
    jobs::configure(jobs::RedisBackend::connect("redis://127.0.0.1/").await.unwrap());
    jobs::worker().concurrency(8).run().await.unwrap();
}
// …or in-process next to the server:
//   let _worker = jobs::worker().spawn();
//   server::serve(addr).await;
```

## Shape

- **`QueueBackend`** — the queue abstraction: `enqueue` / `reserve` (lease with a
  visibility timeout) / `ack` / `retry` / `dead_letter`. `reserve` is a
  non-blocking poll, so the worker owns all pacing and one loop shape covers
  every backend.
- **`#[job]`** — generates a typed `Name::enqueue(args…)` builder (`.delay`,
  `.queue`, `.max_attempts`, `.backoff`) and, in the server/worker build,
  registers the handler via `inventory`. Gated on the invoking crate's `server`
  feature, exactly like `#[server]`.
- **`jobs::worker()`** — the runtime. `.run()` is a dedicated process (blocks
  until SIGINT/SIGTERM, then drains in-flight); `.spawn()` is an in-process
  background task. Retries with backoff on handler `Err` (or panic) until
  `max_attempts`, then dead-letters.
- **`jobs::configure_from_env()`** — reads `IDEALYST_JOBS_BACKEND` /
  `IDEALYST_JOBS_URL` (set by `idealyst dev` / `idealyst worker`) and installs
  the matching backend.

## Backends (feature-gated)

| Feature    | Backend        | Mechanism |
| ---------- | -------------- | --------- |
| `memory`   | `MemoryBackend`| In-process reference (default). Delay wheel + visibility-timeout leases + dead-letter. The correctness reference and test substrate. |
| `redis`    | `RedisBackend` | Reliable queue: ready `LIST` + scheduled/inflight `ZSET`s + per-job `HASH`; atomic reserve via one Lua script. At-least-once. |
| `postgres` | `PostgresBackend` | One table drained with `SELECT … FOR UPDATE SKIP LOCKED`; visibility via a `reserved_until` column. |
| `sqs`      | `SqsBackend`   | Native SQS: visibility timeout = lease, `ApproximateReceiveCount` = attempt, `ChangeMessageVisibility` = retry-with-delay. Pairs with `server-aws`. |

Backend features are independent of `worker` — a server that only *enqueues*
turns on (say) `redis` without the worker runtime.

## Deployment

- **Dedicated worker** — a `src/bin/worker.rs` that calls `jobs::worker().run()`.
  Run it with `idealyst worker`; `idealyst dev` auto-spawns it when `dev.toml`'s
  `[jobs]` block configures a shared broker.
- **In-process** — call `jobs::worker().spawn()` before `server::serve(addr)`.
  This is the right default for single-node dev and the in-memory backend (a
  separate process can't share an in-memory queue).

See [`examples/jobs-demo`](../../../../examples/jobs-demo) for the full-stack wiring.

## Testing checklist

`cargo test -p jobs --features server` runs the host-runnable suite (memory
backend state machine, the `#[job]` macro, and the worker loop end-to-end).

| Backend    | Tests | Native verification |
| ---------- | ----- | ------------------- |
| `memory`   | 🧪 unit (enqueue/reserve/ack, retry, delay, visibility reclaim, dead-letter, priority) · 🔌 integration (macro + worker: injected state, retry-then-succeed, dead-letter, unknown-job) | ✅ host-verified (also run live in `examples/jobs-demo`) |
| `redis`    | 🖥️ host (`tests/redis_backend.rs`, `#[ignore]`) | ✅ **host-verified** against a live Redis |
| `postgres` | 🖥️ host (`tests/postgres_backend.rs`, `#[ignore]`) | ⚠️ compile-checked; ignored host tests ready — run against a live Postgres |
| `sqs`      | 🖥️ host (`tests/sqs_backend.rs`, `#[ignore]`) | ⚠️ compile-checked; ignored host test ready — needs AWS credentials + a live queue |

Run the host tests against a live broker:

```sh
JOBS_TEST_REDIS_URL=redis://127.0.0.1:6379 cargo test -p jobs --features redis -- --ignored
JOBS_TEST_PG_URL=postgres://postgres@127.0.0.1/jobstest cargo test -p jobs --features postgres -- --ignored
JOBS_TEST_SQS_URL=https://sqs.us-east-1.amazonaws.com/…/q cargo test -p jobs --features sqs -- --ignored
```

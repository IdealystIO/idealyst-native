# jobs-demo

Full-stack background jobs on the [`jobs`](../../crates/sdk/server/jobs) SDK.

- [`src/lib.rs`](src/lib.rs) — a `#[job] send_welcome_email` (injects
  `State<Mailer>`), a `#[server] signup` fn that enqueues it, and the client UI
  (email field + Sign-up button).
- [`src/bin/server.rs`](src/bin/server.rs) — serves the UI + `/_srv/*` API and
  runs the worker **in-process** (memory backend), so it works with no broker.
- [`src/bin/worker.rs`](src/bin/worker.rs) — the dedicated worker `idealyst
  worker` runs and `idealyst dev` auto-spawns when a shared broker is configured.

## Run

```sh
idealyst dev --web crates/sdk/server/jobs/examples/jobs-demo
```

Type an address, press **Sign up** → `signup` enqueues the job → the worker logs
`[worker] sending welcome email to …` in the server console.

Or exercise just the backend end-to-end:

```sh
cargo run -p jobs-demo --bin server --features server
curl -X POST http://127.0.0.1:3000/_srv/signup \
  -H 'content-type: application/json' -d '["alice@example.com"]'
# → server console: [worker] sending welcome email to alice@example.com …
```

## Use a shared broker

Uncomment the `[jobs]` block in [`dev.toml`](dev.toml) (e.g. `backend = "redis"`)
and `idealyst dev` will spawn a **dedicated** worker process instead of the
in-process one, draining the same queue the server enqueues to.

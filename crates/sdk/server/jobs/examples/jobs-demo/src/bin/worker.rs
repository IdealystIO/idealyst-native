//! Dedicated jobs worker — the standalone drainer `idealyst worker` runs and
//! `idealyst dev` auto-spawns (when `dev.toml` configures a shared broker).
//!
//! ```text
//! cargo run -p jobs-demo --bin worker --features server
//! # or, with a shared backend:
//! IDEALYST_JOBS_BACKEND=redis IDEALYST_JOBS_URL=redis://127.0.0.1:6379 \
//!   cargo run -p jobs-demo --bin worker --features server
//! ```
//!
//! Only makes sense against a *shared* backend (redis/postgres/sqs) that the
//! server also enqueues to; with the in-memory backend the server runs the
//! worker in-process instead (see `src/bin/server.rs`).

use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use jobs_demo::Mailer;

#[tokio::main]
async fn main() {
    // The same state the job handlers inject. (In a real app this is the email
    // client; per-process is fine.)
    server::install_state(Mailer {
        from: "hello@jobs-demo.dev".to_string(),
        sent: Arc::new(AtomicU32::new(0)),
    });

    // Backend from the environment (set by the CLI). Defaults to memory, which
    // is per-process — a standalone worker really wants a shared broker.
    // `configure_all` reads the same `idealyst.toml` the server does, so the
    // worker drains the exact queue the server enqueues to.
    idealyst_config::configure_all()
        .await
        .expect("configure jobs backend from idealyst.toml");

    // Force-link anchor so the `#[job]` registrations survive linking.
    let _anchor = jobs_demo::signup;

    println!("[worker] draining jobs — Ctrl-C to stop");
    jobs::worker().run().await.expect("worker run");
}

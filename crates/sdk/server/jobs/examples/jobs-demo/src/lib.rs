//! `jobs-demo` — a **full-stack** background-jobs app on the [`jobs`] SDK.
//!
//! - A `#[job]` ([`send_welcome_email`]) is the unit of deferred work. Its body
//!   runs on a **worker**, off the request path, and injects app state
//!   (`State<Mailer>`) exactly like a `#[server]` fn.
//! - A `#[server]` fn ([`signup`]) enqueues the job with the typed
//!   `send_welcome_email::enqueue(..)` surface and returns immediately.
//! - The **server** bin runs an in-process worker over the in-memory backend,
//!   so `idealyst dev` works end-to-end with no external broker. Point
//!   `dev.toml`'s `[jobs]` block at Redis/Postgres/SQS and `idealyst dev`
//!   auto-spawns the dedicated **worker** bin instead (`src/bin/worker.rs`).
//!
//! ## Run it
//!
//! ```text
//! idealyst dev --web crates/sdk/server/jobs/examples/jobs-demo
//! ```
//! Type an address, press **Sign up** → the `#[server]` fn enqueues the job →
//! the worker logs `[worker] sending welcome email to …`.

use std::rc::Rc;
#[cfg(feature = "server")]
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(feature = "server")]
use std::sync::Arc;

use idea_ui::{
    install_idea_theme, light_theme, tone, typography_kind, variant, Button, Card, CardPadding,
    Field, Stack, StackAlign, StackAxis, StackGap, StackPadding, Typography,
};
// `JobError` is only named in the (server-gated) job body; allow it unused on
// the client build.
#[cfg_attr(not(feature = "server"), allow(unused_imports))]
use jobs::{job, JobError};
use runtime_core::{component, rx, signal, ui, Element};
#[cfg(feature = "server")]
use server::State;
use server::{server, ServerError};

// ============================================================================
// App state injected into job handlers (server/worker build only).
// ============================================================================

/// Shared state a job handler injects via `State<Mailer>` — the same
/// dependency-injection surface `#[server]` fns use. A real app would hold an
/// email-provider client here; this demo holds a from-address and a counter.
#[cfg(feature = "server")]
#[derive(Clone)]
pub struct Mailer {
    pub from: String,
    pub sent: Arc<AtomicU32>,
}

// ============================================================================
// The job. Defined ONCE; the wasm client compiles only its `enqueue` surface,
// the worker build compiles the body + registration.
// ============================================================================

/// Send a welcome email. Runs on a worker; `to` is the wire argument, `mailer`
/// is injected server-side.
#[job]
async fn send_welcome_email(to: String, #[ctx] mailer: State<Mailer>) -> Result<(), JobError> {
    let n = mailer.sent.fetch_add(1, Ordering::SeqCst) + 1;
    println!(
        "[worker] sending welcome email to {to} from {} (total sent: {n})",
        mailer.from
    );
    Ok(())
}

// ============================================================================
// The server function. On the wasm client it's an RPC stub; on the server it
// enqueues the job and returns immediately.
// ============================================================================

/// Queue a welcome email for `email`, returning right away — the send happens
/// later on a worker.
#[server]
pub async fn signup(email: String) -> Result<String, ServerError> {
    send_welcome_email::enqueue(email.clone())
        .await
        .map_err(|e| ServerError::failed(format!("could not queue email: {e}")))?;
    Ok(format!("Welcome email queued for {email}"))
}

/// Point the `server` SDK at the API host (see graphql-demo for the rationale).
fn configure_server() {
    #[cfg(target_arch = "wasm32")]
    {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());
        server::configure(server::ClientConfig::new(origin));
    }
    #[cfg(all(not(feature = "server"), not(target_arch = "wasm32")))]
    {
        server::configure(server::ClientConfig::new("http://127.0.0.1:3000"));
    }
}

// ============================================================================
// Client UI.
// ============================================================================

/// Root component: an email field + a Sign-up button that calls the `signup`
/// server fn and shows the result.
#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());
    configure_server();

    let email = signal(String::new());
    let status = signal(String::new());

    let on_email: Rc<dyn Fn(String)> = {
        let email = email.clone();
        Rc::new(move |v: String| email.set(v))
    };

    let submit: Rc<dyn Fn()> = {
        let email = email.clone();
        let status = status.clone();
        Rc::new(move || {
            let addr = email.get();
            if addr.trim().is_empty() {
                return;
            }
            let status = status.clone();
            status.set("queuing…".to_string());
            // Fire the server call; reflect the result reactively.
            runtime_core::driver::spawn_async(async move {
                match signup(addr).await {
                    Ok(msg) => status.set(msg),
                    Err(e) => status.set(format!("error: {e}")),
                }
            });
        })
    };

    ui! {
        Stack(gap = StackGap::Lg, padding = StackPadding::Lg, align = StackAlign::Center) {
            Typography(content = "Background Jobs Demo".to_string(), kind = typography_kind::H1)
            Typography(
                content = "Sign up enqueues a welcome-email job. A worker runs it off the \
                           request path — watch the server console for the send.".to_string(),
                muted = true,
            )
            Card(padding = CardPadding::Lg) {
                Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::End) {
                    Field(
                        label = Some("Email".to_string()),
                        value = email,
                        on_change = on_email,
                        placeholder = Some("you@example.com".to_string()),
                    )
                    Button(
                        label = "Sign up".to_string(),
                        on_click = submit,
                        tone = tone::Primary,
                        variant = variant::Filled,
                    )
                }
            }
            Typography(content = rx!(status.get()), muted = true)
        }
    }
}

// ============================================================================
// CLI-generated wrapper hooks.
// ============================================================================

/// SDK-registration hook the platform wrappers call before mount.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

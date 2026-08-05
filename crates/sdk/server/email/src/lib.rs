//! Server-tier transactional-email SDK. See `Cargo.toml` for the pitch.
//!
//! Compose a message — from raw strings or an idealyst template — and hand it
//! to the configured provider:
//!
//! ```ignore
//! use email::{Email, Mailbox};
//!
//! // ----- configure once at startup -----
//! email::configure(
//!     email::SesProvider::from_env().await.with_default_from("no-reply@app.dev"),
//! );
//!
//! // ----- send from anywhere server-side -----
//! email::send(
//!     Email::to("sam@example.com")
//!         .subject("Welcome!")
//!         .template(|| WelcomeEmail(WelcomeEmailProps { name: "Sam".into() })),
//! )
//! .await?;
//! ```
//!
//! # Shape
//!
//! - [`EmailProvider`] is the delivery abstraction; [`MemoryProvider`] is the
//!   in-process capture default (dev + tests). The SES provider is
//!   feature-gated (`ses`).
//! - [`Email`] is the message + a fluent builder. [`Email::template`] renders
//!   an idealyst component to email-safe HTML via `backend-email` (styles
//!   inlined, theme tokens resolved to literals) — no WASM.
//! - [`send`] validates + dispatches through the configured provider;
//!   [`configure`] / [`configure_from_env`] install it.

mod config;
mod error;
mod model;
mod provider;

pub use config::{configure, configure_from_env, configured_provider, send};
pub use error::EmailError;
pub use model::{Email, EmailId, Mailbox};
pub use provider::EmailProvider;

// Re-export the rendering surface so `email::render(...)` works without a
// separate `backend-email` dependency in app code. The render entries live
// in `backend_email::newcore` (the runtime-v2 render path); the names and
// signatures are unchanged, so this re-export keeps `email::render_email`
// as the stable author path regardless of where the backend hosts them.
pub use backend_email::newcore::{render_email, render_email_with};
pub use backend_email::{EmailBackend, RenderedEmail};

#[cfg(feature = "memory")]
mod memory;
#[cfg(feature = "memory")]
pub use memory::MemoryProvider;

#[cfg(feature = "ses")]
mod ses;
#[cfg(feature = "ses")]
pub use ses::SesProvider;

#[cfg(test)]
mod tests {
    use super::*;

    // `configure` installs a PROCESS-GLOBAL provider, and `send` reads it, so
    // tests that touch the global must not run concurrently. Hold this lock
    // for the whole test body (poison-tolerant — a panicking test shouldn't
    // wedge the rest). `#[tokio::test]` uses a current-thread runtime, so
    // holding the guard across `.await` is fine.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn mailbox_encodes_named_and_bare() {
        assert_eq!(Mailbox::new("a@b.com").encoded(), "a@b.com");
        assert_eq!(Mailbox::named("Sam Doe", "sam@b.com").encoded(), "Sam Doe <sam@b.com>");
        assert_eq!(Mailbox::from(("Sam", "sam@b.com")).encoded(), "Sam <sam@b.com>");
    }

    #[tokio::test]
    async fn send_captures_via_memory_provider() {
        let _g = serial();
        let provider = MemoryProvider::new().with_default_from("no-reply@app.dev");
        configure(provider.clone());

        let id = send(
            Email::to("sam@example.com")
                .subject("Hello")
                .html("<div>Hi</div>")
                .text("Hi"),
        )
        .await
        .expect("send ok");

        assert_eq!(id.0, "mem-1");
        let sent = provider.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to[0].address, "sam@example.com");
        assert_eq!(sent[0].subject, "Hello");
        // `from` was filled from the provider default.
        assert_eq!(sent[0].from.as_ref().unwrap().address, "no-reply@app.dev");
    }

    #[tokio::test]
    async fn send_rejects_message_without_body() {
        let _g = serial();
        configure(MemoryProvider::new().with_default_from("no-reply@app.dev"));
        let err = send(Email::to("sam@example.com").subject("Empty"))
            .await
            .expect_err("no body must fail");
        assert!(matches!(err, EmailError::Invalid(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn send_rejects_message_without_resolvable_sender() {
        let _g = serial();
        // A provider with no default_from + a message with no `from`.
        configure(MemoryProvider::new());
        let err = send(Email::to("sam@example.com").subject("x").text("x"))
            .await
            .expect_err("no sender must fail");
        assert!(matches!(err, EmailError::Invalid(_)), "got: {err:?}");
    }

    #[tokio::test]
    async fn template_renders_html_and_derives_subject() {
        let _g = serial();
        use runtime_core::{text, view};
        configure(MemoryProvider::new().with_default_from("no-reply@app.dev"));

        // A template with no explicit subject; body from the idealyst tree.
        let email = Email::to("sam@example.com")
            .template(|| view(vec![text("Welcome aboard").into()]).into());

        // HTML rendered inline, plaintext derived.
        assert!(email.html.as_ref().unwrap().contains("Welcome aboard"));
        assert!(email.html.as_ref().unwrap().starts_with("<!DOCTYPE html"));
        assert_eq!(email.text.as_deref(), Some("Welcome aboard"));

        send(email).await.expect("send ok");
    }

    #[tokio::test]
    async fn explicit_subject_survives_template() {
        let _g = serial();
        use runtime_core::{text, view};
        configure(MemoryProvider::new().with_default_from("no-reply@app.dev"));
        let email = Email::to("sam@example.com")
            .subject("Explicit")
            .template(|| view(vec![text("body").into()]).into());
        assert_eq!(email.subject, "Explicit");
    }
}

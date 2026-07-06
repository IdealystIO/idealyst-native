//! Process-wide provider configuration + the [`send`] entry point.
//!
//! Mirrors `jobs::configure` / `pubsub::configure`: an app installs one
//! provider at startup and every [`send`] uses it. Held in a
//! `OnceLock<RwLock<…>>` so it's set once and read cheaply from every task.

use crate::{Email, EmailError, EmailId, EmailProvider};
use std::sync::{Arc, OnceLock, RwLock};

static PROVIDER: OnceLock<RwLock<Option<Arc<dyn EmailProvider>>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<Arc<dyn EmailProvider>>> {
    PROVIDER.get_or_init(|| RwLock::new(None))
}

/// Install the process-wide email provider used by [`send`]. Call once at
/// startup. Calling again replaces the provider.
pub fn configure<P: EmailProvider + 'static>(provider: P) {
    *slot().write().unwrap() = Some(Arc::new(provider));
}

/// The configured provider, if [`configure`] has been called.
pub fn configured_provider() -> Option<Arc<dyn EmailProvider>> {
    slot().read().unwrap().clone()
}

/// Send an email through the configured provider.
///
/// Validates the message and resolves its sender first: if `email.from` is
/// `None` it's filled from the provider's [`EmailProvider::default_from`].
/// Fails with [`EmailError::Invalid`] if there's no recipient, no resolvable
/// sender, or no body part; with [`EmailError::NotConfigured`] if no provider
/// was installed.
pub async fn send(mut email: Email) -> Result<EmailId, EmailError> {
    let provider = configured_provider().ok_or(EmailError::NotConfigured)?;

    if email.from.is_none() {
        email.from = provider.default_from();
    }

    if email.to.is_empty() {
        return Err(EmailError::Invalid("no recipient (`to` is empty)".into()));
    }
    if email.from.is_none() {
        return Err(EmailError::Invalid(
            "no sender: message has no `from` and the provider has no default_from".into(),
        ));
    }
    if email.html.is_none() && email.text.is_none() {
        return Err(EmailError::Invalid(
            "no body: set `.html(...)`, `.text(...)`, or `.template(...)`".into(),
        ));
    }

    provider.send(&email).await
}

/// Configure the provider from the environment, then it's ready for [`send`].
/// Reads `IDEALYST_EMAIL_PROVIDER` (`memory` | `ses`, default `memory`) and
/// `IDEALYST_EMAIL_FROM` (an optional default sender address). This is the
/// bridge for `idealyst dev`, which sets those vars from a `[email]` block.
///
/// Errors if the selected provider's cargo feature isn't compiled in.
pub async fn configure_from_env() -> Result<(), EmailError> {
    let provider = std::env::var("IDEALYST_EMAIL_PROVIDER").unwrap_or_else(|_| "memory".to_string());
    let default_from = std::env::var("IDEALYST_EMAIL_FROM").ok();
    match provider.as_str() {
        "memory" => configure_memory(default_from),
        "ses" => configure_ses(default_from).await,
        other => Err(EmailError::Provider(format!(
            "unknown IDEALYST_EMAIL_PROVIDER `{other}` (expected memory|ses)"
        ))),
    }
}

fn feature_off(name: &str) -> EmailError {
    EmailError::Provider(format!(
        "email provider `{name}` selected via IDEALYST_EMAIL_PROVIDER, but its cargo feature \
         isn't enabled on the `email` dependency"
    ))
}

fn configure_memory(default_from: Option<String>) -> Result<(), EmailError> {
    #[cfg(feature = "memory")]
    {
        let mut p = crate::MemoryProvider::new();
        if let Some(from) = default_from {
            p = p.with_default_from(from);
        }
        configure(p);
        Ok(())
    }
    #[cfg(not(feature = "memory"))]
    {
        let _ = default_from;
        Err(feature_off("memory"))
    }
}

async fn configure_ses(default_from: Option<String>) -> Result<(), EmailError> {
    #[cfg(feature = "ses")]
    {
        let mut p = crate::SesProvider::from_env().await;
        if let Some(from) = default_from {
            p = p.with_default_from(from);
        }
        configure(p);
        Ok(())
    }
    #[cfg(not(feature = "ses"))]
    {
        let _ = default_from;
        Err(feature_off("ses"))
    }
}

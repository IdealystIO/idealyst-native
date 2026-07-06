//! The [`EmailProvider`] abstraction every backend implements.

use crate::{Email, EmailError, EmailId, Mailbox};
use async_trait::async_trait;

/// A pluggable email-delivery provider. `Send + Sync` so one instance is
/// shared behind an `Arc` across every send.
///
/// Implementors handle only transport — validation (recipient present,
/// subject non-empty, a body part exists, a sender is resolvable) is done once
/// in [`crate::send`] before dispatch, so a provider's `send` can assume a
/// well-formed [`Email`] whose `from` is already filled in.
#[async_trait]
pub trait EmailProvider: Send + Sync {
    /// Deliver `email`. `email.from` is guaranteed `Some` (resolved from the
    /// message or [`EmailProvider::default_from`] before this is called).
    async fn send(&self, email: &Email) -> Result<EmailId, EmailError>;

    /// A default sender to use when a message omits `from`. `None` (the
    /// default) means the message MUST carry its own `from`.
    fn default_from(&self) -> Option<Mailbox> {
        None
    }
}

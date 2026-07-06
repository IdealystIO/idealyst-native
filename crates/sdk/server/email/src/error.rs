//! Error type for the email SDK.

use thiserror::Error;

/// A failure sending an email.
#[derive(Debug, Error)]
pub enum EmailError {
    /// The provider (SES / …) reported an error.
    #[error("email provider error: {0}")]
    Provider(String),
    /// `email::configure(...)` was never called, so there's no provider.
    #[error("no email provider configured; call email::configure(...) at startup")]
    NotConfigured,
    /// The message is missing something required to send (no recipient, no
    /// sender + no provider default, empty subject, or no body part).
    #[error("invalid email: {0}")]
    Invalid(String),
}

impl EmailError {
    /// Build a `Provider` error from anything displayable.
    pub fn provider(msg: impl std::fmt::Display) -> Self {
        EmailError::Provider(msg.to_string())
    }
}

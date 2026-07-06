//! In-process capture provider — the default dev + test substrate.
//!
//! [`MemoryProvider`] never touches the network: it records every accepted
//! message in a shared buffer you can inspect. That makes it the right
//! provider for local development (see what your app *would* send), for the
//! test suite (assert on the composed message), and for a preview server that
//! renders the captured HTML.

use crate::{Email, EmailError, EmailId, EmailProvider, Mailbox};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// An in-process provider that captures sent emails instead of delivering
/// them. Clone-able and cheap; every clone shares the same capture buffer, so
/// you can hold one handle for assertions/preview while the app holds another.
#[derive(Clone, Default)]
pub struct MemoryProvider {
    sent: Arc<Mutex<Vec<Email>>>,
    counter: Arc<Mutex<u64>>,
    default_from: Option<Mailbox>,
}

impl MemoryProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a default sender used for messages that omit `from`.
    pub fn with_default_from(mut self, from: impl Into<Mailbox>) -> Self {
        self.default_from = Some(from.into());
        self
    }

    /// A snapshot of every captured message, oldest first.
    pub fn sent(&self) -> Vec<Email> {
        self.sent.lock().unwrap().clone()
    }

    /// The number of messages captured so far.
    pub fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }

    /// Drain and return all captured messages.
    pub fn take(&self) -> Vec<Email> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

#[async_trait]
impl EmailProvider for MemoryProvider {
    async fn send(&self, email: &Email) -> Result<EmailId, EmailError> {
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;
        let id = EmailId(format!("mem-{counter}"));
        self.sent.lock().unwrap().push(email.clone());
        Ok(id)
    }

    fn default_from(&self) -> Option<Mailbox> {
        self.default_from.clone()
    }
}

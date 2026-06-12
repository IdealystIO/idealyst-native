//! The SDK's error type.

use storage::StorageError;

/// A sync failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    /// The transport (the app's `pull`/`push` server fns) failed —
    /// network down, server error, timeout. **Retryable**: the engine
    /// keeps the work queued and tries again on the next flush. Carries a
    /// human-readable description.
    Transport(String),
    /// The server's entity schema is incompatible with this client (the
    /// framework's `x-srv-schema` drift check, HTTP 426). **Not**
    /// retryable — the engine halts push and forces a snapshot on the next
    /// pull until the app is upgraded.
    IncompatibleVersion,
    /// The local persistence layer ([`storage`]) failed.
    Storage(StorageError),
    /// A value could not be (de)serialized to/from its persisted or wire
    /// form. Indicates a programming error or corrupt store, not a
    /// transient condition.
    Codec(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Transport(m) => write!(f, "sync transport error: {m}"),
            SyncError::IncompatibleVersion => {
                write!(f, "sync server schema is incompatible with this client")
            }
            SyncError::Storage(e) => write!(f, "sync storage error: {e}"),
            SyncError::Codec(m) => write!(f, "sync codec error: {m}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<StorageError> for SyncError {
    fn from(e: StorageError) -> Self {
        SyncError::Storage(e)
    }
}

impl SyncError {
    /// True if retrying the same work later might succeed (transport
    /// blips). Schema/codec failures are terminal and return `false`.
    pub fn is_retryable(&self) -> bool {
        matches!(self, SyncError::Transport(_) | SyncError::Storage(_))
    }
}

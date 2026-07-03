//! Desktop secure store (Windows / Linux) via the OS credential vault,
//! backed by the `keyring` crate:
//!
//! - **Windows** → Credential Manager.
//! - **Linux** → the Secret Service (GNOME Keyring / KWallet) over D-Bus.
//!
//! Each secret is one vault entry keyed by `(service = namespace, account =
//! key)`. The vault is user-session-locked and encrypted at rest. There's no
//! browser-XSS surface on a desktop, so the OS vault — rather than a hardware
//! enclave — is the right bar here.
//!
//! "When possible": the Linux Secret Service needs a running keyring daemon
//! (a desktop login session). On a headless box with none, operations return
//! [`CredError::Backend`] — there is no secure store to use, and we don't
//! pretend otherwise.

use keyring::{Entry, Error as KeyringError};

use crate::{CredError, Credentials};

/// A [`Credentials`] over the OS credential vault, namespaced by service.
pub struct DesktopCredentials {
    service: String,
}

impl DesktopCredentials {
    pub fn new(namespace: &str) -> Self {
        Self {
            service: namespace.to_string(),
        }
    }

    fn entry(&self, key: &str) -> Result<Entry, CredError> {
        Entry::new(&self.service, key).map_err(map_err)
    }
}

fn map_err(e: KeyringError) -> CredError {
    CredError::Backend(format!("keyring: {e}"))
}

impl Credentials for DesktopCredentials {
    fn get(&self, key: &str) -> Result<Option<String>, CredError> {
        match self.entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), CredError> {
        self.entry(key)?.set_password(value).map_err(map_err)
    }

    fn remove(&self, key: &str) -> Result<(), CredError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }
}

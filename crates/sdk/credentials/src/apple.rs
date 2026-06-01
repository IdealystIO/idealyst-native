//! Keychain-backed secure store for iOS / macOS.
//!
//! Each secret is a Keychain **generic-password** item: `service` is the
//! store namespace, `account` is the key, the secret is the value's bytes.
//! The Keychain is OS-protected (Secure Enclave on supported devices), so
//! reading a secret back into the app's own memory is the correct, secure
//! way to use it — the OS gates access to the owning app.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use crate::{CredError, Credentials};

// OSStatus codes we special-case (from `<Security/SecBase.h>`).
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
const ERR_SEC_AUTH_FAILED: i32 = -25293;
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;
const ERR_SEC_USER_CANCELED: i32 = -128;

/// A [`Credentials`] over the platform Keychain, namespaced by service.
pub struct KeychainCredentials {
    service: String,
}

impl KeychainCredentials {
    pub fn new(namespace: &str) -> Self {
        Self {
            service: namespace.to_string(),
        }
    }
}

/// Map a Security-framework error: an access/interaction denial becomes
/// [`CredError::Denied`]; everything else is a [`CredError::Backend`] with
/// the OSStatus for diagnosis.
fn map_err(e: security_framework::base::Error) -> CredError {
    match e.code() {
        ERR_SEC_AUTH_FAILED | ERR_SEC_INTERACTION_NOT_ALLOWED | ERR_SEC_USER_CANCELED => {
            CredError::Denied
        }
        code => CredError::Backend(format!("Keychain OSStatus {code}: {e}")),
    }
}

impl Credentials for KeychainCredentials {
    fn get(&self, key: &str) -> Result<Option<String>, CredError> {
        match get_generic_password(&self.service, key) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|e| CredError::Backend(format!("secret is not valid UTF-8: {e}"))),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), CredError> {
        // Delete-then-add so an existing item is replaced cleanly,
        // independent of how the helper handles a duplicate item.
        let _ = delete_generic_password(&self.service, key);
        set_generic_password(&self.service, key, value.as_bytes()).map_err(map_err)
    }

    fn remove(&self, key: &str) -> Result<(), CredError> {
        match delete_generic_password(&self.service, key) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real round-trip against the host login Keychain. `#[ignore]` because
    /// it touches the machine's actual Keychain (and on some configs the
    /// first access can prompt); run it explicitly:
    ///
    /// ```text
    /// cargo test -p credentials --lib -- --ignored --nocapture
    /// ```
    ///
    /// Same-process add → read → delete doesn't prompt (the creating
    /// process owns the item). A unique service + final cleanup keep the
    /// Keychain tidy.
    #[test]
    #[ignore = "touches the host Keychain"]
    fn keychain_round_trips() {
        let kc = KeychainCredentials::new("ai.idealyst.credentials.selftest");
        // Clean any leftover from a previous aborted run.
        let _ = kc.remove("token");

        assert_eq!(kc.get("token").unwrap(), None);
        kc.set("token", "s3cr3t").unwrap();
        assert_eq!(kc.get("token").unwrap(), Some("s3cr3t".to_string()));
        kc.set("token", "rotated").unwrap();
        assert_eq!(kc.get("token").unwrap(), Some("rotated".to_string()));
        kc.remove("token").unwrap();
        assert_eq!(kc.get("token").unwrap(), None);
        // remove is idempotent.
        kc.remove("token").unwrap();
    }
}

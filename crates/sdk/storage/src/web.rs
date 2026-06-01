//! `localStorage`-backed plaintext store for the web.
//!
//! Keys are prefixed with the store namespace so several stores can share
//! the one origin-wide `localStorage` and `clear()` only wipes its own.
//!
//! Plaintext, and `localStorage` is readable by any script on the origin —
//! never put secrets here (see the crate docs).

use crate::{Storage, StorageError, StorageFuture};

/// A [`Storage`] over the browser's `localStorage`, namespaced by a key
/// prefix.
pub struct WebStorage {
    prefix: String,
}

impl WebStorage {
    pub fn new(namespace: &str) -> Self {
        Self {
            prefix: format!("{namespace}:"),
        }
    }

    fn local_storage() -> Result<web_sys::Storage, StorageError> {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .ok_or_else(|| StorageError::Backend("localStorage is unavailable".into()))
    }
}

// The async bodies below have no `.await` points and capture only
// `String`s, so although `web_sys::Storage` is `!Send`, it never crosses a
// suspension and the returned futures satisfy the trait's `Send` bound.
impl Storage for WebStorage {
    fn get(&self, key: &str) -> StorageFuture<'_, Option<String>> {
        let full = format!("{}{key}", self.prefix);
        Box::pin(async move {
            Self::local_storage()?
                .get_item(&full)
                .map_err(|e| StorageError::Backend(format!("get_item: {e:?}")))
        })
    }

    fn set(&self, key: &str, value: &str) -> StorageFuture<'_, ()> {
        let full = format!("{}{key}", self.prefix);
        let value = value.to_string();
        Box::pin(async move {
            Self::local_storage()?
                .set_item(&full, &value)
                .map_err(|e| StorageError::Backend(format!("set_item: {e:?}")))
        })
    }

    fn remove(&self, key: &str) -> StorageFuture<'_, ()> {
        let full = format!("{}{key}", self.prefix);
        Box::pin(async move {
            Self::local_storage()?
                .remove_item(&full)
                .map_err(|e| StorageError::Backend(format!("remove_item: {e:?}")))
        })
    }

    fn clear(&self) -> StorageFuture<'_, ()> {
        let prefix = self.prefix.clone();
        Box::pin(async move {
            let ls = Self::local_storage()?;
            let len = ls
                .length()
                .map_err(|e| StorageError::Backend(format!("length: {e:?}")))?;
            // Collect first — removing while iterating shifts indices.
            let mut to_remove = Vec::new();
            for i in 0..len {
                if let Ok(Some(k)) = ls.key(i) {
                    if k.starts_with(&prefix) {
                        to_remove.push(k);
                    }
                }
            }
            for k in to_remove {
                let _ = ls.remove_item(&k);
            }
            Ok(())
        })
    }
}

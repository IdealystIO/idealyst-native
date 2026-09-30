//! `localStorage`-backed plaintext store for the web.
//!
//! Keys are prefixed with the store namespace so several stores can share
//! the one origin-wide `localStorage` and `clear()` only wipes its own.
//!
//! Plaintext, and `localStorage` is readable by any script on the origin —
//! never put secrets here (see the crate docs).
//!
//! The browser calls are web-glue bindings declared here (own-web-bindings
//! phase 3). Every one that touches `localStorage` is `#[catch]`: reading
//! the property throws a `SecurityError` where storage is blocked (a
//! sandboxed iframe, some privacy modes) and `setItem` throws a
//! `QuotaExceededError` when the origin's quota is full. Both must come
//! back as a [`StorageError`], never unwind through the caller.

use web_glue::{string, JsError, JsValue};

use crate::{Storage, StorageError, StorageFuture};

web_glue::import! {
    // `window.localStorage`, or 0 without a window / when it is null.
    #[catch]
    fn js_local_storage() -> u32 =
        "() => { if (typeof window === 'undefined') return 0; \
           const s = window.localStorage; return s == null ? 0 : G.add(s); }";
    // 1 and the value written to `out` when present, 0 when absent.
    #[catch]
    fn js_get_item(s: u32, kp: usize, kl: usize, out: usize) -> u32 =
        "(s, kp, kl, o) => { const v = G.get(s).getItem(G.str(kp, kl)); \
           if (v == null) return 0; G.retStr(v, o); return 1; }";
    #[catch]
    fn js_set_item(s: u32, kp: usize, kl: usize, vp: usize, vl: usize) =
        "(s, kp, kl, vp, vl) => { G.get(s).setItem(G.str(kp, kl), G.str(vp, vl)); }";
    #[catch]
    fn js_remove_item(s: u32, kp: usize, kl: usize) =
        "(s, kp, kl) => { G.get(s).removeItem(G.str(kp, kl)); }";
    #[catch]
    fn js_length(s: u32) -> u32 = "(s) => G.get(s).length";
    // 1 and the key written to `out` when index `i` exists, 0 otherwise.
    #[catch]
    fn js_key(s: u32, i: u32, out: usize) -> u32 =
        "(s, i, o) => { const k = G.get(s).key(i); if (k == null) return 0; G.retStr(k, o); return 1; }";
}

/// A [`Storage`] over the browser's `localStorage`, namespaced by a key
/// prefix.
pub struct WebStorage {
    prefix: String,
}

/// The origin's `localStorage` handle.
struct LocalStorage(JsValue);

impl LocalStorage {
    fn get_item(&self, key: &str) -> Result<Option<String>, JsError> {
        let (kp, kl) = string::abi(key);
        let mut res = Ok(0);
        let s = string::receive(|o| res = unsafe { js_get_item(self.0.raw(), kp, kl, o) });
        res.map(|hit| (hit != 0).then_some(s))
    }

    fn set_item(&self, key: &str, value: &str) -> Result<(), JsError> {
        let (kp, kl) = string::abi(key);
        let (vp, vl) = string::abi(value);
        unsafe { js_set_item(self.0.raw(), kp, kl, vp, vl) }
    }

    fn remove_item(&self, key: &str) -> Result<(), JsError> {
        let (kp, kl) = string::abi(key);
        unsafe { js_remove_item(self.0.raw(), kp, kl) }
    }

    fn length(&self) -> Result<u32, JsError> {
        unsafe { js_length(self.0.raw()) }
    }

    fn key(&self, index: u32) -> Result<Option<String>, JsError> {
        let mut res = Ok(0);
        let s = string::receive(|o| res = unsafe { js_key(self.0.raw(), index, o) });
        res.map(|hit| (hit != 0).then_some(s))
    }
}

impl WebStorage {
    pub fn new(namespace: &str) -> Self {
        Self {
            prefix: format!("{namespace}:"),
        }
    }

    fn local_storage() -> Result<LocalStorage, StorageError> {
        match unsafe { js_local_storage() } {
            // SAFETY: a fresh `G.add` slot the snippet minted for us.
            Ok(h) if h != 0 => Ok(LocalStorage(unsafe { JsValue::from_raw(h) })),
            _ => Err(StorageError::Backend("localStorage is unavailable".into())),
        }
    }
}

// The async bodies below have no `.await` points and capture only
// `String`s, so although the `localStorage` handle is `!Send`, it never
// crosses a suspension and the returned futures satisfy the trait's `Send`
// bound.
impl Storage for WebStorage {
    fn get(&self, key: &str) -> StorageFuture<'_, Option<String>> {
        let full = format!("{}{key}", self.prefix);
        Box::pin(async move {
            Self::local_storage()?
                .get_item(&full)
                .map_err(|e| StorageError::Backend(format!("get_item: {e}")))
        })
    }

    fn set(&self, key: &str, value: &str) -> StorageFuture<'_, ()> {
        let full = format!("{}{key}", self.prefix);
        let value = value.to_string();
        Box::pin(async move {
            Self::local_storage()?
                .set_item(&full, &value)
                .map_err(|e| StorageError::Backend(format!("set_item: {e}")))
        })
    }

    fn remove(&self, key: &str) -> StorageFuture<'_, ()> {
        let full = format!("{}{key}", self.prefix);
        Box::pin(async move {
            Self::local_storage()?
                .remove_item(&full)
                .map_err(|e| StorageError::Backend(format!("remove_item: {e}")))
        })
    }

    fn clear(&self) -> StorageFuture<'_, ()> {
        let prefix = self.prefix.clone();
        Box::pin(async move {
            let ls = Self::local_storage()?;
            let len = ls
                .length()
                .map_err(|e| StorageError::Backend(format!("length: {e}")))?;
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

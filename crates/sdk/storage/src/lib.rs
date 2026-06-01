//! Cross-platform **insecure** key-value storage for non-sensitive app
//! data — preferences, UI state, caches.
//!
//! # This is NOT secure storage
//!
//! Everything written here is stored in the clear and is readable by
//! anything with access to the device/browser profile: other code in the
//! process, other scripts on the same web origin (so any XSS), anyone with
//! the device unlocked, a backup, etc. **Never put credentials, tokens,
//! keys, or any secret here.** There is deliberately no "secure" mode and
//! no encryption — a key-value store that *looked* secure but wasn't would
//! be worse than an honestly-insecure one.
//!
//! For secrets, use the `credentials` SDK, which is secure by construction
//! (OS Keychain/Keystore on native; httpOnly server session on web) and
//! errors loudly where real security isn't achievable rather than
//! pretending. This crate and that one are the storage analog of
//! `AsyncStorage` vs `SecureStore`.
//!
//! # API
//!
//! One async [`Storage`] trait, object-safe so an app holds an
//! `Arc<dyn Storage>` and the backend is chosen per platform. Get one for
//! the current platform with [`platform_storage`], or construct a specific
//! backend directly.
//!
//! Backends:
//! - [`platform_storage`] — the platform's native plaintext store:
//!   `localStorage` (web), `UserDefaults` (iOS/macOS),
//!   `SharedPreferences` (Android), a JSON file (Windows/Linux).
//! - [`MemoryStorage`] — in-process, all targets (tests / ephemeral state).
//! - [`FileStorage`] — a JSON file on disk; native targets only.
//!
//! ```ignore
//! use storage::platform_storage;
//!
//! let store = platform_storage("my_app");   // Arc<dyn Storage>
//! store.set("theme", "dark").await?;
//! assert_eq!(store.get("theme").await?, Some("dark".to_string()));
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

// Platform-native plaintext backends. Exactly one of `web`/`apple`/
// `android` is compiled per target; the rest of the targets fall back to
// `FileStorage` in [`platform_storage`].
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(all(not(target_arch = "wasm32"), any(target_os = "ios", target_os = "macos", target_os = "tvos")))]
mod apple;
#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
mod android;

/// A future returned by a [`Storage`] op. Boxed so the trait stays
/// object-safe (`Arc<dyn Storage>`).
pub type StorageFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, StorageError>> + Send + 'a>>;

/// A storage failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    /// The underlying backend failed (I/O, serialization, platform API).
    Backend(String),
    /// This backend doesn't support the operation on this platform.
    NotSupported,
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Backend(msg) => write!(f, "storage backend error: {msg}"),
            StorageError::NotSupported => write!(f, "storage operation not supported on this platform"),
        }
    }
}

impl std::error::Error for StorageError {}

/// Async key-value persistence. Values are `String`s — encode structured
/// data (e.g. JSON) by the caller.
pub trait Storage: Send + Sync {
    /// The value at `key`, or `None` if absent.
    fn get(&self, key: &str) -> StorageFuture<'_, Option<String>>;
    /// Store `value` at `key`, replacing any existing value.
    fn set(&self, key: &str, value: &str) -> StorageFuture<'_, ()>;
    /// Remove `key`. A no-op if it wasn't present.
    fn remove(&self, key: &str) -> StorageFuture<'_, ()>;
    /// Remove every key owned by this store.
    fn clear(&self) -> StorageFuture<'_, ()>;
}

// ---------------------------------------------------------------------------
// platform_storage — the native plaintext store for the current target.
// ---------------------------------------------------------------------------

/// An `Arc<dyn Storage>` over the current platform's native plaintext
/// key-value store, namespaced by `name`:
///
/// - **web** → `localStorage`, keys prefixed with `name`.
/// - **iOS / macOS** → `NSUserDefaults`, keys prefixed with `name`.
/// - **Android** → `SharedPreferences` file named `name`.
/// - **Windows / Linux** → a JSON [`FileStorage`] under the user's data dir.
///
/// `clear()` removes only this store's own keys. Construction is
/// infallible — backend errors surface per-operation. Remember: this is
/// **plaintext**; for secrets use the `credentials` SDK.
pub fn platform_storage(name: &str) -> Arc<dyn Storage> {
    #[cfg(target_arch = "wasm32")]
    return Arc::new(web::WebStorage::new(name));

    #[cfg(all(not(target_arch = "wasm32"), any(target_os = "ios", target_os = "macos", target_os = "tvos")))]
    return Arc::new(apple::UserDefaultsStorage::new(name));

    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    return Arc::new(android::SharedPrefsStorage::new(name));

    #[cfg(all(
        not(target_arch = "wasm32"),
        not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
        not(target_os = "android")
    ))]
    return Arc::new(FileStorage::new(default_file_path(name)));
}

/// Per-user data-dir path for the desktop [`FileStorage`] fallback
/// (Windows/Linux). Derives from the standard env vars, falling back to a
/// temp dir so a missing `HOME`/`APPDATA` never panics.
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
fn default_file_path(name: &str) -> std::path::PathBuf {
    use std::path::PathBuf;
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("idealyst")
        .join(name)
        .join("store.json")
}

// ---------------------------------------------------------------------------
// MemoryStorage — in-process, all targets.
// ---------------------------------------------------------------------------

/// An in-memory [`Storage`]. State lives for the process lifetime; useful
/// for tests and ephemeral state. Cheap to clone-share behind an `Arc`.
#[derive(Default)]
pub struct MemoryStorage {
    map: Mutex<HashMap<String, String>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Storage for MemoryStorage {
    fn get(&self, key: &str) -> StorageFuture<'_, Option<String>> {
        let value = self.map.lock().unwrap().get(key).cloned();
        Box::pin(async move { Ok(value) })
    }

    fn set(&self, key: &str, value: &str) -> StorageFuture<'_, ()> {
        self.map
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Box::pin(async move { Ok(()) })
    }

    fn remove(&self, key: &str) -> StorageFuture<'_, ()> {
        self.map.lock().unwrap().remove(key);
        Box::pin(async move { Ok(()) })
    }

    fn clear(&self) -> StorageFuture<'_, ()> {
        self.map.lock().unwrap().clear();
        Box::pin(async move { Ok(()) })
    }
}

// ---------------------------------------------------------------------------
// FileStorage — a JSON file on disk (native targets).
// ---------------------------------------------------------------------------

/// A [`Storage`] backed by a single JSON file holding the whole map.
///
/// Suitable for small key sets (auth tokens, preferences). Each mutation
/// rewrites the file; reads load it. Native targets only — `wasm32` has
/// no filesystem (use a `localStorage`-backed impl there).
///
/// The file I/O is synchronous inside the returned future. That's fine
/// for the small payloads this is meant for; a high-throughput caller
/// should front it with its own batching.
#[cfg(not(target_arch = "wasm32"))]
pub struct FileStorage {
    path: std::path::PathBuf,
    // Serialise concurrent writers to avoid lost updates on the file.
    lock: Mutex<()>,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileStorage {
    /// A store backed by the JSON file at `path`. The file is created on
    /// first write; a missing file reads as empty.
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> Result<HashMap<String, String>, StorageError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| StorageError::Backend(format!("decode: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(StorageError::Backend(format!("read: {e}"))),
        }
    }

    fn store(&self, map: &HashMap<String, String>) -> Result<(), StorageError> {
        let bytes =
            serde_json::to_vec(map).map_err(|e| StorageError::Backend(format!("encode: {e}")))?;
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&self.path, bytes).map_err(|e| StorageError::Backend(format!("write: {e}")))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Storage for FileStorage {
    fn get(&self, key: &str) -> StorageFuture<'_, Option<String>> {
        let key = key.to_string();
        Box::pin(async move {
            let _g = self.lock.lock().unwrap();
            Ok(self.load()?.get(&key).cloned())
        })
    }

    fn set(&self, key: &str, value: &str) -> StorageFuture<'_, ()> {
        let key = key.to_string();
        let value = value.to_string();
        Box::pin(async move {
            let _g = self.lock.lock().unwrap();
            let mut map = self.load()?;
            map.insert(key, value);
            self.store(&map)
        })
    }

    fn remove(&self, key: &str) -> StorageFuture<'_, ()> {
        let key = key.to_string();
        Box::pin(async move {
            let _g = self.lock.lock().unwrap();
            let mut map = self.load()?;
            map.remove(&key);
            self.store(&map)
        })
    }

    fn clear(&self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            let _g = self.lock.lock().unwrap();
            self.store(&HashMap::new())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_round_trips() {
        let s = MemoryStorage::new();
        assert_eq!(s.get("k").await.unwrap(), None);
        s.set("k", "v").await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), Some("v".to_string()));
        s.set("k", "v2").await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), Some("v2".to_string()));
        s.remove("k").await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn memory_clear() {
        let s = MemoryStorage::new();
        s.set("a", "1").await.unwrap();
        s.set("b", "2").await.unwrap();
        s.clear().await.unwrap();
        assert_eq!(s.get("a").await.unwrap(), None);
        assert_eq!(s.get("b").await.unwrap(), None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn file_persists_across_instances() {
        // Unique path per test run; cleaned up at the end.
        let path = std::env::temp_dir().join("idealyst_storage_test_persist.json");
        let _ = std::fs::remove_file(&path);

        {
            let s = FileStorage::new(&path);
            s.set("token", "abc").await.unwrap();
        }
        // A fresh instance over the same file sees the persisted value.
        {
            let s = FileStorage::new(&path);
            assert_eq!(s.get("token").await.unwrap(), Some("abc".to_string()));
            s.remove("token").await.unwrap();
            assert_eq!(s.get("token").await.unwrap(), None);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn file_missing_reads_empty() {
        let path = std::env::temp_dir().join("idealyst_storage_test_missing.json");
        let _ = std::fs::remove_file(&path);
        let s = FileStorage::new(&path);
        assert_eq!(s.get("nope").await.unwrap(), None);
    }

    /// `Arc<dyn Storage>` must work — the object-safe shape apps use.
    #[tokio::test]
    async fn object_safe_behind_arc() {
        let s: std::sync::Arc<dyn Storage> = std::sync::Arc::new(MemoryStorage::new());
        s.set("k", "v").await.unwrap();
        assert_eq!(s.get("k").await.unwrap(), Some("v".to_string()));
    }

    /// `platform_storage` returns a working store on whatever host runs
    /// the tests. On macOS that exercises the real `NSUserDefaults`
    /// backend end-to-end; on Linux/Windows it's the JSON `FileStorage`.
    /// A unique namespace + a final `clear()` keep the host's real
    /// defaults/file clean.
    #[tokio::test]
    async fn platform_storage_round_trips_on_host() {
        let store = platform_storage("idealyst_storage_selftest");
        store.clear().await.unwrap(); // start from a known-empty state

        assert_eq!(store.get("greeting").await.unwrap(), None);
        store.set("greeting", "hello").await.unwrap();
        assert_eq!(
            store.get("greeting").await.unwrap(),
            Some("hello".to_string())
        );
        store.set("greeting", "hi").await.unwrap();
        assert_eq!(store.get("greeting").await.unwrap(), Some("hi".to_string()));
        store.remove("greeting").await.unwrap();
        assert_eq!(store.get("greeting").await.unwrap(), None);

        // clear() removes only this store's keys.
        store.set("a", "1").await.unwrap();
        store.set("b", "2").await.unwrap();
        store.clear().await.unwrap();
        assert_eq!(store.get("a").await.unwrap(), None);
        assert_eq!(store.get("b").await.unwrap(), None);
    }
}

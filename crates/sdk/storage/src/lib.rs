//! Cross-platform persistent key-value storage.
//!
//! The primitive that server-function auth uses to persist a token on
//! native targets (where there is no browser cookie jar), and that apps
//! use for preferences. One async [`Storage`] trait, object-safe so an
//! app can hold an `Arc<dyn Storage>` and swap backends per platform.
//!
//! Built-in backends:
//! - [`MemoryStorage`] — in-process, all targets (tests / ephemeral state).
//! - [`FileStorage`] — a JSON file on disk; native targets only.
//!
//! Per-platform persistent / secure backends (web `localStorage`, iOS
//! `UserDefaults`/Keychain, Android `SharedPreferences`/Keystore)
//! implement the same trait. They're the next step; the trait and the
//! two portable backends here are implemented and tested.
//!
//! ```ignore
//! use storage::{Storage, MemoryStorage};
//!
//! let store = MemoryStorage::new();
//! store.set("token", "abc").await?;
//! assert_eq!(store.get("token").await?, Some("abc".to_string()));
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

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
}

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
//!
//! # Values needed before the first frame
//!
//! A saved theme read asynchronously boots in the default and flips a tick
//! later — a visible flash. [`Storage::get_now`] / [`Storage::set_now`] /
//! [`Storage::remove_now`] answer synchronously instead. Every store this
//! crate ships is synchronous underneath, so they always answer; a custom
//! store that would have to wait returns [`StorageError::NotSupported`]
//! rather than blocking.
//!
//! ```ignore
//! let store = platform_storage("my_app");
//! let dark = store.get_now("theme").ok().flatten().as_deref() == Some("dark");
//! ```

#![deny(missing_docs)]

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

/// `persisted_signal` — hydrate-from-storage + persist-on-change with
/// user-writes-win race semantics. Opt-in via the `reactive` feature.
#[cfg(feature = "reactive")]
pub mod persisted;
#[cfg(feature = "reactive")]
pub use persisted::{persisted_signal, persisted_signal_with};

/// Compile-checked usage recipes (docs / MCP catalog). The `recipe!` macro
/// self-gates on the `catalog` feature, so this is empty in production.
#[cfg(feature = "catalog")]
pub mod recipes;

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

    /// [`get`](Self::get), answered **now** instead of awaited — for a
    /// value that has to be known before the first frame: a saved theme
    /// (booting light and flipping to dark a tick later is a visible
    /// flash), a collapsed sidebar, a remembered layout.
    ///
    /// Every store this crate ships is synchronous underneath
    /// (`localStorage`, `NSUserDefaults`, `SharedPreferences`, a file, a
    /// map) and returns a future that is already complete, so this always
    /// answers for them. A store that genuinely has to wait (your own
    /// `Storage` over a network, say) answers
    /// [`StorageError::NotSupported`] rather than blocking — there is no
    /// executor to block on in a browser, and a UI thread must not.
    ///
    /// Prefer [`get`](Self::get) (or `persisted_signal`) for anything that
    /// can arrive after first paint; this is for the handful of values
    /// that can't.
    fn get_now(&self, key: &str) -> Result<Option<String>, StorageError> {
        resolve_now(self.get(key))
    }

    /// [`set`](Self::set), completed **now** — the write-side twin of
    /// [`get_now`](Self::get_now), so a preference toggled in an event
    /// handler is persisted before anything reacting to it runs (and a
    /// reload in the same tick cannot come back with the old value).
    /// Same [`StorageError::NotSupported`] contract for a store that
    /// would have to wait.
    fn set_now(&self, key: &str, value: &str) -> Result<(), StorageError> {
        resolve_now(self.set(key, value))
    }

    /// [`remove`](Self::remove), completed **now**. Same contract as
    /// [`get_now`](Self::get_now).
    fn remove_now(&self, key: &str) -> Result<(), StorageError> {
        resolve_now(self.remove(key))
    }
}

/// Poll a storage future exactly once and take its answer if it has one.
///
/// Sound for any future — a `Pending` result just means "this store
/// can't answer synchronously", reported as
/// [`StorageError::NotSupported`]. The no-op waker is correct precisely
/// because we never poll again: nothing waits on the wake-up. (The
/// dropped future may leave a write half-done only for a store that
/// suspends mid-write, and that store already told the caller it could
/// not do this synchronously.)
fn resolve_now<T>(fut: StorageFuture<'_, T>) -> Result<T, StorageError> {
    let mut fut = fut;
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => Err(StorageError::NotSupported),
    }
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
    /// Create an empty in-memory store.
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

    /// The synchronous accessors answer for the shipped stores (whose
    /// futures complete on first poll) and see the same data as the
    /// async API — they are views of one store, not a second one.
    #[test]
    fn now_accessors_round_trip_and_share_state_with_async() {
        let s = MemoryStorage::new();
        assert_eq!(s.get_now("k"), Ok(None));
        s.set_now("k", "v").unwrap();
        assert_eq!(s.get_now("k"), Ok(Some("v".to_string())));
        assert_eq!(pollster::block_on(s.get("k")), Ok(Some("v".to_string())));
        pollster::block_on(s.set("k", "w")).unwrap();
        assert_eq!(s.get_now("k"), Ok(Some("w".to_string())));
        s.remove_now("k").unwrap();
        assert_eq!(s.get_now("k"), Ok(None));
    }

    /// A store that genuinely has to wait must say so, not block the UI
    /// thread and not pretend the key is absent (`Ok(None)` would boot
    /// the app with the default and silently drop the user's choice).
    #[test]
    fn now_accessors_report_not_supported_for_a_store_that_would_wait() {
        struct Slow;
        impl Storage for Slow {
            fn get(&self, _: &str) -> StorageFuture<'_, Option<String>> {
                Box::pin(std::future::pending())
            }
            fn set(&self, _: &str, _: &str) -> StorageFuture<'_, ()> {
                Box::pin(std::future::pending())
            }
            fn remove(&self, _: &str) -> StorageFuture<'_, ()> {
                Box::pin(std::future::pending())
            }
            fn clear(&self) -> StorageFuture<'_, ()> {
                Box::pin(std::future::pending())
            }
        }
        assert_eq!(Slow.get_now("k"), Err(StorageError::NotSupported));
        assert_eq!(Slow.set_now("k", "v"), Err(StorageError::NotSupported));
        assert_eq!(Slow.remove_now("k"), Err(StorageError::NotSupported));
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

    /// The file store's synchronous accessors hit the same file: a
    /// value written with `set_now` is on disk for the next instance.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn file_now_accessors_persist_across_instances() {
        let path = std::env::temp_dir().join("idealyst_storage_test_now.json");
        let _ = std::fs::remove_file(&path);
        FileStorage::new(&path).set_now("theme", "dark").unwrap();
        let reopened = FileStorage::new(&path);
        assert_eq!(reopened.get_now("theme"), Ok(Some("dark".to_string())));
        reopened.remove_now("theme").unwrap();
        assert_eq!(FileStorage::new(&path).get_now("theme"), Ok(None));
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

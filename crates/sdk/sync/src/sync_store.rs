//! The persistence **port** — the trait the engine writes through, plus
//! two tested default implementations.
//!
//! The engine needs to load and save three per-partition concerns: the
//! record cache, the outbox, and the cursor. [`SyncStore`] abstracts
//! exactly those operations as opaque serialized strings, so the backing
//! store is pluggable: a key/value store, a local SQLite database, flat
//! files, an encrypted vault — anything that can persist a handful of
//! named blobs per partition.
//!
//! Two defaults ship:
//!
//! - [`KvSyncStore`] — over any [`storage::Storage`], so the platform
//!   native store (`localStorage` / `UserDefaults` / `SharedPreferences` /
//!   a file) backs sync with one line. This is the production default.
//! - [`MemorySyncStore`] — a dependency-light in-memory store for tests
//!   and ephemeral state.
//!
//! ## Why the engine keeps write ordering, not the backend
//!
//! Crash-safety depends on *ordering* writes (outbox before records on a
//! local edit; records before the outbox pop on an ack; cursor last on a
//! pull). That ordering is the engine's job — it makes the granular
//! `save_*` calls in the right sequence. A backend only has to make each
//! individual `save_*` durable; it never has to understand the protocol.
//! (A backend that *can* offer cross-call atomicity — e.g. a SQLite
//! transaction spanning a whole operation — is free to, but isn't
//! required to.)
//!
//! ## A note on granularity
//!
//! Records are saved as one whole-partition blob today. That keeps each
//! save a single atomic write and matches the "download one project"
//! scale this SDK targets. A future richer port could expose per-record
//! upsert/delete so a SQLite backend stores one row per record without
//! rewriting the partition on every change; that optimization is
//! deliberately out of scope for v1 and would layer on without changing
//! the engine's ordering rules.

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use storage::Storage;

use crate::error::SyncError;

/// A boxed future returned by a [`SyncStore`] op. Non-`Send`, matching the
/// SDK's single-threaded async model.
pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, SyncError>> + 'a>>;

/// The persistence port. Implement this to back sync with any store.
///
/// Each method targets one named concern of one partition. Loads return
/// `None` for a never-written concern (never an error). The engine
/// sequences these calls to uphold crash-safety; an implementation only
/// has to make each individual write durable.
pub trait SyncStore {
    /// Load the serialized record cache for `partition`.
    fn load_records(&self, partition: &str) -> StoreFuture<'_, Option<String>>;
    /// Persist the serialized record cache for `partition`.
    fn save_records(&self, partition: &str, records_json: &str) -> StoreFuture<'_, ()>;
    /// Load the serialized outbox for `partition`.
    fn load_outbox(&self, partition: &str) -> StoreFuture<'_, Option<String>>;
    /// Persist the serialized outbox for `partition`.
    fn save_outbox(&self, partition: &str, outbox_json: &str) -> StoreFuture<'_, ()>;
    /// Load the serialized cursor for `partition`.
    fn load_cursor(&self, partition: &str) -> StoreFuture<'_, Option<String>>;
    /// Persist the serialized cursor for `partition`.
    fn save_cursor(&self, partition: &str, cursor_json: &str) -> StoreFuture<'_, ()>;
    /// Drop every persisted concern for `partition`.
    fn clear(&self, partition: &str) -> StoreFuture<'_, ()>;
}

// ---------------------------------------------------------------------------
// KvSyncStore — over any storage::Storage
// ---------------------------------------------------------------------------

/// A [`SyncStore`] backed by any [`storage::Storage`] (the production
/// default). Stores each concern under `sync/<partition>/<concern>`.
///
/// ```ignore
/// let store = KvSyncStore::new(storage::platform_storage("sync"));
/// let engine = SyncEngine::new(std::sync::Arc::new(store), device_id);
/// // or the convenience shortcut:
/// let engine = SyncEngine::with_kv(storage::platform_storage("sync"), device_id);
/// ```
pub struct KvSyncStore {
    storage: Arc<dyn Storage>,
}

impl KvSyncStore {
    /// Wrap a [`storage::Storage`] as a [`SyncStore`].
    pub fn new(storage: Arc<dyn Storage>) -> Self {
        KvSyncStore { storage }
    }

    fn key(partition: &str, concern: &str) -> String {
        format!("sync/{partition}/{concern}")
    }
}

impl SyncStore for KvSyncStore {
    fn load_records(&self, partition: &str) -> StoreFuture<'_, Option<String>> {
        let key = Self::key(partition, "cache");
        Box::pin(async move { Ok(self.storage.get(&key).await?) })
    }

    fn save_records(&self, partition: &str, records_json: &str) -> StoreFuture<'_, ()> {
        let key = Self::key(partition, "cache");
        let value = records_json.to_string();
        Box::pin(async move { Ok(self.storage.set(&key, &value).await?) })
    }

    fn load_outbox(&self, partition: &str) -> StoreFuture<'_, Option<String>> {
        let key = Self::key(partition, "outbox");
        Box::pin(async move { Ok(self.storage.get(&key).await?) })
    }

    fn save_outbox(&self, partition: &str, outbox_json: &str) -> StoreFuture<'_, ()> {
        let key = Self::key(partition, "outbox");
        let value = outbox_json.to_string();
        Box::pin(async move { Ok(self.storage.set(&key, &value).await?) })
    }

    fn load_cursor(&self, partition: &str) -> StoreFuture<'_, Option<String>> {
        let key = Self::key(partition, "cursor");
        Box::pin(async move { Ok(self.storage.get(&key).await?) })
    }

    fn save_cursor(&self, partition: &str, cursor_json: &str) -> StoreFuture<'_, ()> {
        let key = Self::key(partition, "cursor");
        let value = cursor_json.to_string();
        Box::pin(async move { Ok(self.storage.set(&key, &value).await?) })
    }

    fn clear(&self, partition: &str) -> StoreFuture<'_, ()> {
        let cache = Self::key(partition, "cache");
        let outbox = Self::key(partition, "outbox");
        let cursor = Self::key(partition, "cursor");
        Box::pin(async move {
            self.storage.remove(&cache).await?;
            self.storage.remove(&outbox).await?;
            self.storage.remove(&cursor).await?;
            Ok(())
        })
    }
}

// ---------------------------------------------------------------------------
// MemorySyncStore — dependency-light in-memory
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Slots {
    records: Option<String>,
    outbox: Option<String>,
    cursor: Option<String>,
}

/// An in-memory [`SyncStore`] for tests and ephemeral state. State lives
/// for the process lifetime. Not `Send`/`Sync` (single-threaded, like the
/// rest of the SDK).
#[derive(Default)]
pub struct MemorySyncStore {
    partitions: RefCell<HashMap<String, Slots>>,
}

impl MemorySyncStore {
    /// An empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SyncStore for MemorySyncStore {
    fn load_records(&self, partition: &str) -> StoreFuture<'_, Option<String>> {
        let v = self
            .partitions
            .borrow()
            .get(partition)
            .and_then(|s| s.records.clone());
        Box::pin(async move { Ok(v) })
    }

    fn save_records(&self, partition: &str, records_json: &str) -> StoreFuture<'_, ()> {
        self.partitions
            .borrow_mut()
            .entry(partition.to_string())
            .or_default()
            .records = Some(records_json.to_string());
        Box::pin(async move { Ok(()) })
    }

    fn load_outbox(&self, partition: &str) -> StoreFuture<'_, Option<String>> {
        let v = self
            .partitions
            .borrow()
            .get(partition)
            .and_then(|s| s.outbox.clone());
        Box::pin(async move { Ok(v) })
    }

    fn save_outbox(&self, partition: &str, outbox_json: &str) -> StoreFuture<'_, ()> {
        self.partitions
            .borrow_mut()
            .entry(partition.to_string())
            .or_default()
            .outbox = Some(outbox_json.to_string());
        Box::pin(async move { Ok(()) })
    }

    fn load_cursor(&self, partition: &str) -> StoreFuture<'_, Option<String>> {
        let v = self
            .partitions
            .borrow()
            .get(partition)
            .and_then(|s| s.cursor.clone());
        Box::pin(async move { Ok(v) })
    }

    fn save_cursor(&self, partition: &str, cursor_json: &str) -> StoreFuture<'_, ()> {
        self.partitions
            .borrow_mut()
            .entry(partition.to_string())
            .or_default()
            .cursor = Some(cursor_json.to_string());
        Box::pin(async move { Ok(()) })
    }

    fn clear(&self, partition: &str) -> StoreFuture<'_, ()> {
        self.partitions.borrow_mut().remove(partition);
        Box::pin(async move { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage::MemoryStorage;

    // The same conformance suite, run against both default impls.
    async fn round_trips(store: &dyn SyncStore) {
        assert_eq!(store.load_records("p").await.unwrap(), None);
        store.save_records("p", "[1,2]").await.unwrap();
        assert_eq!(store.load_records("p").await.unwrap().as_deref(), Some("[1,2]"));

        store.save_outbox("p", "[]").await.unwrap();
        assert_eq!(store.load_outbox("p").await.unwrap().as_deref(), Some("[]"));

        store.save_cursor("p", "\"rev:1\"").await.unwrap();
        assert_eq!(store.load_cursor("p").await.unwrap().as_deref(), Some("\"rev:1\""));

        // Isolation between partitions.
        assert_eq!(store.load_records("other").await.unwrap(), None);

        // Clear drops everything for the partition.
        store.clear("p").await.unwrap();
        assert_eq!(store.load_records("p").await.unwrap(), None);
        assert_eq!(store.load_outbox("p").await.unwrap(), None);
        assert_eq!(store.load_cursor("p").await.unwrap(), None);
    }

    #[tokio::test]
    async fn kv_store_conforms() {
        let store = KvSyncStore::new(Arc::new(MemoryStorage::new()));
        round_trips(&store).await;
    }

    #[tokio::test]
    async fn memory_store_conforms() {
        let store = MemorySyncStore::new();
        round_trips(&store).await;
    }
}

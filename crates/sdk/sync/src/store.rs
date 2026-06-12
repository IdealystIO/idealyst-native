//! Crash-safe persistence over the [`storage`] key/value SDK.
//!
//! The whole crash-safety model rests on one fact: a single
//! [`Storage::set`](storage::Storage::set) is atomic; nothing spanning two
//! keys is. There is no batch, no transaction, no atomic rename. So this
//! layer stores each of a partition's three pieces of state as **one blob
//! at one key**, and every state transition is therefore a single atomic
//! `set`:
//!
//! - `sync/<partition>/cache`  → the full record set (a JSON array).
//! - `sync/<partition>/outbox` → the pending-mutation queue (a JSON array).
//! - `sync/<partition>/cursor` → the opaque sync cursor.
//!
//! Storing the whole partition cache as one value (rather than a key per
//! record) is deliberate: [`Storage`](storage::Storage) has no key
//! enumeration, so a per-record layout couldn't be loaded on boot anyway —
//! and the single-blob form makes applying a pull page commit atomically.
//! It trades a whole-partition rewrite per change for that atomicity,
//! which is the right call at the "download one project" scale this SDK
//! targets. The engine sequences the three keys to uphold the invariants
//! (record-data before cursor; record-state before outbox pop).

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use storage::Storage;

use crate::error::SyncError;
use crate::model::{Cursor, Record};
use crate::outbox::OutboxOp;

/// Per-partition view over the shared [`Storage`]. Cheap to construct.
pub(crate) struct PartitionStore {
    storage: Arc<dyn Storage>,
    partition: String,
}

impl PartitionStore {
    /// Bind a store to one partition over the engine's shared storage.
    pub fn new(storage: Arc<dyn Storage>, partition: impl Into<String>) -> Self {
        PartitionStore {
            storage,
            partition: partition.into(),
        }
    }

    fn cache_key(&self) -> String {
        format!("sync/{}/cache", self.partition)
    }

    fn outbox_key(&self) -> String {
        format!("sync/{}/outbox", self.partition)
    }

    fn cursor_key(&self) -> String {
        format!("sync/{}/cursor", self.partition)
    }

    /// Load the cached record set. A never-written partition reads as an
    /// empty vec, never an error.
    pub async fn load_cache<T: DeserializeOwned>(&self) -> Result<Vec<Record<T>>, SyncError> {
        match self.storage.get(&self.cache_key()).await? {
            Some(json) => decode(&json),
            None => Ok(Vec::new()),
        }
    }

    /// Persist the whole cached record set in one atomic `set`.
    pub async fn save_cache<T: Serialize>(&self, records: &[Record<T>]) -> Result<(), SyncError> {
        let json = encode(records)?;
        self.storage.set(&self.cache_key(), &json).await?;
        Ok(())
    }

    /// Load the pending-mutation queue. Empty for a fresh partition.
    pub async fn load_outbox(&self) -> Result<Vec<OutboxOp>, SyncError> {
        match self.storage.get(&self.outbox_key()).await? {
            Some(json) => decode(&json),
            None => Ok(Vec::new()),
        }
    }

    /// Persist the whole outbox queue in one atomic `set`.
    pub async fn save_outbox(&self, ops: &[OutboxOp]) -> Result<(), SyncError> {
        let json = encode(ops)?;
        self.storage.set(&self.outbox_key(), &json).await?;
        Ok(())
    }

    /// Load the last persisted cursor, or `None` if the partition was
    /// never pulled (which forces a snapshot on the first pull).
    pub async fn load_cursor(&self) -> Result<Option<Cursor>, SyncError> {
        match self.storage.get(&self.cursor_key()).await? {
            Some(json) => decode(&json).map(Some),
            None => Ok(None),
        }
    }

    /// Persist the cursor. The engine calls this **last** in a pull — after
    /// every record in every page is durable — so a crash can only ever
    /// leave the cursor *behind* the data, never ahead of it.
    pub async fn save_cursor(&self, cursor: &Cursor) -> Result<(), SyncError> {
        let json = encode(cursor)?;
        self.storage.set(&self.cursor_key(), &json).await?;
        Ok(())
    }

    /// Drop everything for this partition (used by "stop syncing / forget
    /// this project").
    pub async fn clear(&self) -> Result<(), SyncError> {
        self.storage.remove(&self.cache_key()).await?;
        self.storage.remove(&self.outbox_key()).await?;
        self.storage.remove(&self.cursor_key()).await?;
        Ok(())
    }
}

fn encode<V: Serialize + ?Sized>(value: &V) -> Result<String, SyncError> {
    serde_json::to_string(value).map_err(|e| SyncError::Codec(format!("encode: {e}")))
}

fn decode<V: DeserializeOwned>(json: &str) -> Result<V, SyncError> {
    serde_json::from_str(json).map_err(|e| SyncError::Codec(format!("decode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Id, Rev};
    use crate::protocol::OpKind;
    use serde::Deserialize;
    use storage::MemoryStorage;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Doc {
        title: String,
    }

    fn store() -> PartitionStore {
        PartitionStore::new(Arc::new(MemoryStorage::new()), "project:1")
    }

    #[tokio::test]
    async fn cache_round_trips_and_empty_reads_empty() {
        let s = store();
        assert!(s.load_cache::<Doc>().await.unwrap().is_empty());

        let records = vec![Record::synced(
            Id::from("a"),
            Doc { title: "x".into() },
            Rev(1),
        )];
        s.save_cache(&records).await.unwrap();
        assert_eq!(s.load_cache::<Doc>().await.unwrap(), records);
    }

    #[tokio::test]
    async fn outbox_round_trips() {
        let s = store();
        assert!(s.load_outbox().await.unwrap().is_empty());

        let ops = vec![OutboxOp::new(
            1,
            "k1".into(),
            Id::from("a"),
            OpKind::Create,
            None,
            Some("\"v\"".into()),
        )];
        s.save_outbox(&ops).await.unwrap();
        assert_eq!(s.load_outbox().await.unwrap(), ops);
    }

    #[tokio::test]
    async fn cursor_round_trips_and_absent_is_none() {
        let s = store();
        assert_eq!(s.load_cursor().await.unwrap(), None);
        s.save_cursor(&Cursor("rev:5".into())).await.unwrap();
        assert_eq!(s.load_cursor().await.unwrap(), Some(Cursor("rev:5".into())));
    }

    #[tokio::test]
    async fn clear_removes_all_three_keys() {
        let s = store();
        s.save_cache(&[Record::synced(Id::from("a"), Doc { title: "x".into() }, Rev(1))])
            .await
            .unwrap();
        s.save_outbox(&[OutboxOp::new(1, "k".into(), Id::from("a"), OpKind::Create, None, None)])
            .await
            .unwrap();
        s.save_cursor(&Cursor("c".into())).await.unwrap();

        s.clear().await.unwrap();

        assert!(s.load_cache::<Doc>().await.unwrap().is_empty());
        assert!(s.load_outbox().await.unwrap().is_empty());
        assert_eq!(s.load_cursor().await.unwrap(), None);
    }

    #[tokio::test]
    async fn partitions_are_isolated() {
        let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let a = PartitionStore::new(storage.clone(), "project:1");
        let b = PartitionStore::new(storage.clone(), "project:2");
        a.save_cursor(&Cursor("a".into())).await.unwrap();
        assert_eq!(b.load_cursor().await.unwrap(), None);
        assert_eq!(a.load_cursor().await.unwrap(), Some(Cursor("a".into())));
    }
}

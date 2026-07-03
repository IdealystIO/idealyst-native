//! Typed, crash-safe persistence over the [`SyncStore`] port.
//!
//! This layer turns the engine's typed state (`Vec<Record<T>>`, the
//! outbox, the cursor) into the serialized blobs the [`SyncStore`] trait
//! persists, and back. The *backend* is pluggable (key/value, SQLite,
//! files — see [`crate::sync_store`]); this layer only owns the
//! serialization and the per-partition keying.
//!
//! Crash-safety lives in the engine's *ordering* of these calls, not here:
//! it saves the outbox before the records on a local edit (outbox is the
//! commit point), the records before the outbox pop on an ack, and the
//! cursor **last** on a pull. Each `save_*` is one durable write the
//! backend makes atomic; this layer never assumes cross-call atomicity.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::SyncError;
use crate::model::{Cursor, Record};
use crate::outbox::OutboxOp;
use crate::sync_store::SyncStore;

/// Per-partition typed view over a shared [`SyncStore`]. Cheap to construct.
pub(crate) struct PartitionStore {
    store: Arc<dyn SyncStore>,
    partition: String,
}

impl PartitionStore {
    /// Bind a typed store to one partition over the engine's shared backend.
    pub fn new(store: Arc<dyn SyncStore>, partition: impl Into<String>) -> Self {
        PartitionStore {
            store,
            partition: partition.into(),
        }
    }

    /// Load the cached record set. A never-written partition reads as an
    /// empty vec, never an error.
    pub async fn load_cache<T: DeserializeOwned>(&self) -> Result<Vec<Record<T>>, SyncError> {
        match self.store.load_records(&self.partition).await? {
            Some(json) => decode(&json),
            None => Ok(Vec::new()),
        }
    }

    /// Persist the whole cached record set.
    pub async fn save_cache<T: Serialize>(&self, records: &[Record<T>]) -> Result<(), SyncError> {
        let json = encode(records)?;
        self.store.save_records(&self.partition, &json).await
    }

    /// Load the pending-mutation queue. Empty for a fresh partition.
    pub async fn load_outbox(&self) -> Result<Vec<OutboxOp>, SyncError> {
        match self.store.load_outbox(&self.partition).await? {
            Some(json) => decode(&json),
            None => Ok(Vec::new()),
        }
    }

    /// Persist the whole outbox queue.
    pub async fn save_outbox(&self, ops: &[OutboxOp]) -> Result<(), SyncError> {
        let json = encode(ops)?;
        self.store.save_outbox(&self.partition, &json).await
    }

    /// Load the last persisted cursor, or `None` if the partition was
    /// never pulled (which forces a snapshot on the first pull).
    pub async fn load_cursor(&self) -> Result<Option<Cursor>, SyncError> {
        match self.store.load_cursor(&self.partition).await? {
            Some(json) => decode(&json).map(Some),
            None => Ok(None),
        }
    }

    /// Persist the cursor. The engine calls this **last** in a pull — after
    /// every record in every page is durable — so a crash can only ever
    /// leave the cursor *behind* the data, never ahead of it.
    pub async fn save_cursor(&self, cursor: &Cursor) -> Result<(), SyncError> {
        let json = encode(cursor)?;
        self.store.save_cursor(&self.partition, &json).await
    }

    /// Drop everything for this partition (used by "stop syncing / forget
    /// this project").
    pub async fn clear(&self) -> Result<(), SyncError> {
        self.store.clear(&self.partition).await
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
    use crate::sync_store::MemorySyncStore;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Doc {
        title: String,
    }

    fn store() -> PartitionStore {
        PartitionStore::new(Arc::new(MemorySyncStore::new()), "project:1")
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
        let backend: Arc<dyn SyncStore> = Arc::new(MemorySyncStore::new());
        let a = PartitionStore::new(backend.clone(), "project:1");
        let b = PartitionStore::new(backend.clone(), "project:2");
        a.save_cursor(&Cursor("a".into())).await.unwrap();
        assert_eq!(b.load_cursor().await.unwrap(), None);
        assert_eq!(a.load_cursor().await.unwrap(), Some(Cursor("a".into())));
    }
}

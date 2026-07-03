//! Crash-recovery integration tests.
//!
//! Each test drops a single targeted `storage` write mid-operation to
//! simulate a crash between two key writes, then "restarts" (a fresh
//! engine over the same surviving storage) and asserts the crash-safety
//! invariants hold: no lost mutation, no skipped cursor, no double-apply.
//!
//! Requires the `reference-server` feature for the in-process authority:
//!   cargo test -p sync --features reference-server --test crash_recovery
#![cfg(feature = "reference-server")]

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use storage::{MemoryStorage, Storage, StorageError, StorageFuture};
use sync::reference::Authority;
use sync::{
    Merge, MergeCtx, PullRequest, PullResponse, PushRequest, PushResponse, Resolution, SyncEngine,
    SyncError, Transport, TransportFuture,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Item {
    text: String,
}

impl Merge for Item {
    fn merge(ctx: MergeCtx<'_, Self>) -> Resolution<Self> {
        match (ctx.local, ctx.incoming) {
            (Some(_), None) => Resolution::TakeLocal,
            _ => Resolution::TakeIncoming,
        }
    }
}

fn item(t: &str) -> Item {
    Item { text: t.into() }
}

// --- a storage wrapper that drops one targeted write to mimic a crash ---

struct FaultStorage {
    inner: Arc<dyn Storage>,
    /// `(key_substring, nth)`: fail the `nth` (0-indexed) set whose key
    /// contains the substring, by returning an error without persisting.
    spec: Mutex<Option<(String, usize)>>,
    hits: Mutex<usize>,
}

impl FaultStorage {
    fn new(inner: Arc<dyn Storage>, key_substr: &str, nth: usize) -> Self {
        FaultStorage {
            inner,
            spec: Mutex::new(Some((key_substr.to_string(), nth))),
            hits: Mutex::new(0),
        }
    }
}

impl Storage for FaultStorage {
    fn get(&self, key: &str) -> StorageFuture<'_, Option<String>> {
        self.inner.get(key)
    }

    fn set(&self, key: &str, value: &str) -> StorageFuture<'_, ()> {
        let mut fail = false;
        {
            let spec = self.spec.lock().unwrap();
            if let Some((sub, nth)) = spec.as_ref() {
                if key.contains(sub) {
                    let mut h = self.hits.lock().unwrap();
                    if *h == *nth {
                        fail = true;
                    }
                    *h += 1;
                }
            }
        }
        if fail {
            return Box::pin(async { Err(StorageError::Backend("simulated crash".into())) });
        }
        self.inner.set(key, value)
    }

    fn remove(&self, key: &str) -> StorageFuture<'_, ()> {
        self.inner.remove(key)
    }

    fn clear(&self) -> StorageFuture<'_, ()> {
        self.inner.clear()
    }
}

// --- transport over an in-process reference Authority ---

#[derive(Clone)]
struct RefTransport {
    authority: Rc<RefCell<Authority<Item>>>,
}

impl Transport<Item> for RefTransport {
    fn pull(&self, req: PullRequest) -> TransportFuture<'_, PullResponse<Item>> {
        let authority = self.authority.clone();
        Box::pin(async move {
            Ok(authority.borrow().pull(req.cursor.as_ref(), req.limit))
        })
    }

    fn push(&self, req: PushRequest<Item>) -> TransportFuture<'_, PushResponse<Item>> {
        let authority = self.authority.clone();
        Box::pin(async move { Ok(authority.borrow_mut().push(req.ops)) })
    }
}

type BoxFut<T> = Pin<Box<dyn Future<Output = T>>>;

fn transport(authority: &Rc<RefCell<Authority<Item>>>) -> Rc<RefTransport> {
    Rc::new(RefTransport {
        authority: authority.clone(),
    })
}

/// INV-3: the cursor advances LAST. If the cursor write is lost after the
/// records landed, a restart re-pulls from the old (absent) cursor and
/// re-applies idempotently — no corruption, no missing records.
#[tokio::test]
async fn cursor_write_lost_after_records_recovers_by_reapply() {
    let backing: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let authority = Rc::new(RefCell::new(Authority::<Item>::new()));
    authority.borrow_mut().seed("a", item("alpha"));
    authority.borrow_mut().seed("b", item("beta"));

    // Session 1: download, but the cursor write is dropped.
    {
        let faulty: Arc<dyn Storage> = Arc::new(FaultStorage::new(backing.clone(), "/cursor", 0));
        let eng = SyncEngine::with_kv(faulty, "dev");
        let p = eng.partition::<Item>("p", transport(&authority)).await.unwrap();
        let err = p.sync().await.unwrap_err();
        assert!(matches!(err, SyncError::Storage(_)), "cursor write failed");
    }

    // Session 2: fresh engine over the surviving storage. The cursor never
    // persisted, so it re-downloads and re-applies the same records.
    let eng2 = SyncEngine::with_kv(backing.clone(), "dev");
    let p2 = eng2.partition::<Item>("p", transport(&authority)).await.unwrap();
    p2.sync().await.unwrap();

    let mut live = p2.snapshot();
    live.sort_by(|x, y| x.text.cmp(&y.text));
    assert_eq!(live, vec![item("alpha"), item("beta")], "no loss, no dup");
}

/// INV-1: the outbox is the durable record of a mutation. If the cache
/// snapshot write is lost after the outbox committed, the mutation is NOT
/// lost — it replays from the outbox and reaches the server.
#[tokio::test]
async fn cache_write_lost_after_outbox_still_reaches_server() {
    let backing: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let authority = Rc::new(RefCell::new(Authority::<Item>::new()));

    // Session 1: offline create whose cache write is dropped (outbox lands
    // first as the commit point).
    {
        let faulty: Arc<dyn Storage> = Arc::new(FaultStorage::new(backing.clone(), "/cache", 0));
        let eng = SyncEngine::with_kv(faulty, "dev");
        eng.set_online(false);
        let p = eng.partition::<Item>("p", transport(&authority)).await.unwrap();
        let err = p.upsert("x", item("queued")).await.unwrap_err();
        assert!(matches!(err, SyncError::Storage(_)));
    }

    // Session 2: the queued op survived in the outbox even though the cache
    // snapshot didn't. Flushing replays it to the server (no data loss),
    // and the local view heals from the ack.
    let eng2 = SyncEngine::with_kv(backing.clone(), "dev");
    let p2 = eng2.partition::<Item>("p", transport(&authority)).await.unwrap();
    assert!(p2.has_pending(), "mutation survived in the outbox");
    p2.flush().await.unwrap();
    assert_eq!(authority.borrow().live_count(), 1, "reached the server");
    assert_eq!(p2.snapshot(), vec![item("queued")], "local view healed");
}

/// INV-2: record-state is durable before the acked op is dropped. If the
/// post-ack outbox write (the op removal) is lost, a restart replays the
/// op — and the server's idempotency dedup makes that a no-op, not a
/// double-apply.
#[tokio::test]
async fn outbox_pop_lost_after_ack_does_not_double_apply() {
    let backing: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let authority = Rc::new(RefCell::new(Authority::<Item>::new()));

    // Session 1: create + flush, but drop the SECOND outbox write (the
    // post-ack op removal). The first outbox write (the seal) succeeds.
    {
        // The /outbox writes in this session, in order: upsert (0), flush
        // seal (1), flush post-ack pop (2). Drop the pop.
        let faulty: Arc<dyn Storage> = Arc::new(FaultStorage::new(backing.clone(), "/outbox", 2));
        let eng = SyncEngine::with_kv(faulty, "dev");
        let p = eng.partition::<Item>("p", transport(&authority)).await.unwrap();
        p.upsert("x", item("v")).await.unwrap();
        // Flush: seal-write (ok) → push (server applies) → cache (ok) →
        // outbox-pop (dropped).
        let err = p.flush().await.unwrap_err();
        assert!(matches!(err, SyncError::Storage(_)));
    }
    assert_eq!(authority.borrow().live_count(), 1, "server applied once");

    // Session 2: the un-popped op replays; the server dedups it.
    let eng2 = SyncEngine::with_kv(backing.clone(), "dev");
    let p2 = eng2.partition::<Item>("p", transport(&authority)).await.unwrap();
    assert!(p2.has_pending(), "op still queued (pop was lost)");
    p2.flush().await.unwrap();
    assert_eq!(authority.borrow().live_count(), 1, "no double-apply");
    assert!(!p2.has_pending(), "op finally drained");
}

/// Reproduces the demo's exact sequence: initial download (sets a cursor),
/// go offline, CREATE a new task, then reconnect and sync. The offline
/// create must reach the server. (Guards against an offline-create being
/// dropped by the post-download delta pull.)
#[tokio::test]
async fn offline_create_after_download_reaches_server_on_reconnect() {
    let backing: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let authority = Rc::new(RefCell::new(Authority::<Item>::new()));
    authority.borrow_mut().seed("seed-a", item("seeded"));

    let eng = SyncEngine::with_kv(backing.clone(), "dev");
    let p = eng.partition::<Item>("p", transport(&authority)).await.unwrap();
    p.sync().await.unwrap(); // initial download → cursor persisted
    assert_eq!(p.snapshot(), vec![item("seeded")]);

    // Offline: create a brand-new task locally.
    eng.set_online(false);
    p.upsert("local-1", item("made offline")).await.unwrap();
    let _ = p.flush().await; // no-op while offline
    assert!(p.has_pending(), "offline create is queued");
    assert_eq!(authority.borrow().live_count(), 1, "not on server yet");

    // Reconnect + sync.
    eng.set_online(true);
    p.sync_now().await.unwrap();

    assert_eq!(authority.borrow().live_count(), 2, "offline create reached the server");
    assert!(!p.has_pending(), "outbox drained");
}

/// End-to-end: download → edit offline → server changes underneath →
/// reconnect reconciles via the merge policy, no silent loss.
#[tokio::test]
async fn full_offline_edit_then_reconcile_cycle() {
    let backing: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    let authority = Rc::new(RefCell::new(Authority::<Item>::new()));
    authority.borrow_mut().seed("doc", item("original"));

    let eng = SyncEngine::with_kv(backing.clone(), "dev");
    let p = eng.partition::<Item>("p", transport(&authority)).await.unwrap();
    p.sync().await.unwrap();
    assert_eq!(p.snapshot(), vec![item("original")]);

    // Go offline, edit locally.
    eng.set_online(false);
    p.upsert("doc", item("my edit")).await.unwrap();

    // Meanwhile the server deletes the doc.
    {
        let cursor = authority.borrow().pull(None, None).next_cursor;
        let _ = cursor;
        // delete via a push from another client
        let ops = vec![sync::Op {
            idem_key: "other:doc:1".into(),
            id: sync::Id::from("doc"),
            kind: sync::OpKind::Delete,
            base_rev: Some(sync::Rev(1)),
            value: None,
        }];
        authority.borrow_mut().push(ops);
    }

    // Reconnect: pull sees the server's delete under our edit; our merge
    // policy keeps the local edit (TakeLocal on delete/update). The first
    // flush hits the server's tombstone → Gone → resolves to "resurrect as
    // a create", which is fresh queued work; a second flush sends it.
    eng.set_online(true);
    p.sync_now().await.unwrap();
    assert_eq!(p.snapshot(), vec![item("my edit")], "local edit preserved");
    p.flush().await.unwrap(); // drain the resurrect-as-create

    assert_eq!(authority.borrow().live_count(), 1, "re-created on the server");
    assert!(!p.has_pending(), "fully drained");
}

// Silence an unused-type-alias warning when the file grows.
#[allow(dead_code)]
type _Unused = BoxFut<()>;

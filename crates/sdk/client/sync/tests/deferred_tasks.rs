//! The SDK under a real executor's timing: tasks run LATER, outside the
//! reactive world.
//!
//! Every other test in this crate runs without an installed executor, so
//! `spawn_async` falls back to `pollster` and runs each task inline, still
//! inside whatever `World::enter` the caller is in. That hides the one
//! thing that differs on device: web runs a spawned task on a microtask,
//! Apple on the run loop, Android on the looper, and none of them enter the
//! world. Anything a task *creates* (a signal, an effect) panics there with
//! "signal()/effect() called outside World::enter".
//!
//! This binary installs an executor that only queues. A test creates its
//! handles inside `world.enter` (standing in for a build), then drains the
//! queue with no world entered, which is how the tasks run in an app.
//!
//! Regression: `todo-sync-demo` panicked at startup on every platform.
//! `SharedPartition::open` and `SyncEngine::partition` were `async fn`s
//! that created their signals when the task first resumed, and a tab
//! becoming leader created the owner partition's signals from the Web Lock
//! callback.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use runtime_core::driver::{install_async_executor, AsyncExecutor};
use runtime_core::{__World as World, world_is_entered};
use serde::{Deserialize, Serialize};
use storage::{MemoryStorage, Storage, StorageFuture};
use sync::{
    Cursor, Merge, MergeCtx, OpResult, PullMode, PullRequest, PullResponse, PushRequest,
    PushResponse, Resolution, Rev, SharedPartition, SyncEngine, Transport, TransportFuture,
};

// --- an executor that only queues -----------------------------------------

type Task = Pin<Box<dyn Future<Output = ()>>>;

thread_local! {
    /// Each `#[test]` runs on its own thread, so each gets its own queue.
    static TASKS: RefCell<Vec<Option<Task>>> = const { RefCell::new(Vec::new()) };
}

struct QueueExecutor;

impl AsyncExecutor for QueueExecutor {
    fn spawn(&self, future: Task) {
        TASKS.with(|t| t.borrow_mut().push(Some(future)));
    }
}

/// First install wins process-wide; every test calls it, which is harmless.
fn install() {
    install_async_executor(Box::new(QueueExecutor));
}

/// Poll queued tasks, the way a platform executor would: outside the
/// world. Re-polls until a pass completes no task and spawns nothing new.
/// (A no-op waker is enough because every pending task is simply polled
/// again on the next pass.)
fn run_tasks() {
    assert!(
        !world_is_entered(),
        "tasks must run outside the world, as on device"
    );
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        let before = TASKS.with(|t| t.borrow().len());
        let mut completed = false;
        for i in 0..before {
            let Some(mut task) = TASKS.with(|t| t.borrow_mut()[i].take()) else {
                continue;
            };
            match task.as_mut().poll(&mut cx) {
                Poll::Ready(()) => completed = true,
                Poll::Pending => TASKS.with(|t| t.borrow_mut()[i] = Some(task)),
            }
        }
        let spawned = TASKS.with(|t| t.borrow().len()) > before;
        if !completed && !spawned {
            break;
        }
    }
}

/// Queue `fut` as a task (so it runs in `run_tasks`, outside the world).
fn spawn(fut: impl Future<Output = ()> + 'static) {
    runtime_core::driver::spawn_async(fut);
}

// --- entity + a minimal accepting server ------------------------------------

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

/// A server that has nothing new to send and accepts every push.
struct AcceptAll;

impl Transport<Item> for AcceptAll {
    fn pull(&self, _req: PullRequest) -> TransportFuture<'_, PullResponse<Item>> {
        Box::pin(async {
            Ok(PullResponse {
                mode: PullMode::Delta,
                changes: Vec::new(),
                next_cursor: Cursor("0".into()),
                has_more: false,
            })
        })
    }

    fn push(&self, req: PushRequest<Item>) -> TransportFuture<'_, PushResponse<Item>> {
        Box::pin(async move {
            let results = req
                .ops
                .into_iter()
                .enumerate()
                .map(|(n, op)| OpResult::Applied {
                    id: op.id,
                    new_rev: Rev(n as u64 + 1),
                })
                .collect();
            Ok(PushResponse { results })
        })
    }
}

/// Storage whose reads stay pending until `release`d, so a test can hold a
/// partition mid-load.
struct HeldStorage {
    inner: Arc<dyn Storage>,
    released: Arc<AtomicBool>,
}

impl Storage for HeldStorage {
    fn get(&self, key: &str) -> StorageFuture<'_, Option<String>> {
        let released = self.released.clone();
        let read = self.inner.get(key);
        Box::pin(async move {
            std::future::poll_fn(|_| {
                if released.load(Ordering::SeqCst) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            read.await
        })
    }
    fn set(&self, key: &str, value: &str) -> StorageFuture<'_, ()> {
        self.inner.set(key, value)
    }
    fn remove(&self, key: &str) -> StorageFuture<'_, ()> {
        self.inner.remove(key)
    }
    fn clear(&self) -> StorageFuture<'_, ()> {
        self.inner.clear()
    }
}

// --- tests -----------------------------------------------------------------

/// The demo's exact shape: open in the build, everything else later.
#[test]
fn regression_shared_partition_opened_in_a_build_becomes_owner_outside_the_world() {
    install();
    let world = World::new();
    let engine = SyncEngine::with_kv(Arc::new(MemoryStorage::new()), "device-1");

    let shared =
        world.enter(|| SharedPartition::<Item>::open(engine.clone(), "todos", Rc::new(AcceptAll)));
    assert!(
        !shared.leader_signal().get(),
        "nothing has run yet: the tasks are queued"
    );

    // Becoming owner builds the owner partition, loads it, and syncs, all
    // from a task. Before the fix this panicked creating signals.
    run_tasks();
    world.flush();
    assert!(shared.leader_signal().get(), "native is always the owner");
    assert!(
        shared.loaded().get(),
        "owner load reached the shared signals"
    );

    let s = shared.clone();
    spawn(async move { s.upsert("a", item("written after open")).await.unwrap() });
    run_tasks();
    world.flush();
    assert_eq!(shared.items().get(), vec![item("written after open")]);
}

/// A partition created in a build loads in a task, and the load fills the
/// signals the build created.
#[test]
fn regression_partition_created_in_a_build_loads_outside_the_world() {
    install();
    let world = World::new();
    let storage: Arc<dyn Storage> = Arc::new(MemoryStorage::new());

    // Session 1: queue an edit offline, so it only lives in storage.
    {
        let engine = SyncEngine::with_kv(storage.clone(), "device-1");
        engine.set_online(false);
        let p = world.enter(|| engine.partition::<Item>("p", Rc::new(AcceptAll)));
        spawn(async move { p.upsert("a", item("saved")).await.unwrap() });
        run_tasks();
    }

    // Session 2: a fresh engine over the same storage.
    let engine = SyncEngine::with_kv(storage, "device-1");
    let p = world.enter(|| engine.partition::<Item>("p", Rc::new(AcceptAll)));
    assert!(!p.loaded().get(), "load is queued, not run inline");
    assert!(p.items().get().is_empty());

    run_tasks();
    world.flush();
    assert!(p.loaded().get());
    assert_eq!(
        p.items().get(),
        vec![item("saved")],
        "persisted edit restored into the signal"
    );
    assert!(p.has_pending(), "and still queued for the server");
}

/// An edit made while the load is still in flight waits for it, then
/// lands on top of the loaded records instead of being overwritten by
/// them (or overwriting them).
#[test]
fn edit_before_load_finishes_lands_on_top_of_loaded_state() {
    install();
    let world = World::new();
    let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());

    // Seed one record through a first engine.
    {
        let engine = SyncEngine::with_kv(mem.clone(), "device-1");
        engine.set_online(false);
        let p = world.enter(|| engine.partition::<Item>("p", Rc::new(AcceptAll)));
        spawn(async move { p.upsert("old", item("from storage")).await.unwrap() });
        run_tasks();
    }

    let released = Arc::new(AtomicBool::new(false));
    let held: Arc<dyn Storage> = Arc::new(HeldStorage {
        inner: mem,
        released: released.clone(),
    });
    let engine = SyncEngine::with_kv(held, "device-1");
    engine.set_online(false);
    let p = world.enter(|| engine.partition::<Item>("p", Rc::new(AcceptAll)));

    let writer = p.clone();
    spawn(async move {
        writer
            .upsert("new", item("typed during load"))
            .await
            .unwrap()
    });
    run_tasks();
    assert!(!p.loaded().get(), "storage read still held");
    assert!(p.snapshot().is_empty(), "the edit waited for the load");

    released.store(true, Ordering::SeqCst);
    run_tasks();
    world.flush();
    let mut texts: Vec<String> = p.items().get().into_iter().map(|i| i.text).collect();
    texts.sort();
    assert_eq!(texts, vec!["from storage", "typed during load"]);
}

/// A failed load is reported by `ready()` and fails later operations with
/// the same error, rather than letting them run against empty state and
/// overwrite what is on disk.
#[test]
fn failed_load_fails_every_operation() {
    install();
    let world = World::new();
    let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
    // A cache entry that cannot be decoded as this partition's records
    // (key layout: `KvSyncStore::key`).
    let seed = mem.clone();
    spawn(async move {
        seed.set("sync/p/cache", "not json").await.unwrap();
    });
    run_tasks();

    let engine = SyncEngine::with_kv(mem, "device-1");
    let p = world.enter(|| engine.partition::<Item>("p", Rc::new(AcceptAll)));
    let outcome: Rc<RefCell<Option<(bool, bool)>>> = Rc::new(RefCell::new(None));
    let (o, q) = (outcome.clone(), p.clone());
    spawn(async move {
        let ready_failed = q.ready().await.is_err();
        let upsert_failed = q.upsert("x", item("x")).await.is_err();
        *o.borrow_mut() = Some((ready_failed, upsert_failed));
    });
    run_tasks();
    world.flush();

    assert_eq!(*outcome.borrow(), Some((true, true)));
    assert!(p.load_error().is_some());
    assert!(!p.loaded().get());
}

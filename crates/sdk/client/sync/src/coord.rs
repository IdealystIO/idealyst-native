//! Web multi-tab coordination via **leader election**.
//!
//! A [`SyncEngine`] owns its storage namespace + client id, so two tabs
//! must not each run one over the same browser storage (they'd clobber each
//! other — see the SDK README). [`SharedPartition`] solves this: exactly
//! one tab is the **leader** (it holds a `navigator.locks` Web Lock), owns
//! the real [`Partition`] (engine, storage, server sync), and broadcasts
//! its state over a `BroadcastChannel`. Every other tab is a **follower**:
//! it mirrors the leader's state into a local signal and proxies its
//! mutations to the leader. When the leader's tab closes the lock releases
//! and a follower is promoted automatically — the storage is already
//! durable, so nothing is lost.
//!
//! The API mirrors [`Partition`] (`entries()`, `items()`, `loaded()`,
//! `ready()`, `upsert`, `delete`, `sync_now`, `set_online`), so app code is
//! identical whether a tab is leader or follower. On **native** there's
//! only ever one instance, so `SharedPartition` is just an owner with no
//! coordination.
//!
//! [`SharedPartition::open`] is synchronous and creates every signal the
//! handle will ever expose, for the reason in the `partition` module docs:
//! a tab becomes leader from the Web Lock callback, and nothing reactive
//! can be *created* there. So the leader's owner [`Partition`] publishes
//! into the signals `open` made, and the broadcast to followers hangs off a
//! plain publish listener instead of an effect.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{signal, unscope, ReadSignal, Signal};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::SyncError;
use crate::merge::Merge;
use crate::model::{Entry, Id};
use crate::partition::{LoadGate, Partition, PartitionSignals};
use crate::protocol::Transport;
use crate::SyncEngine;

/// Messages exchanged between tabs over the `BroadcastChannel`. Payloads are
/// pre-serialized JSON strings so the wire stays free of the entity type.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum CoordMsg {
    /// Leader → all: "I'm the owner" (followers re-request state).
    OwnerHello,
    /// Follower → leader: please broadcast current state (I just joined).
    RequestState,
    /// Leader → all: the current entries (JSON of `Vec<Entry<T>>`).
    State { entries: String },
    /// Follower → leader: create/update a record (`value` is JSON of `T`).
    Upsert { id: String, value: String },
    /// Follower → leader: delete a record.
    Delete { id: String },
    /// Follower → leader: pull + flush now.
    SyncNow,
    /// Follower → leader: set connectivity.
    SetOnline { online: bool },
}

/// Shared state behind a [`SharedPartition`], captured by both the public
/// handle and the (web) message/lock callbacks.
struct SharedInner<T> {
    /// Stable signals the UI binds to — written by the owner partition
    /// (leader) or by incoming `State` messages (follower).
    signals: PartitionSignals<T>,
    /// Reactive leadership flag — flips to `true` when this tab becomes the
    /// leader (initially or via promotion), so the UI can show its role.
    leader_sig: Signal<bool>,
    /// Finished by the first state this tab gets: its own load as leader,
    /// or the leader's broadcast as follower.
    gate: Rc<LoadGate>,
    /// `Some` once this tab is the leader.
    owner: RefCell<Option<Partition<T>>>,
    engine: SyncEngine,
    name: String,
    transport: Rc<dyn Transport<T>>,
    /// The cross-tab bus (web only); `None` on native.
    #[cfg(target_arch = "wasm32")]
    bus: RefCell<Option<web::TabBus>>,
}

impl<T: Clone + PartialEq + Serialize + DeserializeOwned + Merge + 'static> SharedInner<T> {
    /// Create the shared state, signals included. Runs inside `open`, which
    /// is called during a build, so the world is entered here and nowhere
    /// later.
    fn new(engine: SyncEngine, name: &str, transport: Rc<dyn Transport<T>>) -> Self {
        SharedInner {
            signals: PartitionSignals::new(),
            leader_sig: unscope(|| signal(false)),
            gate: LoadGate::new(),
            owner: RefCell::new(None),
            engine,
            name: name.to_string(),
            transport,
            #[cfg(target_arch = "wasm32")]
            bus: RefCell::new(None),
        }
    }

    fn set_state(&self, entries: Vec<Entry<T>>) {
        let items = entries
            .iter()
            .map(|e| e.value.clone())
            .collect::<Vec<_>>();
        // `set_always`: author item type `T` carries no `PartialEq`
        // bound (only `Merge`), so the projected lists can't be
        // equality-guarded. No explicit batch window is needed — a `set`
        // STAGES the write and the flush commits every staged write in one
        // pass, so these two coalesce into a single fan-out by
        // construction.
        self.signals.items.set_always(items);
        self.signals.entries.set_always(entries);
        self.mark_loaded(Ok(()));
    }

    fn mark_loaded(&self, result: Result<(), SyncError>) {
        if result.is_ok() {
            self.signals.loaded.set(true);
        }
        self.gate.finish_once(result);
    }

    fn is_owner(&self) -> bool {
        self.owner.borrow().is_some()
    }

    #[cfg(target_arch = "wasm32")]
    fn post(&self, msg: &CoordMsg) {
        if let Some(bus) = self.bus.borrow().as_ref() {
            if let Ok(json) = serde_json::to_string(msg) {
                bus.post(&json);
            }
        }
    }
}

/// A multi-tab-safe partition handle. See the module docs.
pub struct SharedPartition<T> {
    inner: Rc<SharedInner<T>>,
}

impl<T> Clone for SharedPartition<T> {
    fn clone(&self) -> Self {
        SharedPartition {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Clone + PartialEq + Serialize + DeserializeOwned + Merge + 'static> SharedPartition<T> {
    /// The reactive entries view (status-aware), identical to
    /// [`Partition::entries`]. Stable across a leader handoff.
    pub fn entries(&self) -> Signal<Vec<Entry<T>>> {
        self.inner.signals.entries
    }

    /// The reactive values view, identical to [`Partition::items`].
    pub fn items(&self) -> Signal<Vec<T>> {
        self.inner.signals.items
    }

    /// `true` once this tab has state to show: its own load as leader, or
    /// the leader's first broadcast as follower. Until then
    /// [`items`](Self::items) and [`entries`](Self::entries) are empty.
    pub fn loaded(&self) -> ReadSignal<bool> {
        self.inner.signals.loaded.read_only()
    }

    /// Wait until [`loaded`](Self::loaded), and get the error if this tab
    /// became leader and failed to load its partition from storage.
    pub async fn ready(&self) -> Result<(), SyncError> {
        self.inner.gate.clone().wait().await
    }

    /// Create or update a record. Leader: applies + flushes directly.
    /// Follower: proxies to the leader (the UI updates when the leader
    /// broadcasts the new state).
    pub async fn upsert(&self, id: impl Into<Id>, value: T) -> Result<(), SyncError> {
        let id = id.into();
        let owner = self.inner.owner.borrow().clone();
        match owner {
            Some(p) => {
                p.upsert(id, value).await?;
                p.flush().await
            }
            None => {
                self.proxy_upsert(&id, &value);
                Ok(())
            }
        }
    }

    /// Delete a record (leader applies + flushes; follower proxies).
    pub async fn delete(&self, id: impl Into<Id>) -> Result<(), SyncError> {
        let id = id.into();
        let owner = self.inner.owner.borrow().clone();
        match owner {
            Some(p) => {
                p.delete(id).await?;
                p.flush().await
            }
            None => {
                self.proxy_delete(&id);
                Ok(())
            }
        }
    }

    /// Pull + flush (leader runs it; follower asks the leader to).
    pub async fn sync_now(&self) -> Result<(), SyncError> {
        let owner = self.inner.owner.borrow().clone();
        match owner {
            Some(p) => p.sync_now().await,
            None => {
                self.proxy_sync_now();
                Ok(())
            }
        }
    }

    /// Set connectivity for the owning engine (leader applies; follower
    /// forwards to the leader).
    pub fn set_online(&self, online: bool) {
        if self.inner.is_owner() {
            self.inner.engine.set_online(online);
        } else {
            self.proxy_set_online(online);
        }
    }

    /// True if this tab is the current leader (owns the engine). Non-reactive
    /// snapshot; for UI, bind to [`leader_signal`](Self::leader_signal).
    pub fn is_leader(&self) -> bool {
        self.inner.is_owner()
    }

    /// Reactive leadership flag — `true` while this tab is the leader. Flips
    /// when leadership is acquired (initially or via promotion when the
    /// previous leader's tab closes), so the UI can show the tab's role live.
    pub fn leader_signal(&self) -> Signal<bool> {
        self.inner.leader_sig
    }
}

// ---------------------------------------------------------------------------
// Native: no coordination — always the owner.
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
impl<T: Clone + PartialEq + Serialize + DeserializeOwned + Merge + 'static> SharedPartition<T> {
    /// Open the partition. On native this is just an owner with no
    /// coordination: it starts loading right away.
    ///
    /// **Call it during a build** (a component body, or `app()`): it
    /// creates the handle's signals, which needs the reactive world
    /// entered. Watch [`loaded`](Self::loaded) or await
    /// [`ready`](Self::ready) for the load.
    pub fn open(engine: SyncEngine, name: &str, transport: Rc<dyn Transport<T>>) -> Self {
        let inner = Rc::new(SharedInner::new(engine, name, transport));
        let owner = inner.clone();
        runtime_core::driver::spawn_async(async move {
            let _ = become_owner(owner).await;
        });
        SharedPartition { inner }
    }

    // Native never proxies (always owner); these are unreachable.
    fn proxy_upsert(&self, _id: &Id, _value: &T) {}
    fn proxy_delete(&self, _id: &Id) {}
    fn proxy_sync_now(&self) {}
    fn proxy_set_online(&self, _online: bool) {}
}

// ---------------------------------------------------------------------------
// Web: leader election + follower proxying.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
impl<T: Clone + PartialEq + Serialize + DeserializeOwned + Merge + 'static> SharedPartition<T> {
    /// Open the partition with multi-tab coordination. Starts as a follower
    /// and requests leadership; the first tab to acquire the lock becomes
    /// the owner, and a follower is promoted when the owner's tab closes.
    ///
    /// **Call it during a build** (a component body, or `app()`): it
    /// creates the handle's signals, which needs the reactive world
    /// entered. Watch [`loaded`](Self::loaded) or await
    /// [`ready`](Self::ready) for the first state.
    pub fn open(engine: SyncEngine, name: &str, transport: Rc<dyn Transport<T>>) -> Self {
        let inner = Rc::new(SharedInner::new(engine, name, transport));

        // Wire the cross-tab bus: dispatch incoming messages.
        let inner_for_bus = inner.clone();
        let bus = web::TabBus::new(&format!("sync-coord:{name}"), move |json| {
            if let Ok(msg) = serde_json::from_str::<CoordMsg>(&json) {
                handle_msg(&inner_for_bus, msg);
            }
        });
        *inner.bus.borrow_mut() = Some(bus);

        // As a fresh follower, ask whoever's leader for current state.
        inner.post(&CoordMsg::RequestState);

        // Request leadership; when granted, build the owner.
        let inner_for_lock = inner.clone();
        web::request_leadership(&format!("sync-leader:{name}"), move || {
            let inner = inner_for_lock.clone();
            runtime_core::driver::spawn_async(async move {
                let _ = become_owner(inner).await;
            });
        });

        SharedPartition { inner }
    }

    fn proxy_upsert(&self, id: &Id, value: &T) {
        if let Ok(value) = serde_json::to_string(value) {
            self.inner.post(&CoordMsg::Upsert {
                id: id.0.clone(),
                value,
            });
        }
    }
    fn proxy_delete(&self, id: &Id) {
        self.inner.post(&CoordMsg::Delete { id: id.0.clone() });
    }
    fn proxy_sync_now(&self) {
        self.inner.post(&CoordMsg::SyncNow);
    }
    fn proxy_set_online(&self, online: bool) {
        self.inner.post(&CoordMsg::SetOnline { online });
    }
}

/// Build the real owner partition for this tab, publish its state into the
/// shared signals (and broadcast it on every change), then announce + run an
/// initial sync. Used by both native `open` and the web leadership callback.
///
/// Runs from an async task (native) or the Web Lock callback (web), so it
/// must not create anything reactive: the owner partition publishes into
/// the signals `open` already made, and the broadcast is a publish listener.
async fn become_owner<T: Clone + PartialEq + Serialize + DeserializeOwned + Merge + 'static>(
    inner: Rc<SharedInner<T>>,
) -> Result<(), SyncError> {
    let partition =
        inner
            .engine
            .partition_into::<T>(&inner.name, inner.transport.clone(), inner.signals);
    // `false` when the app already had a plain `Partition` of this name on
    // the same engine: it has its own signals, so mirror them.
    let publishes_here = partition.publishes_into(&inner.signals);
    *inner.owner.borrow_mut() = Some(partition.clone());
    inner.leader_sig.set(true);

    // The listener keeps `inner` alive through the partition the engine
    // holds: both live as long as the app, which is the lifetime of the
    // mirror too.
    let for_listener = inner.clone();
    partition.add_publish_listener(Rc::new(move |entries: &[Entry<T>]| {
        if !publishes_here {
            for_listener.set_state(entries.to_vec());
        }
        #[cfg(target_arch = "wasm32")]
        {
            if let Ok(json) = serde_json::to_string(entries) {
                for_listener.post(&CoordMsg::State { entries: json });
            }
        }
    }));

    let loaded = partition.ready().await;
    if loaded.is_ok() {
        // The load may have published before the listener existed (native
        // with no executor runs it inline), so push the current state once.
        let entries = partition.entries_snapshot();
        #[cfg(target_arch = "wasm32")]
        {
            if let Ok(json) = serde_json::to_string(&entries) {
                inner.post(&CoordMsg::State { entries: json });
            }
        }
        if !publishes_here {
            inner.set_state(entries);
        }
    }
    inner.mark_loaded(loaded.clone());
    loaded?;

    // Announce leadership + run an initial sync.
    #[cfg(target_arch = "wasm32")]
    inner.post(&CoordMsg::OwnerHello);
    let _ = partition.sync_now().await;
    Ok(())
}

/// Dispatch an incoming cross-tab message (web only).
#[cfg(target_arch = "wasm32")]
fn handle_msg<T: Clone + PartialEq + Serialize + DeserializeOwned + Merge + 'static>(
    inner: &Rc<SharedInner<T>>,
    msg: CoordMsg,
) {
    match msg {
        // Follower receives the leader's state → mirror it.
        CoordMsg::State { entries } => {
            if !inner.is_owner() {
                if let Ok(entries) = serde_json::from_str::<Vec<Entry<T>>>(&entries) {
                    inner.set_state(entries);
                }
            }
        }
        // A follower asked for state — if we're the leader, broadcast it.
        CoordMsg::RequestState => {
            if let Some(p) = inner.owner.borrow().clone() {
                let entries = p.entries().get();
                if let Ok(json) = serde_json::to_string(&entries) {
                    inner.post(&CoordMsg::State { entries: json });
                }
            }
        }
        // A leader announced itself — if we're a follower, (re-)request
        // state so we get the current snapshot regardless of join order.
        CoordMsg::OwnerHello => {
            if !inner.is_owner() {
                inner.post(&CoordMsg::RequestState);
            }
        }
        // Follower mutations — only the leader applies them (queue + flush).
        CoordMsg::Upsert { id, value } => {
            if let Some(p) = inner.owner.borrow().clone() {
                if let Ok(value) = serde_json::from_str::<T>(&value) {
                    runtime_core::driver::spawn_async(async move {
                        if p.upsert(Id(id), value).await.is_ok() {
                            let _ = p.flush().await;
                        }
                    });
                }
            }
        }
        CoordMsg::Delete { id } => {
            if let Some(p) = inner.owner.borrow().clone() {
                runtime_core::driver::spawn_async(async move {
                    if p.delete(Id(id)).await.is_ok() {
                        let _ = p.flush().await;
                    }
                });
            }
        }
        CoordMsg::SyncNow => {
            if let Some(p) = inner.owner.borrow().clone() {
                runtime_core::driver::spawn_async(async move {
                    let _ = p.sync_now().await;
                });
            }
        }
        CoordMsg::SetOnline { online } => {
            if inner.is_owner() {
                inner.engine.set_online(online);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Web primitives: BroadcastChannel bus + Web Locks leader election.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod web {
    //! The browser calls are web-glue bindings declared here
    //! (own-web-bindings phase 3).

    use web_glue::{string, Closure, JsValue};

    web_glue::import! {
        #[catch]
        fn js_channel_new(p: usize, l: usize) -> u32 =
            "(p, l) => G.add(new BroadcastChannel(G.str(p, l)))";
        fn js_channel_set_onmessage(c: u32, f: u32) =
            "(c, f) => { G.get(c).onmessage = G.get(f); }";
        // Detach the handler and close the channel: a closed channel
        // receives nothing more.
        fn js_channel_close(c: u32) =
            "(c) => { const ch = G.get(c); ch.onmessage = null; ch.close(); }";
        // `postMessage` throws on a closed channel; the post is best-effort.
        #[catch]
        fn js_channel_post(c: u32, p: usize, l: usize) =
            "(c, p, l) => { G.get(c).postMessage(G.str(p, l)); }";
        // A `MessageEvent`'s string `data` into `out`; 0 for non-string data.
        fn js_message_str(e: u32, out: usize) -> u32 =
            "(e, o) => { const d = G.get(e).data; if (typeof d !== 'string') return 0; \
               G.retStr(d, o); return 1; }";
        // `navigator.locks.request(name, cb)`. The lock callback is wrapped
        // in JS so it can RETURN a never-resolving Promise — that is what
        // holds the lock until the tab closes; the Rust callback itself
        // returns nothing. 0 (no-op) without a window or the Web Locks API.
        #[catch]
        fn js_request_lock(p: usize, l: usize, f: u32) -> u32 =
            "(p, l, f) => { if (typeof window === 'undefined') return 0; \
               const locks = window.navigator.locks; \
               if (locks == null || typeof locks.request !== 'function') return 0; \
               const cb = G.get(f); \
               locks.request(G.str(p, l), (lock) => { cb(lock); return new Promise(() => {}); }); \
               return 1; }";
    }

    /// A `BroadcastChannel` wrapper. The message callback is retained in the
    /// struct (not leaked) so it lives exactly as long as the bus.
    pub(super) struct TabBus {
        channel: JsValue,
        _on_message: Closure,
    }

    impl TabBus {
        pub(super) fn new(name: &str, on_msg: impl Fn(String) + 'static) -> Self {
            let (p, l) = string::abi(name);
            let channel = unsafe { js_channel_new(p, l) }
                // SAFETY: a fresh `G.add` slot the snippet minted for us.
                .map(|h| unsafe { JsValue::from_raw(h) })
                .expect("sync: BroadcastChannel unavailable");
            let cb = Closure::new(move |ev: JsValue| {
                let mut is_str = 0;
                let s = string::receive(|o| is_str = unsafe { js_message_str(ev.raw(), o) });
                if is_str != 0 {
                    on_msg(s);
                }
            });
            unsafe { js_channel_set_onmessage(channel.raw(), cb.as_js().raw()) };
            TabBus {
                channel,
                _on_message: cb,
            }
        }

        pub(super) fn post(&self, msg: &str) {
            let (p, l) = string::abi(msg);
            let _ = unsafe { js_channel_post(self.channel.raw(), p, l) };
        }
    }

    /// Detach and close before the closure drops (fields drop after this
    /// body). The web-sys version only dropped the closure: the channel
    /// stayed open with `onmessage` still pointing at it, so the next
    /// message from another tab invoked a dropped closure and threw into
    /// the page. Regression: `tests::regression_dropped_bus_leaves_no_dead_handler`.
    impl Drop for TabBus {
        fn drop(&mut self) {
            unsafe { js_channel_close(self.channel.raw()) }
        }
    }

    /// Request leadership via the Web Locks API. Exactly one tab holds the
    /// named lock; `on_acquire` fires when this tab becomes leader (the
    /// initial holder, or a promotion when the previous leader's tab
    /// closes). The lock is held until the tab closes (the callback returns
    /// a never-resolving promise). A no-op where the browser lacks the Web
    /// Locks API (no leader election).
    pub(super) fn request_leadership(name: &str, on_acquire: impl FnOnce() + 'static) {
        // The lock callback: announce acquisition. Its JS wrapper (in
        // `js_request_lock`) returns the never-resolving Promise.
        let cb = Closure::once_into_js(move |_lock: JsValue| on_acquire());
        let (p, l) = string::abi(name);
        let _ = unsafe { js_request_lock(p, l, cb.raw()) };
    }

    #[cfg(test)]
    mod tests {
        //! Browser tests — `cargo test -p sync --lib --target
        //! wasm32-unknown-unknown` (the workspace runner supplies web-glue's
        //! JS; `wasm-pack test` cannot).

        use std::cell::RefCell;
        use std::rc::Rc;

        use wasm_bindgen_test::*;
        use web_glue::{JsFuture, JsValue};

        use super::{request_leadership, TabBus};

        wasm_bindgen_test_configure!(run_in_browser);

        fn eval(body: &str) -> JsValue {
            let f = JsValue::global()
                .get("Function")
                .unwrap()
                .construct(&[&JsValue::from_str(body)])
                .unwrap();
            f.call(&JsValue::undefined(), &[]).unwrap()
        }

        /// Let BroadcastChannel deliver (it posts on a task).
        async fn settle() {
            let p = eval("return new Promise((r) => setTimeout(r, 50));");
            let _ = JsFuture::new(&p).await;
        }

        #[wasm_bindgen_test]
        async fn a_message_reaches_the_other_bus() {
            let got: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
            let sink = got.clone();
            let a = TabBus::new("sync-test-deliver", |_| {});
            let _b = TabBus::new("sync-test-deliver", move |m| sink.borrow_mut().push(m));
            a.post("{\"hello\":\"ü\"}");
            settle().await;
            assert_eq!(*got.borrow(), vec!["{\"hello\":\"ü\"}".to_string()]);
        }

        /// Regression: a dropped bus must detach and close its channel, so
        /// a message another tab posts afterwards reaches no dropped
        /// closure. A throw inside `onmessage` is reported as a window
        /// `error` event, which this records.
        #[wasm_bindgen_test]
        async fn regression_dropped_bus_leaves_no_dead_handler() {
            let a = TabBus::new("sync-test-drop", |_| {});
            let b = TabBus::new("sync-test-drop", |_| {});
            drop(b);
            eval("globalThis.__errs = []; globalThis.__spy = (e) => __errs.push(e.message); \
                  window.addEventListener('error', __spy);");
            a.post("after-drop");
            settle().await;
            eval("window.removeEventListener('error', __spy);");
            let errs = JsValue::global().get("__errs").unwrap();
            let n = errs.get("length").unwrap().as_f64();
            let joined = errs.call_method("join", &[&JsValue::from_str(" | ")]).unwrap().as_string();
            assert_eq!(n, Some(0.0), "a message after drop reached a dead handler: {joined:?}");
        }

        #[wasm_bindgen_test]
        async fn the_first_requester_acquires_the_lock() {
            let acquired = Rc::new(RefCell::new(0));
            let (a1, a2) = (acquired.clone(), acquired.clone());
            request_leadership("sync-test-lock", move || *a1.borrow_mut() += 1);
            // Same name: queued behind the held (never-released) lock.
            request_leadership("sync-test-lock", move || *a2.borrow_mut() += 1);
            // The lock manager answers over IPC; give it up to ~2 s.
            for _ in 0..40 {
                if *acquired.borrow() != 0 {
                    break;
                }
                settle().await;
            }
            settle().await;
            let n = *acquired.borrow();
            assert_eq!(n, 1, "exactly one requester holds the lock");
        }
    }
}

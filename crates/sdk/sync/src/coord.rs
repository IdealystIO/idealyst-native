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
//! The API mirrors [`Partition`] (`entries()`, `items()`, `upsert`,
//! `delete`, `sync_now`, `set_online`), so app code is identical whether a
//! tab is leader or follower. On **native** there's only ever one instance,
//! so `SharedPartition` is just an owner with no coordination.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{unscope, Signal};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::SyncError;
use crate::merge::Merge;
use crate::model::{Entry, Id};
use crate::partition::Partition;
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
    /// Stable signals the UI binds to — written by the owner-mirror effect
    /// (leader) or by incoming `State` messages (follower).
    entries_sig: Signal<Vec<Entry<T>>>,
    items_sig: Signal<Vec<T>>,
    /// Reactive leadership flag — flips to `true` when this tab becomes the
    /// leader (initially or via promotion), so the UI can show its role.
    leader_sig: Signal<bool>,
    /// `Some` once this tab is the leader.
    owner: RefCell<Option<Partition<T>>>,
    engine: SyncEngine,
    name: String,
    transport: Rc<dyn Transport<T>>,
    /// The cross-tab bus (web only); `None` on native.
    #[cfg(target_arch = "wasm32")]
    bus: RefCell<Option<web::TabBus>>,
}

impl<T: Clone + Serialize + DeserializeOwned + Merge + 'static> SharedInner<T> {
    fn set_state(&self, entries: Vec<Entry<T>>) {
        let items = entries
            .iter()
            .map(|e| e.value.clone())
            .collect::<Vec<_>>();
        runtime_core::cycle(|| {
            self.items_sig.set(items);
            self.entries_sig.set(entries);
        });
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

impl<T: Clone + Serialize + DeserializeOwned + Merge + 'static> SharedPartition<T> {
    /// The reactive entries view (status-aware), identical to
    /// [`Partition::entries`]. Stable across a leader handoff.
    pub fn entries(&self) -> Signal<Vec<Entry<T>>> {
        self.inner.entries_sig
    }

    /// The reactive values view, identical to [`Partition::items`].
    pub fn items(&self) -> Signal<Vec<T>> {
        self.inner.items_sig
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
impl<T: Clone + Serialize + DeserializeOwned + Merge + 'static> SharedPartition<T> {
    /// Open the partition. On native this is just an owner with no
    /// coordination.
    pub async fn open(
        engine: SyncEngine,
        name: &str,
        transport: Rc<dyn Transport<T>>,
    ) -> Result<Self, SyncError> {
        let (entries_sig, items_sig, leader_sig) = unscope(|| {
            (Signal::new(Vec::new()), Signal::new(Vec::new()), Signal::new(false))
        });
        let inner = Rc::new(SharedInner {
            entries_sig,
            items_sig,
            leader_sig,
            owner: RefCell::new(None),
            engine,
            name: name.to_string(),
            transport,
            #[cfg(target_arch = "wasm32")]
            bus: RefCell::new(None),
        });
        become_owner(inner.clone()).await?;
        Ok(SharedPartition { inner })
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
impl<T: Clone + Serialize + DeserializeOwned + Merge + 'static> SharedPartition<T> {
    /// Open the partition with multi-tab coordination. Starts as a follower
    /// and requests leadership; the first tab to acquire the lock becomes
    /// the owner, and a follower is promoted when the owner's tab closes.
    pub async fn open(
        engine: SyncEngine,
        name: &str,
        transport: Rc<dyn Transport<T>>,
    ) -> Result<Self, SyncError> {
        let (entries_sig, items_sig, leader_sig) = unscope(|| {
            (Signal::new(Vec::new()), Signal::new(Vec::new()), Signal::new(false))
        });
        let inner = Rc::new(SharedInner {
            entries_sig,
            items_sig,
            leader_sig,
            owner: RefCell::new(None),
            engine,
            name: name.to_string(),
            transport,
            bus: RefCell::new(None),
        });

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

        Ok(SharedPartition { inner })
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

/// Build the real owner partition for this tab, mirror its signals into the
/// shared signals (and broadcast on every change), then announce + initial
/// sync. Used by both native open and the web leadership callback.
async fn become_owner<T: Clone + Serialize + DeserializeOwned + Merge + 'static>(
    inner: Rc<SharedInner<T>>,
) -> Result<(), SyncError> {
    let partition = inner
        .engine
        .partition::<T>(&inner.name, inner.transport.clone())
        .await?;
    *inner.owner.borrow_mut() = Some(partition.clone());
    inner.leader_sig.set(true);

    // Mirror the owner partition's entries into the shared signals, and
    // (web) broadcast them to followers, on every change.
    let inner_for_effect = inner.clone();
    let pe = partition.entries();
    runtime_core::Effect::new(move || {
        let entries = pe.get();
        inner_for_effect.set_state(entries.clone());
        #[cfg(target_arch = "wasm32")]
        {
            if let Ok(json) = serde_json::to_string(&entries) {
                inner_for_effect.post(&CoordMsg::State { entries: json });
            }
        }
    })
    .persist();

    // Announce leadership + run an initial sync.
    #[cfg(target_arch = "wasm32")]
    inner.post(&CoordMsg::OwnerHello);
    let _ = partition.sync_now().await;
    Ok(())
}

/// Dispatch an incoming cross-tab message (web only).
#[cfg(target_arch = "wasm32")]
fn handle_msg<T: Clone + Serialize + DeserializeOwned + Merge + 'static>(
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
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    /// A `BroadcastChannel` wrapper. The message callback is retained in the
    /// struct (not leaked) so it lives exactly as long as the bus.
    pub(super) struct TabBus {
        channel: web_sys::BroadcastChannel,
        _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    }

    impl TabBus {
        pub(super) fn new(name: &str, on_msg: impl Fn(String) + 'static) -> Self {
            let channel = web_sys::BroadcastChannel::new(name)
                .expect("sync: BroadcastChannel unavailable");
            let cb = Closure::wrap(Box::new(move |ev: web_sys::MessageEvent| {
                if let Some(s) = ev.data().as_string() {
                    on_msg(s);
                }
            }) as Box<dyn FnMut(web_sys::MessageEvent)>);
            channel.set_onmessage(Some(cb.as_ref().unchecked_ref()));
            TabBus {
                channel,
                _on_message: cb,
            }
        }

        pub(super) fn post(&self, msg: &str) {
            let _ = self.channel.post_message(&JsValue::from_str(msg));
        }
    }

    /// Request leadership via the Web Locks API. Exactly one tab holds the
    /// named lock; `on_acquire` fires when this tab becomes leader (the
    /// initial holder, or a promotion when the previous leader's tab
    /// closes). The lock is held until the tab closes (the callback returns
    /// a never-resolving promise).
    pub(super) fn request_leadership(name: &str, on_acquire: impl FnOnce() + 'static) {
        let Some(window) = web_sys::window() else {
            return;
        };
        // Reach `navigator.locks.request(name, callback)` dynamically via
        // Reflect — `web_sys::Navigator::locks()` is behind web-sys's
        // unstable-APIs cfg, and this also degrades gracefully (no-op) if
        // the browser lacks the Web Locks API.
        let navigator = window.navigator();
        let Ok(locks) = js_sys::Reflect::get(&navigator, &JsValue::from_str("locks")) else {
            return;
        };
        if locks.is_undefined() || locks.is_null() {
            return; // Web Locks unsupported → no leader election
        }
        let Ok(request) = js_sys::Reflect::get(&locks, &JsValue::from_str("request")) else {
            return;
        };
        let Ok(request) = request.dyn_into::<js_sys::Function>() else {
            return;
        };

        // The lock callback: announce acquisition, then hold the lock open
        // by returning a Promise that never resolves (released when the tab
        // closes).
        let cb = Closure::once_into_js(move |_lock: JsValue| -> JsValue {
            on_acquire();
            js_sys::Promise::new(&mut |_resolve, _reject| {}).into()
        });

        let _ = request.call2(&locks, &JsValue::from_str(name), &cb);
    }
}

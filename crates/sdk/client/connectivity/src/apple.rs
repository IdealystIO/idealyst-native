//! iOS / macOS / tvOS reachability via **`NWPathMonitor`** (the Network
//! framework).
//!
//! ⚠️ **Compile-checked only — not device-verified.** The C bindings,
//! lifetime handling, and block bridge are written against Apple's public
//! `network/path_monitor.h` / `network/path.h` contract, but this hasn't been
//! exercised on a real iOS/macOS run loop in this change.
//!
//! ## Why the C API, not an Obj-C class
//!
//! `NWPathMonitor` is the Swift name for the C type `nw_path_monitor_t`; there
//! is no Obj-C class to `msg_send` to. The Network framework is a C API of
//! ARC-managed `nw_object`s, so we bind the `nw_path_monitor_*` /
//! `nw_path_*` C functions directly and manage retain/release explicitly.
//!
//! ## `current()` — a synchronous snapshot from an async monitor
//!
//! `NWPathMonitor` has no synchronous "current path" getter; the path is
//! delivered through the update handler after `start(queue:)`. To answer
//! [`current`] cheaply and synchronously we spin up a short-lived monitor on a
//! dedicated serial queue, block on a semaphore until the first path arrives
//! (the handler fires once immediately with the current path), snapshot it,
//! then cancel. This is the documented idiom for a one-shot read and is
//! bounded by the framework delivering the initial path promptly.
//!
//! ## `watch()` — the monitor lives inside the subscription
//!
//! For [`watch`] we keep the monitor running: its update handler bridges each
//! path change to the SDK callback. The retained monitor + queue + the boxed
//! callback all live inside [`Subscription`]; its `Drop` calls
//! `nw_path_monitor_cancel` and releases the retained handles, so the OS stops
//! delivering and the callback is freed exactly when the caller drops the
//! guard. No `mem::forget`.

#![allow(non_camel_case_types)]

use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use block2::RcBlock;

use crate::{Connectivity, Transport, WatchCallback};

// Opaque `nw_object` handles (all ARC-managed; we retain/release explicitly).
type nw_path_monitor_t = *mut c_void;
type nw_path_t = *mut c_void;
type dispatch_queue_t = *mut c_void;
type dispatch_semaphore_t = *mut c_void;

// `nw_path_status_t` (network/path.h).
const NW_PATH_STATUS_SATISFIED: c_int = 1;

// `nw_interface_type_t` (network/interface.h).
const NW_INTERFACE_TYPE_OTHER: c_int = 0;
const NW_INTERFACE_TYPE_WIFI: c_int = 1;
const NW_INTERFACE_TYPE_CELLULAR: c_int = 2;
const NW_INTERFACE_TYPE_WIRED: c_int = 3;
// (4 = loopback — treated as Other.)

// QOS class for the dispatch queue (dispatch/queue.h). UTILITY is fine for a
// background monitor.
const QOS_CLASS_UTILITY: usize = 0x11;

#[link(name = "Network", kind = "framework")]
extern "C" {
    fn nw_path_monitor_create() -> nw_path_monitor_t;
    fn nw_path_monitor_set_queue(monitor: nw_path_monitor_t, queue: dispatch_queue_t);
    fn nw_path_monitor_set_update_handler(
        monitor: nw_path_monitor_t,
        // nw_path_monitor_update_handler_t = void(^)(nw_path_t)
        handler: *const block2::Block<dyn Fn(nw_path_t)>,
    );
    fn nw_path_monitor_start(monitor: nw_path_monitor_t);
    fn nw_path_monitor_cancel(monitor: nw_path_monitor_t);

    fn nw_path_get_status(path: nw_path_t) -> c_int;
    fn nw_path_uses_interface_type(path: nw_path_t, interface_type: c_int) -> bool;
}

// libdispatch (libSystem — always linked).
extern "C" {
    fn dispatch_queue_attr_make_with_qos_class(
        attr: *const c_void,
        qos_class: usize,
        relative_priority: c_int,
    ) -> *const c_void;
    fn dispatch_queue_create(label: *const i8, attr: *const c_void) -> dispatch_queue_t;
    fn dispatch_semaphore_create(value: isize) -> dispatch_semaphore_t;
    fn dispatch_semaphore_signal(sema: dispatch_semaphore_t) -> isize;
    fn dispatch_semaphore_wait(sema: dispatch_semaphore_t, timeout: u64) -> isize;
    fn dispatch_release(object: *mut c_void);
}

// `nw_object` release. `nw_path_monitor_create` returns an object with a +1
// retain count we own, so we never re-retain — we just `nw_release` it once
// when the subscription drops, balancing the create.
extern "C" {
    fn nw_release(obj: *mut c_void);
}

// `dispatch_time_t` forever.
const DISPATCH_TIME_FOREVER: u64 = u64::MAX;

/// Map an `nw_path_t` to a [`Connectivity`] snapshot. Online iff the path
/// status is `satisfied`; the transport is the first matching interface type,
/// preferring wifi → cellular → wired before [`Transport::Other`].
fn path_to_connectivity(path: nw_path_t) -> Connectivity {
    if path.is_null() {
        return Connectivity::OFFLINE;
    }
    let satisfied = unsafe { nw_path_get_status(path) } == NW_PATH_STATUS_SATISFIED;
    if !satisfied {
        return Connectivity::OFFLINE;
    }
    let transport = unsafe {
        if nw_path_uses_interface_type(path, NW_INTERFACE_TYPE_WIFI) {
            Transport::Wifi
        } else if nw_path_uses_interface_type(path, NW_INTERFACE_TYPE_CELLULAR) {
            Transport::Cellular
        } else if nw_path_uses_interface_type(path, NW_INTERFACE_TYPE_WIRED) {
            Transport::Ethernet
        } else {
            // Connected over loopback / VPN / an interface type we don't
            // categorize — still online.
            let _ = NW_INTERFACE_TYPE_OTHER;
            Transport::Other
        }
    };
    Connectivity {
        online: true,
        transport,
    }
}

/// Build a serial dispatch queue for a monitor. Returns null on failure
/// (callers tolerate a null queue by skipping `set_queue`, leaving the
/// monitor inert rather than crashing).
fn make_queue(label: &[u8]) -> dispatch_queue_t {
    unsafe {
        let attr = dispatch_queue_attr_make_with_qos_class(
            std::ptr::null(),
            QOS_CLASS_UTILITY,
            0,
        );
        dispatch_queue_create(label.as_ptr() as *const i8, attr)
    }
}

/// Synchronous snapshot: a one-shot monitor blocked on the first path.
///
/// See the module docs for why this is the idiom. Bounded by the OS
/// delivering the initial path; if anything is null (allocation failure) we
/// fall back to the SDK's best-effort `ASSUME_ONLINE`.
pub(crate) fn current() -> Connectivity {
    unsafe {
        let monitor = nw_path_monitor_create();
        if monitor.is_null() {
            return Connectivity::ASSUME_ONLINE;
        }
        let queue = make_queue(b"com.idealyst.connectivity.snapshot\0");
        let sema = dispatch_semaphore_create(0);
        if queue.is_null() || sema.is_null() {
            nw_release(monitor);
            if !queue.is_null() {
                dispatch_release(queue);
            }
            if !sema.is_null() {
                dispatch_release(sema);
            }
            return Connectivity::ASSUME_ONLINE;
        }

        // Result slot the (single) update fills before signalling.
        let slot: Arc<Mutex<Option<Connectivity>>> = Arc::new(Mutex::new(None));
        let slot_in = slot.clone();
        let sema_ptr = sema as usize; // move a Copy across the block boundary

        let handler = RcBlock::new(move |path: nw_path_t| {
            // FFI boundary: never unwind into libdispatch.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let conn = path_to_connectivity(path);
                *slot_in.lock().unwrap() = Some(conn);
                dispatch_semaphore_signal(sema_ptr as dispatch_semaphore_t);
            }));
        });

        // `RcBlock` derefs to `&Block<F>`; the C API wants the block pointer.
        nw_path_monitor_set_update_handler(monitor, &*handler as *const _);
        nw_path_monitor_set_queue(monitor, queue);
        nw_path_monitor_start(monitor);

        // The handler fires once immediately with the current path.
        dispatch_semaphore_wait(sema, DISPATCH_TIME_FOREVER);
        nw_path_monitor_cancel(monitor);

        let result = slot.lock().unwrap().take();

        nw_release(monitor);
        dispatch_release(queue);
        dispatch_release(sema);

        result.unwrap_or(Connectivity::ASSUME_ONLINE)
    }
}

/// Start a long-lived monitor whose every path update bridges to `callback`.
pub(crate) fn watch(callback: WatchCallback) -> Subscription {
    unsafe {
        let monitor = nw_path_monitor_create();
        if monitor.is_null() {
            // Inert subscription — nothing to cancel; still owns the callback.
            return Subscription {
                monitor: None,
                queue: None,
                _callback_owner: Arc::new(()),
            };
        }
        let queue = make_queue(b"com.idealyst.connectivity.watch\0");

        // The callback is shared into the (repeatedly-invoked) update block.
        // Box it behind an Arc so the block holds one owner and the
        // subscription holds another for symmetry of teardown.
        let cb: Arc<WatchCallback> = Arc::new(callback);
        let cb_in = cb.clone();

        let handler = RcBlock::new(move |path: nw_path_t| {
            // FFI boundary: catch + swallow; the monitor stays live.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let conn = path_to_connectivity(path);
                (cb_in)(conn);
            }));
        });

        nw_path_monitor_set_update_handler(monitor, &*handler as *const _);
        if !queue.is_null() {
            nw_path_monitor_set_queue(monitor, queue);
            nw_path_monitor_start(monitor);
        }

        // Retain the monitor for the subscription's lifetime; the `RcBlock`
        // `handler` is held by the monitor itself (set_update_handler copies
        // it), and also by the framework, so we don't store it here — but the
        // Arc'd callback must outlive the block, which it does via `cb`.
        let _kept_block = handler; // dropped here; the framework copy persists.

        Subscription {
            monitor: NonNull::new(monitor),
            queue: NonNull::new(queue),
            _callback_owner: cb,
        }
    }
}

/// Apple subscription: holds the running monitor + its queue and the callback
/// owner. `Drop` cancels the monitor (the OS stops delivering) and releases
/// the retained handles. Cancelling before releasing is required so no
/// in-flight update touches freed state.
pub(crate) struct Subscription {
    monitor: Option<NonNull<c_void>>,
    queue: Option<NonNull<c_void>>,
    // Keeps the boxed callback alive for as long as the monitor can fire.
    _callback_owner: Arc<dyn std::any::Any>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        unsafe {
            if let Some(m) = self.monitor {
                nw_path_monitor_cancel(m.as_ptr());
                nw_release(m.as_ptr());
            }
            if let Some(q) = self.queue {
                dispatch_release(q.as_ptr());
            }
        }
    }
}

// The monitor + queue handles are only ever touched from `Drop` (and the OS's
// own serial queue, which we cancel first). The callback owner is `!Send` in
// general, matching the SDK contract that `watch` is not `Send`.

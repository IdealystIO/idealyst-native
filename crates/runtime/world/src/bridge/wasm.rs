//! The bundle side of the bridge over wasm: [`HostOps`] as wasm IMPORTS, and
//! this bundle's [`GuestHooks`](super::GuestHooks) as wasm EXPORTS.
//!
//! Compiled only into a remote bundle (`--cfg idealyst_stream_guest`, which
//! the bundle build sets — a build flag rather than a cargo feature, so it
//! can never reach an app through feature unification). There the crate's
//! active engine is `Bridged<Imports>`: the bundle's whole kernel runs on the
//! host app's graph.
//!
//! The ABI is plain wasm values. Ids cross as `i64` (the host namespaces
//! them per bundle — see `bridge::Id`); "none" is `-1`; a slot handle or a
//! `(handle, flag)` result is written to a small out-buffer in this module's
//! memory. The host implements the imports in `remote-host`.

use super::guest::Local;
use super::{GuestHooks, Handle, HostOps, Id, StageMode};
use crate::engine::{EffectClass, WorldId};

#[link(wasm_import_module = "idealyst_kernel")]
extern "C" {
    fn world_new() -> u32;
    fn world_drop(world: u32);
    fn world_flush(world: u32);
    fn world_is_flushing(world: u32) -> u32;
    fn world_provide(world: u32, key: i64, ctx: i64);
    fn world_inject(world: u32, key: i64) -> i64;
    fn enter_push(world: u32);
    fn enter_pop();

    fn is_flushing() -> u32;
    fn is_entered() -> u32;
    fn in_effect() -> u32;
    fn effect_depth() -> u32;
    fn current_effect(out: *mut u32) -> u32;
    fn in_collector() -> u32;

    fn signal_create(world: i64, value: i64, out: *mut u32);
    fn signal_check(world: u32, slot: u32, gen: u32) -> u32;
    fn signal_read_check(world: u32, slot: u32, gen: u32, track: u32) -> i32;
    fn signal_enqueue(world: u32, slot: u32, gen: u32, force: u32);
    fn signal_touch(world: u32, slot: u32, gen: u32);
    fn signal_is_alive(world: u32, slot: u32, gen: u32) -> u32;
    fn signal_subscriber_count(world: u32, slot: u32, gen: u32) -> u32;

    fn effect_create(world: i64, class: u32, effect: i64, out: *mut u32);
    fn effect_is_alive(world: u32, slot: u32, gen: u32) -> u32;
    fn on_cleanup(cleanup: i64);

    fn untrack_push();
    fn untrack_pop();
    fn unscoped_begin();
    fn unscoped_end();
    fn unanchored_begin();
    fn unanchored_end();

    fn collect_begin();
    fn collect_end() -> u32;
    fn collect_abort();
    fn scope_merge(into: u32, other: u32);
    fn scope_len(scope: u32) -> u32;
    fn scope_drop(scope: u32);

    fn ctx_provide(key: i64, ctx: i64);
    fn ctx_inject(key: i64) -> i64;

    fn value_fetch(world: u32, slot: u32, gen: u32, staged: u32, out: *mut u8, cap: u32) -> i64;
    fn value_stage(world: u32, slot: u32, gen: u32, bytes: *const u8, len: u32, mode: u32);
    fn ctx_fetch(name: *const u8, name_len: u32, out: *mut u8, cap: u32) -> i64;
}

/// Run a "write into my buffer" import: `-1` = nothing; a length larger than
/// the buffer means "grow and ask again" (the value cannot change between
/// the two calls — writes are staged until the host's flush, which never
/// runs inside a bundle call).
fn fetch_into(out: &mut Vec<u8>, mut call: impl FnMut(*mut u8, u32) -> i64) -> bool {
    out.clear();
    if out.capacity() < 64 {
        out.reserve(64);
    }
    loop {
        let cap = out.capacity();
        let len = call(out.as_mut_ptr(), cap as u32);
        if len < 0 {
            return false;
        }
        let len = len as usize;
        if len <= cap {
            // SAFETY: the host wrote exactly `len` bytes.
            unsafe { out.set_len(len) };
            return true;
        }
        out.reserve(len);
    }
}

/// Effect class on the wire.
const DERIVATION: u32 = 0;
const REACTION: u32 = 1;

fn opt_world(world: Option<WorldId>) -> i64 {
    world.map_or(-1, i64::from)
}

fn opt_id(v: i64) -> Option<Id> {
    (v >= 0).then_some(v as Id)
}

/// The host's imports, as a [`HostOps`].
pub(crate) struct Imports;

// SAFETY (all blocks below): plain-value imports implemented by the host;
// the out-pointers point at locals of the calling frame, sized for what the
// host writes (3 or 4 `u32`s).
impl HostOps for Imports {
    fn world_new() -> WorldId {
        unsafe { world_new() }
    }
    fn world_drop(world: WorldId) {
        unsafe { world_drop(world) }
    }
    fn world_flush(world: WorldId) {
        unsafe { world_flush(world) }
    }
    fn world_is_flushing(world: WorldId) -> bool {
        unsafe { world_is_flushing(world) != 0 }
    }
    fn world_provide(world: WorldId, key: Id, ctx: Id) {
        unsafe { world_provide(world, key as i64, ctx as i64) }
    }
    fn world_inject(world: WorldId, key: Id) -> Option<Id> {
        opt_id(unsafe { world_inject(world, key as i64) })
    }
    fn enter_push(world: WorldId) {
        unsafe { enter_push(world) }
    }
    fn enter_pop() {
        unsafe { enter_pop() }
    }

    fn is_flushing() -> bool {
        unsafe { is_flushing() != 0 }
    }
    fn is_entered() -> bool {
        unsafe { is_entered() != 0 }
    }
    fn in_effect() -> bool {
        unsafe { in_effect() != 0 }
    }
    fn effect_depth() -> u32 {
        unsafe { effect_depth() }
    }
    fn current_effect() -> Option<Handle> {
        let mut out = [0u32; 3];
        let has = unsafe { current_effect(out.as_mut_ptr()) };
        (has != 0).then_some((out[0], out[1], out[2]))
    }
    fn in_collector() -> bool {
        unsafe { in_collector() != 0 }
    }

    fn signal_create(world: Option<WorldId>, value: Id) -> (Handle, bool) {
        let mut out = [0u32; 4];
        unsafe { signal_create(opt_world(world), value as i64, out.as_mut_ptr()) };
        ((out[0], out[1], out[2]), out[3] != 0)
    }
    fn signal_check((w, s, g): Handle) -> bool {
        unsafe { signal_check(w, s, g) != 0 }
    }
    fn signal_read_check((w, s, g): Handle, track: bool) -> Option<bool> {
        match unsafe { signal_read_check(w, s, g, track as u32) } {
            -1 => None,
            r => Some(r != 0),
        }
    }
    fn signal_enqueue((w, s, g): Handle, force: bool) {
        unsafe { signal_enqueue(w, s, g, force as u32) }
    }
    fn signal_touch((w, s, g): Handle) {
        unsafe { signal_touch(w, s, g) }
    }
    fn signal_is_alive((w, s, g): Handle) -> bool {
        unsafe { signal_is_alive(w, s, g) != 0 }
    }
    fn signal_subscriber_count((w, s, g): Handle) -> u32 {
        unsafe { signal_subscriber_count(w, s, g) }
    }

    fn effect_create(world: Option<WorldId>, class: EffectClass, effect: Id) -> Handle {
        let class = match class {
            EffectClass::Derivation => DERIVATION,
            EffectClass::Reaction => REACTION,
        };
        let mut out = [0u32; 3];
        unsafe { effect_create(opt_world(world), class, effect as i64, out.as_mut_ptr()) };
        (out[0], out[1], out[2])
    }
    fn effect_is_alive((w, s, g): Handle) -> bool {
        unsafe { effect_is_alive(w, s, g) != 0 }
    }
    fn on_cleanup(cleanup: Id) {
        unsafe { on_cleanup(cleanup as i64) }
    }

    fn untrack_push() {
        unsafe { untrack_push() }
    }
    fn untrack_pop() {
        unsafe { untrack_pop() }
    }
    fn unscoped_begin() {
        unsafe { unscoped_begin() }
    }
    fn unscoped_end() {
        unsafe { unscoped_end() }
    }
    fn unanchored_begin() {
        unsafe { unanchored_begin() }
    }
    fn unanchored_end() {
        unsafe { unanchored_end() }
    }

    fn collect_begin() {
        unsafe { collect_begin() }
    }
    fn collect_end() -> u32 {
        unsafe { collect_end() }
    }
    fn collect_abort() {
        unsafe { collect_abort() }
    }
    fn scope_merge(into: u32, other: u32) {
        unsafe { scope_merge(into, other) }
    }
    fn scope_len(scope: u32) -> u32 {
        unsafe { scope_len(scope) }
    }
    fn scope_drop(scope: u32) {
        unsafe { scope_drop(scope) }
    }

    fn ctx_provide(key: Id, ctx: Id) {
        unsafe { ctx_provide(key as i64, ctx as i64) }
    }
    fn ctx_inject(key: Id) -> Option<Id> {
        opt_id(unsafe { ctx_inject(key as i64) })
    }

    fn value_fetch((w, s, g): Handle, staged: bool, out: &mut Vec<u8>) -> bool {
        fetch_into(out, |ptr, cap| unsafe { value_fetch(w, s, g, staged as u32, ptr, cap) })
    }
    fn value_stage((w, s, g): Handle, bytes: &[u8], mode: StageMode) {
        let mode = match mode {
            StageMode::Set => 0,
            StageMode::SetAlways => 1,
            StageMode::Untracked => 2,
        };
        unsafe { value_stage(w, s, g, bytes.as_ptr(), bytes.len() as u32, mode) }
    }
    fn ctx_fetch(name: &str, out: &mut Vec<u8>) -> bool {
        fetch_into(out, |ptr, cap| unsafe { ctx_fetch(name.as_ptr(), name.len() as u32, ptr, cap) })
    }
}

// ---------------------------------------------------------------------------
// This bundle's hooks, exported for the host's proxies to call.
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn idealyst_kernel_commit(value: i64, forced: u32) -> u32 {
    Local::commit(value as Id, forced != 0) as u32
}

#[no_mangle]
pub extern "C" fn idealyst_kernel_drop_value(value: i64) {
    Local::drop_value(value as Id)
}

#[no_mangle]
pub extern "C" fn idealyst_kernel_run_effect(effect: i64) {
    Local::run_effect(effect as Id)
}

#[no_mangle]
pub extern "C" fn idealyst_kernel_drop_effect(effect: i64) {
    Local::drop_effect(effect as Id)
}

#[no_mangle]
pub extern "C" fn idealyst_kernel_run_cleanup(cleanup: i64) {
    Local::run_cleanup(cleanup as Id)
}

#[no_mangle]
pub extern "C" fn idealyst_kernel_drop_cleanup(cleanup: i64) {
    Local::drop_cleanup(cleanup as Id)
}

#[no_mangle]
pub extern "C" fn idealyst_kernel_drop_context(ctx: i64) {
    Local::drop_context(ctx as Id)
}

thread_local! {
    /// The last promotion's encoding, read by the host right after the call.
    static PROMOTED: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[no_mangle]
pub extern "C" fn idealyst_kernel_promote_finish(value: i64) {
    Local::promote_finish(value as Id)
}

/// [`GuestHooks::promote`] over wasm: `-1` when not offered; else a packed
/// `ptr << 32 | len` of `[has_staged: u8][committed_len: u32 le][committed][staged]`
/// in this module's memory, valid until the next promotion.
#[no_mangle]
pub extern "C" fn idealyst_kernel_promote(value: i64) -> i64 {
    let (mut committed, mut staged) = (Vec::new(), Vec::new());
    let Some(has_staged) = Local::promote(value as Id, &mut committed, &mut staged) else {
        return -1;
    };
    PROMOTED.with(|p| {
        let mut p = p.borrow_mut();
        p.clear();
        p.push(has_staged as u8);
        p.extend_from_slice(&(committed.len() as u32).to_le_bytes());
        p.extend_from_slice(&committed);
        p.extend_from_slice(&staged);
        ((p.as_ptr() as u32 as i64) << 32) | p.len() as i64
    })
}

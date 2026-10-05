//! The kernel bridge over wasm, host side.
//!
//! A remote bundle built with `--cfg idealyst_stream_guest` runs
//! runtime-world's BRIDGED engine: every graph operation of its kernel is a
//! wasm import from module `idealyst_kernel`, and the bundle exports
//! `idealyst_kernel_*` hooks for the host's proxies to call back. This module
//! serves those imports with `runtime_world::remote::Host` — the app's own
//! native graph — so a bundle's signals, effects, memos, scopes and context
//! live in the app's arena (see `runtime-world/src/bridge/mod.rs`).
//!
//! # Ids are namespaced per bundle
//!
//! A bundle numbers its values, effect bodies, cleanups, context values and
//! context keys itself (`u32` range). Several bundles share one graph, so
//! every id is widened on the way in to `bundle << 32 | local` and split on
//! the way out, which is how a proxy's callback finds its bundle — and why
//! two bundles' context keys can never collide.
//!
//! # Re-entrancy
//!
//! The host calls back into a bundle while that bundle is mid-call
//! constantly: a bundle creating an effect calls `effect_create`, and the
//! kernel runs the new effect's body — a call back into the same bundle —
//! before the import returns. wasmi permits that only through the import's
//! own `Caller`. So each import publishes its `Caller` on [`ACTIVE`] for its
//! duration, and a hook bound for a bundle with an import in flight calls
//! through that `Caller`; otherwise it borrows the bundle's `Store` directly
//! (the host flushing from its own code, outside any bundle call).

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use runtime_world::remote::{EffectClass, GuestHooks, Handle, Host, HostOps, Id, StageMode, WorldId};
use rustc_hash::FxHashMap;
use runtime_vocabulary::remote::host_fn::{HostFnDef, HostFnKind, HOST_FN_MODULE};
use wasmi::{AsContextMut, Caller, Engine, ExternType, Linker, Memory, Module, Store, TypedFunc, Val, ValType};

use crate::LoadError;

/// The host side as the kernel imports see it.
type H = Host<WasmGuest>;

const MODULE: &str = "idealyst_kernel";

/// Per-store data: which bundle this is, and its memory (for out-buffers).
pub struct KState {
    bundle: u32,
    memory: Option<Memory>,
}

/// The element codec's exports (`runtime_vocabulary::remote::wasm`); only a
/// bundle that renders UI has them.
#[derive(Clone, Copy)]
struct UiHooks {
    panic_message: Option<TypedFunc<(), i64>>,
    alloc: TypedFunc<u32, u32>,
    invoke: TypedFunc<(u32, u32), i64>,
    release: TypedFunc<u32, ()>,
}

#[derive(Clone, Copy)]
struct Hooks {
    commit: TypedFunc<(i64, u32), u32>,
    drop_value: TypedFunc<i64, ()>,
    run_effect: TypedFunc<i64, ()>,
    drop_effect: TypedFunc<i64, ()>,
    run_cleanup: TypedFunc<i64, ()>,
    drop_cleanup: TypedFunc<i64, ()>,
    drop_context: TypedFunc<i64, ()>,
    /// Promotion (`runtime_world::remote::receive_signal`): optional, so a
    /// bundle built before it still loads (its signals just never promote).
    promote: Option<TypedFunc<i64, i64>>,
    promote_finish: Option<TypedFunc<i64, ()>>,
    ui: Option<UiHooks>,
}

struct Inner {
    id: u32,
    store: RefCell<Store<KState>>,
    instance: wasmi::Instance,
    hooks: Hooks,
    /// Set by the bundle's first trap, with its panic message: the bundle is
    /// POISONED and never called again (see [`poison`]).
    poisoned: RefCell<Option<String>>,
    /// Told once, when the bundle is poisoned (`KernelBundle::on_poison`).
    on_poison: RefCell<Vec<Rc<dyn Fn(&str)>>>,
}

/// A trap ends a bundle. A Rust panic in wasm is `panic = "abort"`: the
/// trap unwinds the interpreter, but no destructor in the bundle runs, so a
/// `RefCell` it had borrowed stays borrowed and a table it was updating
/// stays half-updated. Calling it again would trip over that state — at
/// best another trap, at worst wrong answers. So the first trap POISONS
/// it: every later call answers `None` without entering the bundle (each
/// hook's caller already treats `None` as "bundle gone"), and the
/// `on_poison` listeners (the remote loader) replace its components with
/// the panic message. The app keeps running.
///
/// The same abort leaves the kernel frames the bundle opened in the APP
/// (an entered world, `untrack`, a collecting scope) open, so every entry
/// into a bundle marks them first and a trap unwinds back to the mark
/// (`runtime_world::remote::unwind_bundle_frames`) before poisoning.
fn poison(inner: &Inner, msg: String) {
    if inner.poisoned.borrow().is_some() {
        return;
    }
    eprintln!("[remote] bundle {} panicked and was stopped: {msg}", inner.id);
    *inner.poisoned.borrow_mut() = Some(msg.clone());
    // Listeners run outside the borrow: they may register more.
    let listeners = inner.on_poison.borrow().clone();
    for f in listeners {
        f(&msg);
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = BUNDLES.try_with(|b| b.borrow_mut().remove(&self.id));
    }
}

thread_local! {
    /// Live bundles by id — how a namespaced id's callback finds its bundle.
    static BUNDLES: RefCell<FxHashMap<u32, Weak<Inner>>> = RefCell::new(FxHashMap::default());
    static NEXT_BUNDLE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Imports in flight, innermost last: `(bundle, *mut Caller<KState>)`.
    /// The pointer is valid exactly while its import runs (it is pushed and
    /// popped around the import's body, see [`with_active`]).
    static ACTIVE: RefCell<Vec<(u32, *mut ())>> = const { RefCell::new(Vec::new()) };
}

fn ns(bundle: u32, local: i64) -> Id {
    assert!(
        (0..=u32::MAX as i64).contains(&local),
        "kernel bridge: bundle {bundle} sent id {local}, outside its u32 id space"
    );
    ((bundle as u64) << 32) | local as u64
}

fn split(id: Id) -> (u32, i64) {
    ((id >> 32) as u32, (id & 0xffff_ffff) as i64)
}

fn opt(v: Option<Id>) -> i64 {
    v.map_or(-1, |id| split(id).1)
}

/// Run an import's body with its `Caller` published for re-entrant hooks.
fn with_active<R>(caller: &mut Caller<'_, KState>, f: impl FnOnce() -> R) -> R {
    let bundle = caller.data().bundle;
    let ptr = caller as *mut Caller<'_, KState> as *mut ();
    ACTIVE.with(|a| a.borrow_mut().push((bundle, ptr)));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            let _ = ACTIVE.try_with(|a| a.borrow_mut().pop());
        }
    }
    let _pop = Pop;
    f()
}

/// One call into a bundle, over either context. A trait object because
/// `TypedFunc::call` is generic over the context type and the two paths
/// (`Caller` / `Store`) are different types.
trait GuestCall {
    fn call_i64(&mut self, f: TypedFunc<i64, ()>, arg: i64) -> Result<(), wasmi::Error>;
    fn commit(&mut self, f: TypedFunc<(i64, u32), u32>, value: i64, forced: u32) -> Result<u32, wasmi::Error>;
    /// Call the bundle's promote export; its packed reply, copied out.
    fn promote(&mut self, f: TypedFunc<i64, i64>, value: i64) -> Result<Option<Vec<u8>>, wasmi::Error>;
    /// Run element callback `cb` on `args`; its reply.
    fn ui_invoke(&mut self, ui: UiHooks, cb: u32, args: &[u8]) -> Result<Vec<u8>, wasmi::Error>;
    fn ui_release(&mut self, ui: UiHooks, cb: u32) -> Result<(), wasmi::Error>;
    /// The bundle's last panic message (after a trap).
    fn last_panic(&mut self, ui: UiHooks) -> Option<String>;
}

impl<C: AsContextMut<Data = KState>> GuestCall for C {
    fn call_i64(&mut self, f: TypedFunc<i64, ()>, arg: i64) -> Result<(), wasmi::Error> {
        f.call(&mut *self, arg)
    }
    fn commit(&mut self, f: TypedFunc<(i64, u32), u32>, value: i64, forced: u32) -> Result<u32, wasmi::Error> {
        f.call(&mut *self, (value, forced))
    }
    fn promote(&mut self, f: TypedFunc<i64, i64>, value: i64) -> Result<Option<Vec<u8>>, wasmi::Error> {
        let packed = f.call(&mut *self, value)?;
        if packed < 0 {
            return Ok(None);
        }
        let memory = self.as_context().data().memory.expect("kernel bridge: bundle exports no memory");
        Ok(Some(read_packed(&*self, memory, packed)?))
    }
    fn ui_invoke(&mut self, ui: UiHooks, cb: u32, args: &[u8]) -> Result<Vec<u8>, wasmi::Error> {
        let memory = self.as_context().data().memory.expect("kernel bridge: bundle exports no memory");
        let ptr = ui.alloc.call(&mut *self, args.len() as u32)?;
        memory
            .write(&mut *self, ptr as usize, args)
            .map_err(|_| wasmi::Error::new(format!("remote codec: argument buffer {ptr} out of bounds")))?;
        let packed = ui.invoke.call(&mut *self, (cb, args.len() as u32))?;
        read_packed(&*self, memory, packed)
    }
    fn ui_release(&mut self, ui: UiHooks, cb: u32) -> Result<(), wasmi::Error> {
        ui.release.call(&mut *self, cb)
    }
    fn last_panic(&mut self, ui: UiHooks) -> Option<String> {
        panic_message(self, ui)
    }
}

/// Copy a `ptr << 32 | len` result out of the bundle's memory.
/// `Err` (the bundle is stopped) when the bundle's pointer is out of bounds.
fn read_packed(ctx: impl wasmi::AsContext, memory: Memory, packed: i64) -> Result<Vec<u8>, wasmi::Error> {
    let (ptr, len) = ((packed >> 32) as u32 as usize, packed as u32 as usize);
    let mut out = vec![0u8; len];
    memory
        .read(&ctx, ptr, &mut out)
        .map_err(|_| wasmi::Error::new(format!("remote codec: reply buffer {ptr}+{len} out of bounds")))?;
    Ok(out)
}

/// Route one hook to bundle `bundle`: through its in-flight import's
/// `Caller` if it has one, else through its `Store`. `None` when the bundle
/// is gone (its proxies outlived it), the thread is tearing down, or the
/// bundle is POISONED — by an earlier trap, or by trapping in this very
/// call (see [`poison`]).
fn route<R>(bundle: u32, f: impl FnOnce(&mut dyn GuestCall, Hooks) -> Result<R, wasmi::Error>) -> Option<R> {
    let inner = BUNDLES.try_with(|b| b.borrow().get(&bundle).and_then(Weak::upgrade)).ok().flatten()?;
    if inner.poisoned.borrow().is_some() {
        return None;
    }
    let active = ACTIVE.try_with(|a| a.borrow().iter().rev().find(|(b, _)| *b == bundle).map(|(_, p)| *p)).ok().flatten();
    // A trap skips the bundle's end calls for every kernel frame it opened
    // during this call; they are closed from here (see `poison`).
    let mark = runtime_world::remote::bundle_frames_mark();
    let result = match active {
        Some(ptr) => {
            // SAFETY: `ptr` was published by `with_active` from a live
            // `&mut Caller` whose import is still on the stack (it is popped
            // when that import's body returns), and that import is suspended
            // inside the host call that led here, so nothing else is using
            // the `Caller` for the duration of this call.
            let caller = unsafe { &mut *(ptr as *mut Caller<'static, KState>) };
            f(caller, inner.hooks).map_err(|e| with_panic(e, caller, inner.hooks))
        }
        None => {
            let mut store = inner.store.try_borrow_mut().unwrap_or_else(|_| {
                panic!(
                    "kernel bridge: bundle {bundle}'s store is busy with no import in flight — a \
                     host→bundle call re-entered the bundle without going through an import"
                )
            });
            f(&mut *store, inner.hooks).map_err(|e| with_panic(e, &mut *store, inner.hooks))
        }
    };
    match result {
        Ok(r) => Some(r),
        Err(msg) => {
            runtime_world::remote::unwind_bundle_frames(mark);
            poison(&inner, msg);
            None
        }
    }
}

/// A trap, described with the bundle's own panic message when it kept one.
fn with_panic(e: wasmi::Error, c: &mut dyn GuestCall, hooks: Hooks) -> String {
    match hooks.ui.and_then(|ui| c.last_panic(ui)) {
        Some(msg) => format!("{msg} [{e}]"),
        None => e.to_string(),
    }
}

/// The hooks, as `runtime_world::remote::GuestHooks` sees them.
pub struct WasmGuest;

impl GuestHooks for WasmGuest {
    fn commit(value: Id, forced: bool) -> bool {
        let (bundle, local) = split(value);
        route(bundle, |c, h| c.commit(h.commit, local, forced as u32)).is_some_and(|r| r != 0)
    }
    fn drop_value(value: Id) {
        let (bundle, local) = split(value);
        route(bundle, |c, h| c.call_i64(h.drop_value, local));
    }
    fn run_effect(effect: Id) {
        let (bundle, local) = split(effect);
        route(bundle, |c, h| c.call_i64(h.run_effect, local));
    }
    fn drop_effect(effect: Id) {
        let (bundle, local) = split(effect);
        route(bundle, |c, h| c.call_i64(h.drop_effect, local));
    }
    fn run_cleanup(cleanup: Id) {
        let (bundle, local) = split(cleanup);
        route(bundle, |c, h| c.call_i64(h.run_cleanup, local));
    }
    fn drop_cleanup(cleanup: Id) {
        let (bundle, local) = split(cleanup);
        route(bundle, |c, h| c.call_i64(h.drop_cleanup, local));
    }
    fn drop_context(ctx: Id) {
        let (bundle, local) = split(ctx);
        route(bundle, |c, h| c.call_i64(h.drop_context, local));
    }
    fn promote(value: Id, committed: &mut Vec<u8>, staged: &mut Vec<u8>) -> Option<bool> {
        let (bundle, local) = split(value);
        let reply = route(bundle, |c, h| match h.promote {
            Some(f) => c.promote(f, local),
            None => Ok(None),
        })??;
        let Some((flag, c, s)) = parse_promotion(&reply) else {
            stop(bundle, format!("kernel bridge: a malformed promotion reply ({} bytes)", reply.len()));
            return None;
        };
        committed.extend_from_slice(c);
        staged.extend_from_slice(s);
        Some(flag != 0)
    }
    fn promote_finish(value: Id) {
        let (bundle, local) = split(value);
        route(bundle, |c, h| match h.promote_finish {
            Some(f) => c.call_i64(f, local),
            None => Ok(()),
        });
    }
}

/// A promotion reply, `[has_staged: u8][committed_len: u32 le][committed]
/// [staged]`: `None` when it is malformed (the bundle is then stopped).
fn parse_promotion(reply: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&flag, rest) = reply.split_first()?;
    let len = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
    let body = rest.get(4..)?;
    Some((flag, body.get(..len)?, body.get(len..)?))
}

// ---------------------------------------------------------------------------
// The imports
// ---------------------------------------------------------------------------

/// An import's outcome: `Err` traps the bundle that called it (see
/// [`poison`]) — for a request the app refused (`runtime_world::remote::
/// take_fault`) or bytes it could not read.
type Imported<T> = Result<T, wasmi::Error>;

fn refused(msg: impl Into<String>) -> wasmi::Error {
    wasmi::Error::new(msg.into())
}

fn memory(caller: &Caller<'_, KState>) -> Imported<Memory> {
    caller.data().memory.ok_or_else(|| refused("kernel bridge: bundle exports no memory"))
}

fn write_u32s(caller: &mut Caller<'_, KState>, ptr: u32, vals: &[u32]) -> Imported<()> {
    let bytes: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
    write_bytes(caller, ptr, &bytes)
}

fn read_bytes(caller: &Caller<'_, KState>, ptr: u32, len: u32) -> Imported<Vec<u8>> {
    let mut buf = vec![0u8; len as usize];
    memory(caller)?
        .read(caller, ptr as usize, &mut buf)
        .map_err(|_| refused(format!("kernel bridge: bundle buffer {ptr}+{len} is out of bounds")))?;
    Ok(buf)
}

fn write_bytes(caller: &mut Caller<'_, KState>, ptr: u32, bytes: &[u8]) -> Imported<()> {
    memory(caller)?
        .write(&mut *caller, ptr as usize, bytes)
        .map_err(|_| refused(format!("kernel bridge: bundle out-buffer {ptr}+{} is out of bounds", bytes.len())))
}

fn opt_world(world: i64) -> Option<WorldId> {
    (world >= 0).then_some(world as WorldId)
}

/// One of the bundle's ids, namespaced by bundle.
fn ns_checked(bundle: u32, local: i64) -> Imported<Id> {
    if !(0..=u32::MAX as i64).contains(&local) {
        return Err(refused(format!("kernel bridge: bundle {bundle} sent id {local}, outside its u32 id space")));
    }
    Ok(ns(bundle, local))
}

/// Run an import's body as [`with_active`] does; a fault it raised in the
/// app's kernel (an invalid request) traps the bundle instead of being
/// answered.
fn kernel<R>(caller: &mut Caller<'_, KState>, f: impl FnOnce() -> R) -> Imported<R> {
    let r = with_active(caller, f);
    match runtime_world::remote::take_fault() {
        Some(msg) => Err(refused(msg)),
        None => Ok(r),
    }
}

macro_rules! import {
    ($linker:ident, $name:literal, |$caller:ident $(, $arg:ident : $ty:ty)*| -> $ret:ty $body:block) => {
        $linker
            .func_wrap(MODULE, $name, |mut $caller: Caller<'_, KState> $(, $arg: $ty)*| -> Imported<$ret> {
                kernel(&mut $caller, || $body)
            })
            .unwrap_or_else(|e| panic!("define {}: {e}", $name));
    };
    ($linker:ident, $name:literal, |$caller:ident $(, $arg:ident : $ty:ty)*| $body:block) => {
        import!($linker, $name, |$caller $(, $arg: $ty)*| -> () $body);
    };
}

/// Define every `idealyst_kernel` import on `linker`.
pub fn define_imports(linker: &mut Linker<KState>) {
    import!(linker, "world_new", |c| -> u32 { H::world_new() });
    import!(linker, "world_drop", |c, w: u32| { H::world_drop(w) });
    import!(linker, "world_flush", |c, w: u32| { H::world_flush(w) });
    import!(linker, "world_is_flushing", |c, w: u32| -> u32 { H::world_is_flushing(w) as u32 });
    linker
        .func_wrap(MODULE, "world_provide", |mut c: Caller<'_, KState>, w: u32, key: i64, ctx: i64| -> Imported<()> {
            let b = c.data().bundle;
            let (key, ctx) = (ns_checked(b, key)?, ns_checked(b, ctx)?);
            kernel(&mut c, || H::world_provide(w, key, ctx))
        })
        .expect("define world_provide");
    linker
        .func_wrap(MODULE, "world_inject", |mut c: Caller<'_, KState>, w: u32, key: i64| -> Imported<i64> {
            let key = ns_checked(c.data().bundle, key)?;
            kernel(&mut c, || opt(H::world_inject(w, key)))
        })
        .expect("define world_inject");
    import!(linker, "enter_push", |c, w: u32| { H::enter_push(w) });
    import!(linker, "enter_pop", |c| { H::enter_pop() });

    import!(linker, "is_flushing", |c| -> u32 { H::is_flushing() as u32 });
    import!(linker, "is_entered", |c| -> u32 { H::is_entered() as u32 });
    import!(linker, "in_effect", |c| -> u32 { H::in_effect() as u32 });
    import!(linker, "effect_depth", |c| -> u32 { H::effect_depth() });
    linker
        .func_wrap(MODULE, "current_effect", |mut c: Caller<'_, KState>, out: u32| -> Imported<u32> {
            match kernel(&mut c, H::current_effect)? {
                Some((w, s, g)) => {
                    write_u32s(&mut c, out, &[w, s, g])?;
                    Ok(1)
                }
                None => Ok(0),
            }
        })
        .expect("define current_effect");
    import!(linker, "in_collector", |c| -> u32 { H::in_collector() as u32 });

    // A bundle calling a method on a handle the app holds for it (a `ref`;
    // see runtime_vocabulary::remote::handles). The reply goes into the
    // bundle's argument buffer, like a sync host function's.
    linker
        .func_wrap("idealyst_ui", "handle_call", |mut c: Caller<'_, KState>, id: u32, ptr: u32, len: u32| -> Imported<i64> {
            let args = read_bytes(&c, ptr, len)?;
            let caller = c.data().bundle as u64;
            let reply =
                kernel(&mut c, || runtime_vocabulary::remote::handles::handle_call(caller, id, &args))?.map_err(refused)?;
            let alloc = c
                .get_export("idealyst_ui_alloc")
                .and_then(|e| e.into_func())
                .ok_or_else(|| refused("handle_call: the bundle exports no idealyst_ui_alloc"))?
                .typed::<u32, u32>(&c)?;
            let out = alloc.call(&mut c, reply.len() as u32)?;
            write_bytes(&mut c, out, &reply)?;
            Ok(reply.len() as i64)
        })
        .expect("define handle_call");

    linker
        .func_wrap(MODULE, "signal_create", |mut c: Caller<'_, KState>, world: i64, value: i64, out: u32| -> Imported<()> {
            let value = ns_checked(c.data().bundle, value)?;
            let ((w, s, g), collected) = kernel(&mut c, || H::signal_create(opt_world(world), value))?;
            write_u32s(&mut c, out, &[w, s, g, collected as u32])
        })
        .expect("define signal_create");
    import!(linker, "signal_check", |c, w: u32, s: u32, g: u32| -> u32 { H::signal_check((w, s, g)) as u32 });
    import!(linker, "signal_read_check", |c, w: u32, s: u32, g: u32, track: u32| -> i32 {
        match H::signal_read_check((w, s, g), track != 0) {
            None => -1,
            Some(sub) => sub as i32,
        }
    });
    import!(linker, "signal_enqueue", |c, w: u32, s: u32, g: u32, force: u32| { H::signal_enqueue((w, s, g), force != 0) });
    import!(linker, "signal_touch", |c, w: u32, s: u32, g: u32| { H::signal_touch((w, s, g)) });
    import!(linker, "signal_is_alive", |c, w: u32, s: u32, g: u32| -> u32 { H::signal_is_alive((w, s, g)) as u32 });
    import!(linker, "signal_subscriber_count", |c, w: u32, s: u32, g: u32| -> u32 {
        H::signal_subscriber_count((w, s, g))
    });

    linker
        .func_wrap(MODULE, "effect_create", |mut c: Caller<'_, KState>, world: i64, class: u32, effect: i64, out: u32| -> Imported<()> {
            let effect = ns_checked(c.data().bundle, effect)?;
            let class = if class == 0 { EffectClass::Derivation } else { EffectClass::Reaction };
            let (w, s, g): Handle = kernel(&mut c, || H::effect_create(opt_world(world), class, effect))?;
            write_u32s(&mut c, out, &[w, s, g])
        })
        .expect("define effect_create");
    import!(linker, "effect_is_alive", |c, w: u32, s: u32, g: u32| -> u32 { H::effect_is_alive((w, s, g)) as u32 });
    linker
        .func_wrap(MODULE, "on_cleanup", |mut c: Caller<'_, KState>, cleanup: i64| -> Imported<()> {
            let cleanup = ns_checked(c.data().bundle, cleanup)?;
            kernel(&mut c, || H::on_cleanup(cleanup))
        })
        .expect("define on_cleanup");

    import!(linker, "untrack_push", |c| { H::untrack_push() });
    import!(linker, "untrack_pop", |c| { H::untrack_pop() });
    import!(linker, "unscoped_begin", |c| { H::unscoped_begin() });
    import!(linker, "unscoped_end", |c| { H::unscoped_end() });
    import!(linker, "unanchored_begin", |c| { H::unanchored_begin() });
    import!(linker, "unanchored_end", |c| { H::unanchored_end() });

    import!(linker, "collect_begin", |c| { H::collect_begin() });
    import!(linker, "collect_end", |c| -> u32 { H::collect_end() });
    import!(linker, "collect_abort", |c| { H::collect_abort() });
    import!(linker, "scope_merge", |c, into: u32, other: u32| { H::scope_merge(into, other) });
    import!(linker, "scope_len", |c, scope: u32| -> u32 { H::scope_len(scope) });
    import!(linker, "scope_drop", |c, scope: u32| { H::scope_drop(scope) });

    linker
        .func_wrap(MODULE, "ctx_provide", |mut c: Caller<'_, KState>, key: i64, ctx: i64| -> Imported<()> {
            let b = c.data().bundle;
            let (key, ctx) = (ns_checked(b, key)?, ns_checked(b, ctx)?);
            kernel(&mut c, || H::ctx_provide(key, ctx))
        })
        .expect("define ctx_provide");
    linker
        .func_wrap(MODULE, "ctx_inject", |mut c: Caller<'_, KState>, key: i64| -> Imported<i64> {
            let key = ns_checked(c.data().bundle, key)?;
            kernel(&mut c, || opt(H::ctx_inject(key)))
        })
        .expect("define ctx_inject");

    // Host-owned values: a fetch writes into the bundle's buffer when it
    // fits and always returns the full length (the bundle grows and asks
    // again); `-1` = nothing.
    linker
        .func_wrap(
            MODULE,
            "value_fetch",
            |mut c: Caller<'_, KState>, w: u32, s: u32, g: u32, staged: u32, out: u32, cap: u32| -> Imported<i64> {
                let mut buf = Vec::new();
                if !kernel(&mut c, || H::value_fetch((w, s, g), staged != 0, &mut buf))? {
                    return Ok(-1);
                }
                if buf.len() <= cap as usize {
                    write_bytes(&mut c, out, &buf)?;
                }
                Ok(buf.len() as i64)
            },
        )
        .expect("define value_fetch");
    linker
        .func_wrap(
            MODULE,
            "value_stage",
            |mut c: Caller<'_, KState>, w: u32, s: u32, g: u32, ptr: u32, len: u32, mode: u32| -> Imported<()> {
                let bytes = read_bytes(&c, ptr, len)?;
                let mode = match mode {
                    0 => StageMode::Set,
                    1 => StageMode::SetAlways,
                    _ => StageMode::Untracked,
                };
                kernel(&mut c, || H::value_stage((w, s, g), &bytes, mode))
            },
        )
        .expect("define value_stage");
    linker
        .func_wrap(
            MODULE,
            "ctx_fetch",
            |mut c: Caller<'_, KState>, name: u32, name_len: u32, out: u32, cap: u32| -> Imported<i64> {
                let name = String::from_utf8(read_bytes(&c, name, name_len)?)
                    .map_err(|_| refused("kernel bridge: a bundle's context name is not UTF-8"))?;
                let mut buf = Vec::new();
                if !kernel(&mut c, || H::ctx_fetch(&name, &mut buf))? {
                    return Ok(-1);
                }
                if buf.len() <= cap as usize {
                    write_bytes(&mut c, out, &buf)?;
                }
                Ok(buf.len() as i64)
            },
        )
        .expect("define ctx_fetch");
}

// ---------------------------------------------------------------------------
// A loaded bundle
// ---------------------------------------------------------------------------

/// A bundle whose kernel runs on this app's graph.
pub struct KernelBundle {
    inner: Rc<Inner>,
}

impl KernelBundle {
    /// Instantiate `wasm`, which must import only `idealyst_kernel` and
    /// export the `idealyst_kernel_*` hooks. A bundle that calls any
    /// `#[host_fn]` needs [`load_with`](Self::load_with).
    pub fn load(engine: &Engine, wasm: &[u8]) -> Result<KernelBundle, LoadError> {
        Self::load_with(engine, wasm, &[])
    }

    /// [`load`](Self::load), letting the bundle call `host_fns` — the app's
    /// allowlist (`my_sdk::take_photo::export()`, …). Checked before the
    /// bundle runs: a bundle that calls a host function not in the list is
    /// refused with [`LoadError::MissingHostFunctions`], one whose
    /// signature changed since the app was built with
    /// [`LoadError::IncompatibleHostFunctions`].
    pub fn load_with(engine: &Engine, wasm: &[u8], host_fns: &[HostFnDef]) -> Result<KernelBundle, LoadError> {
        // From here on a bundle's invalid kernel request stops that bundle
        // instead of panicking the app (`kernel`).
        runtime_world::remote::trap_faults();
        let module = Module::new(engine, wasm)?;
        let bundle = NEXT_BUNDLE.with(|n| {
            let id = n.get().checked_add(1).expect("kernel bridge: bundle ids exhausted");
            n.set(id);
            id
        });
        let mut store = Store::new(engine, KState { bundle, memory: None });
        let mut linker = Linker::new(engine);
        define_imports(&mut linker);
        link_host_fns(&module, host_fns, &mut linker)?;
        let instance = linker.instantiate_and_start(&mut store, &module)?;
        // Required here, so every later `memory.expect` is an invariant,
        // not a bundle-controlled panic.
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| LoadError::Wasm(wasmi::Error::new("the bundle exports no memory")))?;
        store.data_mut().memory = Some(memory);
        let hooks = Hooks {
            commit: instance.get_typed_func(&store, "idealyst_kernel_commit")?,
            drop_value: instance.get_typed_func(&store, "idealyst_kernel_drop_value")?,
            run_effect: instance.get_typed_func(&store, "idealyst_kernel_run_effect")?,
            drop_effect: instance.get_typed_func(&store, "idealyst_kernel_drop_effect")?,
            run_cleanup: instance.get_typed_func(&store, "idealyst_kernel_run_cleanup")?,
            drop_cleanup: instance.get_typed_func(&store, "idealyst_kernel_drop_cleanup")?,
            drop_context: instance.get_typed_func(&store, "idealyst_kernel_drop_context")?,
            promote: instance.get_typed_func(&store, "idealyst_kernel_promote").ok(),
            promote_finish: instance.get_typed_func(&store, "idealyst_kernel_promote_finish").ok(),
            ui: match (
                instance.get_typed_func(&store, "idealyst_ui_alloc"),
                instance.get_typed_func(&store, "idealyst_ui_invoke"),
                instance.get_typed_func(&store, "idealyst_ui_release"),
            ) {
                (Ok(alloc), Ok(invoke), Ok(release)) => Some(UiHooks {
                    panic_message: instance.get_typed_func(&store, "idealyst_ui_panic_message").ok(),
                    alloc,
                    invoke,
                    release,
                }),
                _ => None,
            },
        };
        // Register the bundle's context decoders (`#[derive(Remote)]`): one
        // export per type (a wasm bundle has no link-time registry).
        let ctx_exports: Vec<String> = module
            .exports()
            .map(|e| e.name().to_string())
            .filter(|n| n.starts_with(runtime_vocabulary::remote::CONTEXT_EXPORT_PREFIX))
            .collect();
        // A trap here refuses the bundle; close any kernel frames it left
        // open first (see `route`).
        let mark = runtime_world::remote::bundle_frames_mark();
        let started = (|| -> Result<(), wasmi::Error> {
            if let Ok(init) = instance.get_typed_func::<(), ()>(&store, "idealyst_ui_init") {
                init.call(&mut store, ())?;
            }
            for name in ctx_exports {
                instance.get_typed_func::<(), ()>(&store, &name)?.call(&mut store, ())?;
            }
            Ok(())
        })();
        if let Err(e) = started {
            runtime_world::remote::unwind_bundle_frames(mark);
            return Err(e.into());
        }
        let inner = Rc::new(Inner {
            id: bundle,
            store: RefCell::new(store),
            instance,
            hooks,
            poisoned: RefCell::new(None),
            on_poison: RefCell::new(Vec::new()),
        });
        BUNDLES.with(|b| b.borrow_mut().insert(bundle, Rc::downgrade(&inner)));
        Ok(KernelBundle { inner })
    }

    /// Call a bundle export of shape `(params) -> results` from host code
    /// (outside any import). Panics on a trap, like every bridge call.
    pub fn call<P: wasmi::WasmParams, R: wasmi::WasmResults>(&self, export: &str, params: P) -> R {
        let f: TypedFunc<P, R> = {
            let store = self.inner.store.borrow();
            self.inner
                .instance
                .get_typed_func(&*store, export)
                .unwrap_or_else(|e| panic!("kernel bridge: no export `{export}` of that shape: {e}"))
        };
        let mut store = self.inner.store.try_borrow_mut().unwrap_or_else(|_| {
            panic!("kernel bridge: KernelBundle::call(`{export}`) re-entered a bundle that is already running")
        });
        f.call(&mut *store, params).unwrap_or_else(|e| panic!("kernel bridge: `{export}` trapped: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Handing host state to bundles
// ---------------------------------------------------------------------------

pub use runtime_world::remote::ExportGuard;
use stream_abi::Wire;

fn wire_encode<T: Wire>(v: &T, out: &mut Vec<u8>) {
    v.encode(out)
}

fn wire_decode<T: Wire>(b: &[u8]) -> Option<T> {
    T::from_bytes(b)
}

fn wire_codec<T: Wire>() -> runtime_world::remote::Codec<T> {
    runtime_world::remote::Codec { encode: wire_encode::<T>, decode: wire_decode::<T> }
}

/// Hand `sig` to bundles as a two-way prop. The handle goes into the mount's
/// props; the guard withdraws the export (keep it as long as the mount).
pub fn export_signal<T: Wire + PartialEq + 'static>(sig: runtime_world::Signal<T>) -> (Handle, ExportGuard) {
    runtime_world::remote::export_signal(sig, wire_codec::<T>())
}

/// Hand `sig` to bundles as a read-only prop.
pub fn export_read_signal<T: Wire + PartialEq + 'static>(sig: runtime_world::ReadSignal<T>) -> (Handle, ExportGuard) {
    runtime_world::remote::export_read_signal(sig, wire_codec::<T>())
}

/// Declare host context of type `T` to bundles under `name`: a bundle that
/// registered the same name gets the value the host has provided at the
/// moment it injects, encoded by `encode`.
pub fn export_context<T: Clone + 'static>(name: &str, encode: impl Fn(&T, &mut Vec<u8>) + 'static) -> ExportGuard {
    runtime_world::remote::export_context(name, move |out| match runtime_world::inject::<T>() {
        Some(v) => {
            encode(&v, out);
            true
        }
        None => false,
    })
}

impl KernelBundle {
    /// Copy `len` bytes out of the bundle's memory (test and glue helper).
    pub fn read_memory(&self, ptr: u32, len: u32) -> Vec<u8> {
        let store = self.inner.store.borrow();
        let memory = store.data().memory.expect("bundle exports no memory");
        let mut buf = vec![0u8; len as usize];
        memory.read(&*store, ptr as usize, &mut buf).expect("in bounds");
        buf
    }

    /// The size of the bundle's linear memory, in bytes: what it holds
    /// beside the app (its own heap, stack and data). Wasm memory only
    /// grows, so this is the high-water mark.
    pub fn memory_bytes(&self) -> usize {
        let store = self.inner.store.borrow();
        store.data().memory.map_or(0, |m| m.data_size(&*store))
    }

    /// This bundle's interpreter, weakly (see [`crate::remote::engine`]).
    #[doc(hidden)]
    pub fn __engine(&self) -> wasmi::EngineWeak {
        self.inner.store.borrow().engine().weak()
    }
}

// ---------------------------------------------------------------------------
// Remote components: the element codec's link to a bundle
// ---------------------------------------------------------------------------

use runtime_scene::Element;
use runtime_vocabulary::remote::host::{decode, DecodeError, Link};

/// The element codec's [`Link`] to one bundle: decoded closures call the
/// bundle's callback table through it. Routed like the kernel hooks — a call
/// made while the bundle has an import in flight goes through that import's
/// `Caller` — so a host effect can run a bundle getter at any depth.
struct UiLink {
    bundle: u32,
}

impl Link for UiLink {
    fn call(&self, cb: u32, args: &[u8]) -> Option<Vec<u8>> {
        let bundle = self.bundle;
        route(bundle, |c, h| c.ui_invoke(ui_hooks(bundle, h), cb, args))
    }
    fn release(&self, cb: u32) {
        // A bundle already gone took its table with it: nothing to release.
        let bundle = self.bundle;
        route(bundle, |c, h| c.ui_release(ui_hooks(bundle, h), cb));
    }
    fn fail(&self, msg: String) {
        stop(self.bundle, msg);
    }
    fn bundle(&self) -> u64 {
        self.bundle as u64
    }
}

/// Stop bundle `bundle` for sending the app something it can't use: the
/// same end as a panic in it ([`poison`]).
fn stop(bundle: u32, msg: String) {
    if let Some(inner) = BUNDLES.try_with(|b| b.borrow().get(&bundle).and_then(Weak::upgrade)).ok().flatten() {
        poison(&inner, msg);
    }
}

/// Why a remote component did not mount.
#[derive(Debug)]
pub enum MountError {
    /// The bundle panicked building its tree; the bundle's panic message.
    Panicked(String),
    /// The tree it built does not decode here (e.g. it uses an app
    /// component this app does not export).
    Decode(DecodeError),
    /// The bundle has no mount export of that name: a bundle built from
    /// sources that no longer (or do not yet) define the component.
    NoSuchComponent(String),
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::Panicked(m) => write!(f, "the remote component panicked: {m}"),
            MountError::Decode(e) => write!(f, "{e}"),
            MountError::NoSuchComponent(export) => write!(f, "the bundle has no `{export}`"),
        }
    }
}

impl std::error::Error for MountError {}

/// The bundle's last panic message, if it kept one (`idealyst_ui_init`
/// installed its hook).
fn panic_message(ctx: &mut impl AsContextMut<Data = KState>, ui: UiHooks) -> Option<String> {
    let memory = ctx.as_context().data().memory?;
    let packed = ui.panic_message?.call(&mut *ctx, ()).ok()?;
    let msg = String::from_utf8(read_packed(&*ctx, memory, packed).ok()?).ok()?;
    (!msg.is_empty()).then_some(msg)
}

fn ui_hooks(bundle: u32, h: Hooks) -> UiHooks {
    h.ui.unwrap_or_else(|| panic!("remote component: bundle {bundle} exports no element codec (`idealyst_ui_*`)"))
}

impl KernelBundle {
    /// Mount a remote component: call the bundle's mount `export` — shape
    /// `(args_ptr, args_len) -> packed reply` — with `args` (the component's
    /// props, encoded as the export expects), and decode the tree it
    /// returns. Call inside the world the component should live in; realize
    /// the result with the app's own registry.
    ///
    /// A panic in the bundle — while it builds the tree, or at any point
    /// before (a handler, an effect) — is an `Err` carrying the bundle's
    /// panic message, never a host panic: a bundle is code the app did not
    /// compile, and the app shows its failure instead of dying with it. A
    /// panicked bundle is poisoned and every later mount fails the same way.
    /// The panic message that poisoned this bundle, if it has panicked.
    pub fn poisoned(&self) -> Option<String> {
        self.inner.poisoned.borrow().clone()
    }

    /// Run `f` with the panic message when this bundle panics (once). The
    /// remote loader uses it to replace the bundle's components.
    pub fn on_poison(&self, f: impl Fn(&str) + 'static) {
        self.inner.on_poison.borrow_mut().push(Rc::new(f));
    }

    pub fn mount_remote(&self, export: &str, args: &[u8]) -> Result<Element, MountError> {
        if let Some(msg) = self.poisoned() {
            return Err(MountError::Panicked(msg));
        }
        let mount: TypedFunc<(u32, u32), i64> = {
            let store = self.inner.store.borrow();
            self.inner
                .instance
                .get_typed_func(&*store, export)
                .map_err(|_| MountError::NoSuchComponent(export.to_string()))?
        };
        let ui = ui_hooks(self.inner.id, self.inner.hooks);
        let mark = runtime_world::remote::bundle_frames_mark();
        let bytes = {
            let mut store = self.inner.store.try_borrow_mut().unwrap_or_else(|_| {
                panic!("remote component: mount `{export}` re-entered a bundle that is already running")
            });
            let memory = store.data().memory.expect("kernel bridge: bundle exports no memory");
            let called = ui.alloc.call(&mut *store, args.len() as u32).and_then(|ptr| {
                memory
                    .write(&mut *store, ptr as usize, args)
                    .map_err(|_| wasmi::Error::new(format!("remote component: argument buffer {ptr} out of bounds")))?;
                let packed = mount.call(&mut *store, (ptr, args.len() as u32))?;
                read_packed(&*store, memory, packed)
            });
            match called {
                Ok(bytes) => bytes,
                Err(e) => {
                    let msg = panic_message(&mut *store, ui).unwrap_or_else(|| e.to_string());
                    drop(store);
                    runtime_world::remote::unwind_bundle_frames(mark);
                    poison(&self.inner, msg.clone());
                    return Err(MountError::Panicked(msg));
                }
            }
        };
        decode(Rc::new(UiLink { bundle: self.inner.id }), &bytes).map_err(MountError::Decode)
    }
}

// ---------------------------------------------------------------------------
// `#[host_fn]`s a bridged bundle calls
// ---------------------------------------------------------------------------

/// Check every host-function import of `module` against the app's
/// allowlist, and define the ones that pass. Runs before instantiation, so
/// a refused bundle never executes. The import shapes, as the framework's
/// `#[host_fn]` stubs declare them (`runtime_core::host_fn`):
///
/// - sync `(args_ptr, args_len) -> reply_len`: the reply goes into the
///   bundle's argument buffer (`idealyst_ui_alloc`), where its stub reads it;
/// - async `(args_ptr, args_len, then)`: the app's future runs on the app's
///   executor (`runtime_shared::driver::spawn_async`), and its encoded
///   result is delivered by calling the bundle's one-shot callback `then`,
///   which is then released. A bundle that was stopped (it panicked) in the
///   meantime is not called — `route` answers `None`.
fn link_host_fns(module: &Module, host_fns: &[HostFnDef], linker: &mut Linker<KState>) -> Result<(), LoadError> {
    let mut missing = Vec::new();
    let mut mismatched = Vec::new();
    for import in module.imports() {
        if import.module() != HOST_FN_MODULE {
            continue;
        }
        let name = import.name();
        let Some((path, bundle_schema)) = runtime_vocabulary::remote::host_fn::parse_import_name(name) else {
            missing.push(name.to_string());
            continue;
        };
        let Some(def) = host_fns.iter().find(|d| d.path == path) else {
            missing.push(path.to_string());
            continue;
        };
        let ExternType::Func(ty) = import.ty() else {
            missing.push(path.to_string());
            continue;
        };
        let shape_ok = match def.kind {
            HostFnKind::Sync(_) => ty.params() == [ValType::I32, ValType::I32] && ty.results() == [ValType::I64],
            HostFnKind::Async(_) => ty.params() == [ValType::I32, ValType::I32, ValType::I32] && ty.results().is_empty(),
        };
        if def.schema != bundle_schema || !shape_ok {
            mismatched.push(crate::HostFnMismatch { path: path.to_string(), app_schema: def.schema, bundle_schema });
            continue;
        }
        let path: &'static str = def.path;
        let defined = match def.kind {
            HostFnKind::Sync(f) => linker.func_new(HOST_FN_MODULE, name, ty.clone(), move |mut caller, params, results| {
                let args = read_args(&caller, params)?;
                // App code: it may touch the graph, which may call back into
                // this bundle — through this import's `Caller`.
                let reply = with_active(&mut caller, || f(&args)).map_err(wasmi::Error::new)?;
                let alloc = caller
                    .get_export("idealyst_ui_alloc")
                    .and_then(|e| e.into_func())
                    .ok_or_else(|| wasmi::Error::new(format!("host_fn {path}: the bundle exports no idealyst_ui_alloc")))?
                    .typed::<u32, u32>(&caller)?;
                let ptr = alloc.call(&mut caller, reply.len() as u32)?;
                let memory = caller.data().memory.expect("kernel bridge: bundle exports no memory");
                memory
                    .write(&mut caller, ptr as usize, &reply)
                    .map_err(|_| wasmi::Error::new(format!("host_fn {path}: reply buffer out of bounds")))?;
                results[0] = Val::I64(reply.len() as i64);
                Ok(())
            }),
            HostFnKind::Async(f) => linker.func_new(HOST_FN_MODULE, name, ty.clone(), move |mut caller, params, _| {
                let args = read_args(&caller, params)?;
                let then = params[2].i32().unwrap_or(0) as u32;
                let bundle = caller.data().bundle;
                let future = f(args).map_err(wasmi::Error::new)?;
                // An executor may finish the future inside `spawn_async`
                // (pollster, or an already-ready future); the completion
                // then re-enters this bundle through this import's `Caller`.
                with_active(&mut caller, || {
                    runtime_shared::driver::spawn_async(async move {
                        let reply = future.await;
                        route(bundle, |c, h| c.ui_invoke(ui_hooks(bundle, h), then, &reply));
                        route(bundle, |c, h| c.ui_release(ui_hooks(bundle, h), then));
                    })
                });
                Ok(())
            }),
        };
        defined.expect("each host-function import name is unique within a module");
    }
    if !missing.is_empty() {
        return Err(LoadError::MissingHostFunctions(missing));
    }
    if !mismatched.is_empty() {
        return Err(LoadError::IncompatibleHostFunctions(mismatched));
    }
    Ok(())
}

fn read_args(caller: &Caller<'_, KState>, params: &[Val]) -> Result<Vec<u8>, wasmi::Error> {
    let (ptr, len) = (params[0].i32().unwrap_or(0) as u32, params[1].i32().unwrap_or(0) as u32);
    let memory = caller.data().memory.expect("kernel bridge: bundle exports no memory");
    let mut buf = vec![0u8; len as usize];
    memory
        .read(caller, ptr as usize, &mut buf)
        .map_err(|_| wasmi::Error::new(format!("bundle pointer {ptr}+{len} is out of bounds")))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::parse_promotion;

    /// Regression: a malformed promotion reply indexed out of bounds and
    /// panicked the app; now it parses to `None` (and the bundle is
    /// stopped).
    #[test]
    fn regression_a_malformed_promotion_reply_is_refused_not_a_panic() {
        let mut ok = vec![1u8];
        ok.extend_from_slice(&2u32.to_le_bytes());
        ok.extend_from_slice(&[7, 8, 9]);
        assert_eq!(parse_promotion(&ok), Some((1, &[7u8, 8][..], &[9u8][..])));
        for bad in [&[][..], &[1][..], &[1, 0, 0][..], &[1, 9, 0, 0, 0, 1][..]] {
            assert_eq!(parse_promotion(bad), None, "{bad:?}");
        }
    }
}

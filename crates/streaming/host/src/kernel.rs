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

use runtime_world::remote::{EffectClass, GuestHooks, Handle, Host, HostOps, Id, WorldId};
use rustc_hash::FxHashMap;
use wasmi::{AsContextMut, Caller, Engine, Linker, Memory, Module, Store, TypedFunc};

/// The host side as the kernel imports see it.
type H = Host<WasmGuest>;

const MODULE: &str = "idealyst_kernel";

/// Per-store data: which bundle this is, and its memory (for out-buffers).
pub struct KState {
    bundle: u32,
    memory: Option<Memory>,
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
}

struct Inner {
    id: u32,
    store: RefCell<Store<KState>>,
    instance: wasmi::Instance,
    hooks: Hooks,
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
}

impl<C: AsContextMut<Data = KState>> GuestCall for C {
    fn call_i64(&mut self, f: TypedFunc<i64, ()>, arg: i64) -> Result<(), wasmi::Error> {
        f.call(&mut *self, arg)
    }
    fn commit(&mut self, f: TypedFunc<(i64, u32), u32>, value: i64, forced: u32) -> Result<u32, wasmi::Error> {
        f.call(&mut *self, (value, forced))
    }
}

/// Route one hook to bundle `bundle`: through its in-flight import's
/// `Caller` if it has one, else through its `Store`. `None` when the bundle
/// is gone (its proxies outlived it) or the thread is tearing down — the
/// bundle's side is then already destroyed, and there is nothing to call.
fn route<R>(bundle: u32, f: impl FnOnce(&mut dyn GuestCall, Hooks) -> Result<R, wasmi::Error>) -> Option<R> {
    let inner = BUNDLES.try_with(|b| b.borrow().get(&bundle).and_then(Weak::upgrade)).ok().flatten()?;
    let active = ACTIVE.try_with(|a| a.borrow().iter().rev().find(|(b, _)| *b == bundle).map(|(_, p)| *p)).ok().flatten();
    let result = match active {
        Some(ptr) => {
            // SAFETY: `ptr` was published by `with_active` from a live
            // `&mut Caller` whose import is still on the stack (it is popped
            // when that import's body returns), and that import is suspended
            // inside the host call that led here, so nothing else is using
            // the `Caller` for the duration of this call.
            let caller = unsafe { &mut *(ptr as *mut Caller<'static, KState>) };
            f(caller, inner.hooks)
        }
        None => {
            let mut store = inner.store.try_borrow_mut().unwrap_or_else(|_| {
                panic!(
                    "kernel bridge: bundle {bundle}'s store is busy with no import in flight — a \
                     host→bundle call re-entered the bundle without going through an import"
                )
            });
            f(&mut *store, inner.hooks)
        }
    };
    Some(result.unwrap_or_else(|e| panic!("kernel bridge: bundle {bundle} trapped in a kernel hook: {e}")))
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
}

// ---------------------------------------------------------------------------
// The imports
// ---------------------------------------------------------------------------

fn write_u32s(caller: &mut Caller<'_, KState>, ptr: u32, vals: &[u32]) {
    let memory = caller.data().memory.expect("kernel bridge: bundle exports no memory");
    let bytes: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
    memory
        .write(&mut *caller, ptr as usize, &bytes)
        .unwrap_or_else(|_| panic!("kernel bridge: out-buffer {ptr} out of bounds"));
}

fn opt_world(world: i64) -> Option<WorldId> {
    (world >= 0).then_some(world as WorldId)
}

macro_rules! import {
    ($linker:ident, $name:literal, |$caller:ident $(, $arg:ident : $ty:ty)*| $(-> $ret:ty)? $body:block) => {
        $linker
            .func_wrap(MODULE, $name, |mut $caller: Caller<'_, KState> $(, $arg: $ty)*| $(-> $ret)? {
                with_active(&mut $caller, || $body)
            })
            .unwrap_or_else(|e| panic!("define {}: {e}", $name));
    };
}

/// Define every `idealyst_kernel` import on `linker`.
pub fn define_imports(linker: &mut Linker<KState>) {
    import!(linker, "world_new", |c| -> u32 { H::world_new() });
    import!(linker, "world_drop", |c, w: u32| { H::world_drop(w) });
    import!(linker, "world_flush", |c, w: u32| { H::world_flush(w) });
    import!(linker, "world_is_flushing", |c, w: u32| -> u32 { H::world_is_flushing(w) as u32 });
    linker
        .func_wrap(MODULE, "world_provide", |mut c: Caller<'_, KState>, w: u32, key: i64, ctx: i64| {
            let b = c.data().bundle;
            with_active(&mut c, || H::world_provide(w, ns(b, key), ns(b, ctx)))
        })
        .expect("define world_provide");
    linker
        .func_wrap(MODULE, "world_inject", |mut c: Caller<'_, KState>, w: u32, key: i64| -> i64 {
            let b = c.data().bundle;
            with_active(&mut c, || opt(H::world_inject(w, ns(b, key))))
        })
        .expect("define world_inject");
    import!(linker, "enter_push", |c, w: u32| { H::enter_push(w) });
    import!(linker, "enter_pop", |c| { H::enter_pop() });

    import!(linker, "is_flushing", |c| -> u32 { H::is_flushing() as u32 });
    import!(linker, "is_entered", |c| -> u32 { H::is_entered() as u32 });
    import!(linker, "in_effect", |c| -> u32 { H::in_effect() as u32 });
    import!(linker, "effect_depth", |c| -> u32 { H::effect_depth() });
    linker
        .func_wrap(MODULE, "current_effect", |mut c: Caller<'_, KState>, out: u32| -> u32 {
            match with_active(&mut c, H::current_effect) {
                Some((w, s, g)) => {
                    write_u32s(&mut c, out, &[w, s, g]);
                    1
                }
                None => 0,
            }
        })
        .expect("define current_effect");
    import!(linker, "in_collector", |c| -> u32 { H::in_collector() as u32 });

    linker
        .func_wrap(MODULE, "signal_create", |mut c: Caller<'_, KState>, world: i64, value: i64, out: u32| {
            let b = c.data().bundle;
            let ((w, s, g), collected) = with_active(&mut c, || H::signal_create(opt_world(world), ns(b, value)));
            write_u32s(&mut c, out, &[w, s, g, collected as u32]);
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
        .func_wrap(MODULE, "effect_create", |mut c: Caller<'_, KState>, world: i64, class: u32, effect: i64, out: u32| {
            let b = c.data().bundle;
            let class = if class == 0 { EffectClass::Derivation } else { EffectClass::Reaction };
            let (w, s, g): Handle = with_active(&mut c, || H::effect_create(opt_world(world), class, ns(b, effect)));
            write_u32s(&mut c, out, &[w, s, g]);
        })
        .expect("define effect_create");
    import!(linker, "effect_is_alive", |c, w: u32, s: u32, g: u32| -> u32 { H::effect_is_alive((w, s, g)) as u32 });
    linker
        .func_wrap(MODULE, "on_cleanup", |mut c: Caller<'_, KState>, cleanup: i64| {
            let b = c.data().bundle;
            with_active(&mut c, || H::on_cleanup(ns(b, cleanup)))
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
        .func_wrap(MODULE, "ctx_provide", |mut c: Caller<'_, KState>, key: i64, ctx: i64| {
            let b = c.data().bundle;
            with_active(&mut c, || H::ctx_provide(ns(b, key), ns(b, ctx)))
        })
        .expect("define ctx_provide");
    linker
        .func_wrap(MODULE, "ctx_inject", |mut c: Caller<'_, KState>, key: i64| -> i64 {
            let b = c.data().bundle;
            with_active(&mut c, || opt(H::ctx_inject(ns(b, key))))
        })
        .expect("define ctx_inject");
}

// ---------------------------------------------------------------------------
// A loaded bundle
// ---------------------------------------------------------------------------

/// A bundle whose kernel runs on this app's graph.
pub struct KernelBundle {
    inner: Rc<Inner>,
}

impl KernelBundle {
    /// Instantiate `wasm`, which must import only `idealyst_kernel` (plus
    /// whatever `extra` defines) and export the `idealyst_kernel_*` hooks.
    pub fn load(engine: &Engine, wasm: &[u8]) -> Result<KernelBundle, wasmi::Error> {
        let module = Module::new(engine, wasm)?;
        let bundle = NEXT_BUNDLE.with(|n| {
            let id = n.get().checked_add(1).expect("kernel bridge: bundle ids exhausted");
            n.set(id);
            id
        });
        let mut store = Store::new(engine, KState { bundle, memory: None });
        let mut linker = Linker::new(engine);
        define_imports(&mut linker);
        let instance = linker.instantiate_and_start(&mut store, &module)?;
        store.data_mut().memory = instance.get_memory(&store, "memory");
        let hooks = Hooks {
            commit: instance.get_typed_func(&store, "idealyst_kernel_commit")?,
            drop_value: instance.get_typed_func(&store, "idealyst_kernel_drop_value")?,
            run_effect: instance.get_typed_func(&store, "idealyst_kernel_run_effect")?,
            drop_effect: instance.get_typed_func(&store, "idealyst_kernel_drop_effect")?,
            run_cleanup: instance.get_typed_func(&store, "idealyst_kernel_run_cleanup")?,
            drop_cleanup: instance.get_typed_func(&store, "idealyst_kernel_drop_cleanup")?,
            drop_context: instance.get_typed_func(&store, "idealyst_kernel_drop_context")?,
        };
        let inner = Rc::new(Inner { id: bundle, store: RefCell::new(store), instance, hooks });
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

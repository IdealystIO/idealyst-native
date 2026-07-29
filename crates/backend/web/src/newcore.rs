//! New-core adoption for the web backend (idea-lite migration, P3b).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on [`WebBackend`] — the
//! production shape of the migration: no `LegacyBridge` wrapper in the
//! render path. Every trait method delegates via UFCS
//! (`<WebBackend as Backend>::method(self, …)`) to the existing
//! `Backend` impl, so the DOM mechanism code (node creation, style
//! minting, hydration bookkeeping, event closures) is REUSED verbatim —
//! this module adds a second *front* onto the same machinery, exactly
//! like `LegacyBridge` does generically, but as direct impls on the
//! concrete type (no orphan-rule issue: `WebBackend` is local).
//!
//! # Delegation status — every capability accounted for
//!
//! | Trait | Status |
//! |---|---|
//! | `runtime_scene::Host` (7 ops) | direct (`create_anchor` → `create_reactive_anchor`, `supports_splice` → `supports_child_splice` — the P1 renames) |
//! | `AppEnvOps` | direct |
//! | `LifecycleOps` | direct (`is_hydrating` delegates to the Backend impl; `false` under the [`start`] boot, `true` inside the [`hydrate`] adoption window — see *Hydration* below) |
//! | `ViewOps` | direct |
//! | `InputOps` | direct |
//! | `PressableOps` | direct |
//! | `TextOps` | direct |
//! | `ButtonOps` | direct |
//! | `ImageOps` | direct |
//! | `IconOps` | direct |
//! | `LinkOps` | direct |
//! | `TextInputOps` | direct |
//! | `ToggleOps` | direct |
//! | `SliderOps` | direct |
//! | `ActivityIndicatorOps` | direct |
//! | `ScrollOps` | direct |
//! | `SafeAreaOps` | direct |
//! | `VirtualizerOps` | direct |
//! | `GraphicsOps` | direct |
//! | `PortalOps` | direct |
//! | `PresenceOps` | direct |
//! | `NavigatorOps` | direct |
//! | `ExternalOps` | direct |
//! | `DocumentOps` | direct |
//! | `StyleOps` | direct |
//! | `AssetOps` | direct |
//! | `A11yOps` | direct |
//! | `AnimationOps` | direct |
//! | `IntrospectionOps` | direct |
//! | `BatchOps` | direct |
//! | `WireBindingOps` | direct (wire-recorder no-ops on this backend, same as today) |
//!
//! **30/30 direct, 0 adapted, 0 stubbed.** Nothing panics, nothing
//! silently no-ops beyond what the wrapped `Backend` impl already does.
//! Where a `Backend` method is itself feature-gated on this crate
//! (`prim-*` families), the UFCS call resolves to the same
//! trait-default fallback the old walker would hit — behavior is
//! identical by construction. The impl bodies below are generated
//! mechanically from `runtime_vocabulary::bridge` (the compile-time
//! proof of the signature freeze), with `LegacyBridge<B>` → `WebBackend`
//! and `&mut self.0` → `self`.
//!
//! # Boot path — [`start`] / [`start_in`]
//!
//! Client-render-only mount of a `runtime_scene::Element` tree against
//! the real DOM through the registry: create the backend + `Registry`
//! (`register_builtins` + an app-registration seam) + a [`World`],
//! `world.enter(realize)`, hand the single root node to
//! `Backend::finish` (which clears `#app` and appends — same as the
//! old-core mount), then install the flush driver and retain everything
//! in a thread-local for the page's lifetime (the same
//! `OWNER`-thread-local convention the CLI-generated wrappers use).
//!
//! **Hydration lives in its own boot path** ([`hydrate`] / [`hydrate_in`],
//! `newcore_hydrate.rs`): `start` always constructs via `WebBackend::new`,
//! so `is_hydrating()` stays on the false path and SSR DOM (if any) is
//! replaced, not adopted. `hydrate` constructs via `WebBackend::hydrate`
//! and adopts the server DOM node-for-node.
//!
//! # Flush driver (design §3: web = microtask after event dispatch)
//!
//! The new kernel stages writes; nothing is observable until the host
//! driver calls [`World::flush`]. The driver is **precise dispatch-site
//! glue** — every place the backend invokes author code triggers one
//! deduped flush microtask AFTER the author callback returns. There is
//! no window-level event listener and no per-frame rAF poll.
//!
//! 1. **Author-callback wrapping (this module).** Every callback-taking
//!    capability impl below wraps the author callback before delegating
//!    to the `Backend` machinery: press/click, input/change, toggle,
//!    slider, scroll, hover, wheel, touch, key, blur/focus, file-drop,
//!    image load/error, link activation, portal dismiss, graphics
//!    lifecycle, virtualizer row mount/release, state setters, and the
//!    app-level key handler. The wrapper calls the author fn, then
//!    [`schedule_flush`] — one deduped
//!    `runtime_core::scheduling::schedule_microtask` → `world.flush()`.
//!    Net effect: stage during dispatch, commit in the microtask
//!    checkpoint right after — the idea-lite contract. Because the
//!    wrapping happens in these new-core-only impls, the shared
//!    old-core event closures are reused verbatim and the old core
//!    never pays for it.
//! 2. **Post-dispatch hook ([`crate::dispatch_hook`]).** Author code
//!    also runs from non-DOM surfaces: `after_ms` timers,
//!    `after_animation_frame` one-shots, `raf_loop` iterations, and
//!    executor-spawned future polls. The scheduler and async executor
//!    fire a thread-local hook after each such callback; [`start_in`]
//!    installs [`schedule_flush`] into that slot (no-op default, so the
//!    old core is untouched). This REPLACES the former rAF-poll safety
//!    net — no idle per-frame wakeup.
//!
//! Residual surfaces NOT covered (documented, not silent): DOM closures
//! installed by `Element::External` third-party glue (the External web
//! registry predates the new core; its port must call
//! [`schedule_flush`] after author callbacks). Browser-back / popstate
//! is covered now: the navigator URL-sync port (`newcore_url_sync`,
//! installed by [`start_in`]) fires [`schedule_flush`] after staging
//! reconciled nav commands. Raw `wasm_bindgen_futures::spawn_local`
//! calls that bypass `runtime_core::driver::spawn` are the app's own
//! responsibility.
//!
//! Everything funnels through [`schedule_flush`]/`flush_now`, which
//! skips re-entrant flushes (`world.is_flushing()`) — belt and braces;
//! microtasks can't actually preempt a synchronous flush.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::accessibility::{AccessibilityProps, AccessibilityTree, LiveRegionPriority, Role};
use runtime_core::animation::AnimProp;
use runtime_core::assets::{
    AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
};
use runtime_core::breakpoint::Breakpoint;
use runtime_core::introspect::NativeNode;
use runtime_core::primitives;
use runtime_core::primitives::portal::ViewportRect;
use runtime_core::styled_text::TextRun;
use runtime_core::{
    Action, Backend, BackendBatch, Color, ColorScheme, Easing, FileDropHandler, FontFamily,
    HoverHandler, ImageErrorHandler, ImageLoadHandler, PageMetadata, Platform, SafeAreaSides,
    Screenshot, StateBits, StyleApplication, StyleRules, TokenEntry, Tokenized, TouchHandler,
    TouchId, VirtualizerCallbacks, WheelHandler,
};
use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;

use crate::WebBackend;

// Hydration (adopt-mode) boot — `newcore::hydrate` / `newcore::hydrate_in`.
// Child module so it can share this module's private boot plumbing
// (`App`, `APP`, `FLUSH_WORLD`, `schedule_flush`); `#[path]` keeps the
// file flat in `src/` alongside this one.
#[path = "newcore_hydrate.rs"]
mod newcore_hydrate;
pub use newcore_hydrate::{hydrate, hydrate_in};

// ===========================================================================
// Boot path
// ===========================================================================

thread_local! {
    /// The mounted new-core app: dropping this tears everything down
    /// (Realized first — unmount — then the World). Page-lifetime, same
    /// retention convention as the CLI wrapper's `OWNER` slot.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// The installed window-`resize` listener (viewport source), kept so
    /// [`stop`] can remove it — unlike the old-core observer's
    /// `forget()` leak, repeated boot/stop cycles (tests) must not pile
    /// listeners. The closure captures the mounted world's viewport
    /// signal HANDLE only; after `stop` a straggler event would stage
    /// into a dead world (silent kernel no-op), so removal is hygiene,
    /// not correctness.
    static VIEWPORT_SOURCE: RefCell<Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>>> =
        const { RefCell::new(None) };
    /// The world the flush driver commits. Separate from `APP` so
    /// [`schedule_flush`] never touches the app slot (a flush can run
    /// while `APP` is being written during `start_in`).
    static FLUSH_WORLD: RefCell<Option<World>> = const { RefCell::new(None) };
    /// Dedup flag: one queued flush microtask at a time.
    static FLUSH_QUEUED: Cell<bool> = const { Cell::new(false) };
}

/// Everything the boot path must keep alive. Field order is drop order:
/// the realized tree unmounts before the world (its slots' owner) dies.
struct App {
    realized: Realized<web_sys::Node>,
    _backend: Rc<RefCell<WebBackend>>,
    _registry: Rc<Registry<WebBackend>>,
    world: World,
}

/// Mount a new-core element tree into `#app`. Client-render-only (no
/// hydration — see the module docs). The build closure runs inside
/// `world.enter`, so free `signal()`/`effect()` calls work; top-level
/// creations are world-root-owned (they live until page teardown).
pub fn start(build: impl FnOnce() -> Element) {
    start_in("#app", |_| {}, build)
}

/// [`start`] with an explicit mount selector and a registration seam:
/// `register` runs after [`runtime_vocabulary::register_builtins`], so
/// apps/SDKs can register their own payload handlers on the same
/// registry before the tree realizes.
pub fn start_in(
    selector: &str,
    register: impl FnOnce(&mut Registry<WebBackend>),
    build: impl FnOnce() -> Element,
) {
    // Same idempotent installs the CLI wrapper performs — the flush
    // driver below rides the scheduler, so it must exist first.
    crate::install_scheduler();
    crate::install_time_source();
    // URL sync service for the vocabulary navigator handlers (before
    // the build, so every navigator registers) — see newcore_url_sync.
    #[cfg(feature = "prim-navigator")]
    crate::newcore_url_sync::install();
    // Route `runtime_core::driver::spawn` futures through the hooked
    // executor so future polls fire the post-dispatch flush hook.
    #[cfg(feature = "async-driver")]
    crate::install_async_executor();

    let backend = Rc::new(RefCell::new(WebBackend::new(selector)));
    // Animation writes + batched-text microtasks need the global weak
    // self-handle, exactly as on the old-core path.
    crate::install_global_self(&backend);

    let mut registry: Registry<WebBackend> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    // Seed the SHARED old-core viewport value from the real window
    // BEFORE the build — the world's per-world `ViewportCtx` (created
    // below) seeds from it, so the first build classifies the correct
    // breakpoint instead of 0-width `Xs`. (Hydrate boots deliberately
    // seed the SSR viewport instead — see `newcore_hydrate`.)
    if let Some(size) = current_window_viewport() {
        runtime_core::set_viewport_size(size);
    }

    let world = World::new();
    let (vp_sig, realized) = world.enter(|| {
        let element = build();
        let realized = realize(&backend, &registry, element);
        // Capture the per-world viewport ctx AFTER the build, never
        // before: a build that reads a breakpoint creates the ctx
        // itself mid-build, AFTER the app's `install_breakpoints` runs
        // (the docs app installs its custom Lg=900 table inside its
        // root component) — the ctx's derived-bucket memo captures the
        // threshold table at creation, so an eager pre-build creation
        // would pin the DEFAULT table and misclassify every width the
        // two tables disagree on. Post-build this either fetches the
        // build's ctx or creates one for apps that never read
        // breakpoints (matching the old core's capture-on-first-read).
        let vp_sig = runtime_vocabulary::viewport::viewport_ctx().size_signal();
        (vp_sig, realized)
    });

    // Single-root contract, matching the old-core mount: `finish` clears
    // any prior `#app` contents and appends the live root.
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_web::newcore::start: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    Backend::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount (ref-fill callbacks, handler
    // setup) before the first paint.
    world.flush();

    // Install the flush driver: schedule_flush becomes reachable from
    // (a) the author-callback wrappers in the caps impls below and
    // (b) the scheduler/executor post-dispatch hook.
    crate::dispatch_hook::install_dispatch_hook(schedule_flush);
    FLUSH_WORLD.with(|w| *w.borrow_mut() = Some(world.clone()));
    APP.with(|slot| {
        *slot.borrow_mut() = Some(App {
            realized,
            _backend: backend,
            _registry: registry,
            world,
        })
    });
    // Robot driver env: vocabulary Robot queries enter this world,
    // actions settle via flush_sync (see robot_transport).
    #[cfg(feature = "robot")]
    crate::robot_transport::install_newcore_driver_env();

    // Live viewport source: window resizes re-fire breakpoint-dependent
    // author reactivity (the idea-ui-docs hamburger bug).
    install_viewport_source(vp_sig);
}

/// True while a new-core app is mounted (`start` ran, `stop` hasn't).
/// Core-selection probe for shared transports (robot relay).
pub fn is_booted() -> bool {
    FLUSH_WORLD.with(|w| w.borrow().is_some())
}

/// Borrow the mounted app's live tree (tests, diagnostics).
pub fn with_realized<R>(f: impl FnOnce(&Realized<web_sys::Node>) -> R) -> Option<R> {
    APP.with(|slot| slot.borrow().as_ref().map(|app| f(&app.realized)))
}

/// The world mounted by [`start`]/[`start_in`]/[`hydrate`] (a cheap
/// handle clone; `None` before boot / after [`stop`]).
///
/// Host-integration seam: an embedded renderer mounted INSIDE this
/// page's app — the wgpu simulator preview (`host_web::mount_newcore`)
/// — realizes its scene into this SAME world, so the page's existing
/// flush driver (dispatch-site glue + the scheduler/executor
/// post-dispatch hook) commits the embedded app's staged writes with
/// no second driver: one thread, one world, one logical update stream.
pub fn mounted_world() -> Option<World> {
    FLUSH_WORLD.with(|w| w.borrow().clone())
}

/// Run `f` with the mounted app's world ambient (`World::enter`).
/// JS-interop seam: wasm-bindgen exports that must CREATE reactive
/// state (`signal()`, `memo()`) run outside any handler/effect, where
/// no world is ambient — creation would panic. Reads and writes on
/// existing handles do NOT need this (handles route to their own
/// world). `None` before [`start`].
pub fn with_world_entered<R>(f: impl FnOnce() -> R) -> Option<R> {
    let world = FLUSH_WORLD.with(|w| w.borrow().clone());
    world.map(|w| w.enter(f))
}

/// Synchronously commit staged writes (skipped mid-flush; no-op before
/// [`start`]). JS-interop seam: a wasm-bindgen export that staged
/// writes and must return with the tree updated (bench drivers, robot
/// verbs) cannot ride the async microtask the dispatch-site glue
/// queues — it flushes before returning instead, exactly like an
/// old-core export whose `set()` applied synchronously.
pub fn flush_sync() {
    flush_now();
}

/// Unmount the app started by [`start`]/[`start_in`]: drops the
/// `Realized` (cleanups fire, DOM detaches from the live tree's point of
/// view), uninstalls the flush hook, and drops the world. Primarily for
/// tests.
pub fn stop() {
    #[cfg(feature = "robot")]
    crate::robot_transport::clear_newcore_driver_env();
    #[cfg(feature = "prim-navigator")]
    crate::newcore_url_sync::reset();
    remove_viewport_source();
    crate::dispatch_hook::clear_dispatch_hook();
    FLUSH_WORLD.with(|w| *w.borrow_mut() = None);
    APP.with(|slot| {
        if let Some(app) = slot.borrow_mut().take() {
            // Explicit for readability; struct field order guarantees
            // the same sequence.
            let App { realized, _backend, _registry, world } = app;
            drop(realized);
            drop(world);
        }
    });
}

// ===========================================================================
// JS sid namespace fold
// ===========================================================================

/// Fold a world signal's `raw_id` into the JS-side sid namespace,
/// DISJOINT from old-core arena ids: keep the slot (low 31 bits — the
/// per-live-world uniqueness the JS tables need) and set the high bit.
///
/// WHY (bug: cross-core sid aliasing poisons an old-core binding's
/// first paint): the JS binding tables (`__idealystSignalValues` /
/// `__idealystSignalSubscribers`, keyed u32) are PAGE-global and shared
/// by both cores. World `raw_id`s were previously truncated to their
/// low-32 slot, and old-core arena signal ids are the same small
/// integers — so a still-mounted new-core app's live subscriber could
/// hold a cached value at the exact sid a later old-core signal
/// allocates (the browser battery's
/// `regression_fstring_two_bindings_one_signal` painted a sibling
/// test's cached value on first paint once an unrelated boot-order
/// change shifted old-arena ids by one). The high bit keeps the two
/// cores' key spaces disjoint: old arena ids are sequential slab
/// indices that never reach 2^31. EVERY new-core path that hands a sid
/// to the JS layer must fold through here — registration
/// (`register_reactive_text_binding` / `register_reactive_class_binding`)
/// and delivery (`notify_signal_text_js` / `notify_signal_value_js`)
/// alike, or commits ship to a key nothing subscribed.
///
/// Two live worlds sharing a slot still alias each other (same caveat
/// the plain low-32 truncation documented: one interactive world per
/// page); dropping the generation bits is likewise unchanged.
fn js_sid(raw_id: u64) -> u64 {
    (raw_id & 0x7FFF_FFFF) | 0x8000_0000
}

// ===========================================================================
// Viewport source (the new-core web resize seam)
// ===========================================================================
//
// The vocabulary's per-world viewport/breakpoint signal
// (`runtime_vocabulary::viewport`) is SEED-ONLY unless the platform
// pushes live sizes. This is web's push: a window-`resize` listener that
// writes BOTH sinks —
//
// - the world's `ViewportCtx` signal (handler-safe staged write through
//   the boot-captured Copy handle; the listener is a raw DOM callback,
//   OUTSIDE `World::enter` — capture, don't inject) so breakpoint-
//   dependent author reactivity (`when(!sidebar_pinned(Lg))`) re-fires
//   on the flush it schedules;
// - the shared old-core TLS value (`runtime_core::set_viewport_size`) so
//   every value-read seam (the vocabulary's native breakpoint-overlay
//   merge, `Tokenized` apply paths, a later hydrate's seed) stays
//   coherent with what author reactivity sees.
//
// Mirrors `viewport_observer.rs` (the old-core source) but is
// removable: `stop()` uninstalls it, so boot/stop cycles in the browser
// test battery don't accumulate listeners.

/// `window.inner{Width,Height}` as a logical-pixel [`ViewportSize`].
/// `None` outside a browser context (workers) — degrade like the
/// old-core observer.
fn current_window_viewport() -> Option<runtime_core::ViewportSize> {
    let win = web_sys::window()?;
    let w = win.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let h = win.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    Some(runtime_core::ViewportSize::new(w, h))
}

/// One resize push: both sinks + a scheduled flush (the staged write
/// commits like any handler-staged write).
fn push_viewport(sig: runtime_world::Signal<runtime_core::ViewportSize>) {
    if let Some(size) = current_window_viewport() {
        runtime_core::set_viewport_size(size);
        // Equality-guarded staged write on the world handle; dead-world
        // writes (a straggler event after `stop`) are silent no-ops.
        sig.set(size);
        schedule_flush();
    }
}

/// Install the `resize` listener for the mounted world. Called at the
/// end of every boot path (`start_in` AND `newcore_hydrate`); replaces
/// any previous listener (idempotent across re-boots).
pub(crate) fn install_viewport_source(
    sig: runtime_world::Signal<runtime_core::ViewportSize>,
) {
    remove_viewport_source();
    let Some(win) = web_sys::window() else { return };
    let closure: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)> =
        wasm_bindgen::closure::Closure::new(move |_: web_sys::Event| push_viewport(sig));
    use wasm_bindgen::JsCast;
    let _ = win.add_event_listener_with_callback("resize", closure.as_ref().unchecked_ref());
    VIEWPORT_SOURCE.with(|s| *s.borrow_mut() = Some(closure));
}

/// Push the REAL window size through the source once — the hydrate
/// boot's post-adoption reconcile (its build seeded the SSR viewport;
/// the real one lands after the server DOM is adopted, mirroring the
/// old-core "observer installed post-mount" ordering).
pub(crate) fn push_current_viewport_now(
    sig: runtime_world::Signal<runtime_core::ViewportSize>,
) {
    push_viewport(sig);
}

fn remove_viewport_source() {
    VIEWPORT_SOURCE.with(|s| {
        if let Some(closure) = s.borrow_mut().take() {
            if let Some(win) = web_sys::window() {
                use wasm_bindgen::JsCast;
                let _ = win.remove_event_listener_with_callback(
                    "resize",
                    closure.as_ref().unchecked_ref(),
                );
            }
        }
    });
}

// ===========================================================================
// Flush driver
// ===========================================================================

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// Event glue and future new-core wrappers call this right after
/// author-visible dispatch.
pub fn schedule_flush() {
    if FLUSH_QUEUED.with(|q| q.replace(true)) {
        return;
    }
    runtime_core::scheduling::schedule_microtask(|| {
        FLUSH_QUEUED.with(|q| q.set(false));
        flush_now();
    });
}

/// Flush the mounted world immediately (skipped while it is already
/// mid-flush).
fn flush_now() {
    let world = FLUSH_WORLD.with(|w| w.borrow().clone());
    if let Some(world) = world {
        if !world.is_flushing() {
            let _t = crate::phase_timer::PhaseTimer::start("nc_flush_total");
            world.flush();
        }
    }
}

/// Run a platform-invoked vocabulary callback with the mounted world
/// ambient (`World::enter`).
///
/// WHY (bug: flat_list rendered ZERO rows on new-core web): the
/// virtualizer inverts control — the backend's own scroll/window
/// machinery calls the handler's `mount_item` to REALIZE a row, and
/// realization is creation-side (`signal()`/`effect()`/`inject` for
/// `ThemeCtx`), which panics outside `World::enter`. Ordinary author
/// callbacks (`on_press`, …) only stage writes through captured
/// handles, so the dispatch-site glue never needed entry — `mount_item`
/// / `release_item` are the one callback family that BUILDS, and the
/// vocabulary contract (handlers/virtualizer.rs) assigns the entry to
/// the backend. Pre-boot (`FLUSH_WORLD` empty — the initial mount's
/// realize) the boot's own `enter` is still ambient, so a bare call is
/// already entered; nesting `enter` is a legal stack, so the ambient
/// fallback never double-books.
fn enter_mounted_world<R>(f: impl FnOnce() -> R) -> R {
    match FLUSH_WORLD.with(|w| w.borrow().clone()) {
        Some(world) => world.enter(f),
        None => f(),
    }
}

// ---------------------------------------------------------------------------
// Dispatch-site glue: author-callback wrappers
// ---------------------------------------------------------------------------
//
// Each helper wraps an author callback so that, AFTER the author code
// returns, one deduped flush microtask is queued. These are used by the
// callback-taking caps impls below — the precise dispatch-site glue the
// flush driver is built on (module docs, "Flush driver"). Wrapping here
// (instead of inside the shared `Backend` event closures) keeps the
// old-core render path byte-identical: the old core applies writes
// synchronously and must not pay a flush per event.

/// Wrap a zero-arg author callback (`on_press`, `on_dismiss`,
/// `on_activate`, image error, …).
fn flushing0(f: Rc<dyn Fn()>) -> Rc<dyn Fn()> {
    Rc::new(move || {
        f();
        schedule_flush();
    })
}

/// Wrap a one-value author callback (`on_change(String/bool/f32)`,
/// hover, focus …).
fn flushing1<A: 'static>(f: Rc<dyn Fn(A)>) -> Rc<dyn Fn(A)> {
    Rc::new(move |a| {
        f(a);
        schedule_flush();
    })
}

/// Wrap a key handler (`&KeyEvent -> KeyOutcome`; outcome passes
/// through so the backend's preventDefault decision is unchanged).
fn flushing_key(f: primitives::key::KeyDownHandler) -> primitives::key::KeyDownHandler {
    Rc::new(move |ev| {
        let outcome = f(ev);
        schedule_flush();
        outcome
    })
}

// ===========================================================================
// Host + capability-trait delegation (generated from
// runtime_vocabulary::bridge — keep mechanically in sync; the scene-parity
// goldens + the AllCaps bound on register_builtins are the compile gates)
// ===========================================================================

// ---------------------------------------------------------------------------
// Host — the P1 structural seam
// ---------------------------------------------------------------------------

impl Host for WebBackend {
    type Node = web_sys::Node;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let _t = crate::phase_timer::PhaseTimer::start("nc_insert");
        <WebBackend as Backend>::insert(self, parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        let _t = crate::phase_timer::PhaseTimer::start("nc_insert_many");
        <WebBackend as Backend>::insert_many(self, parent, children)
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        <WebBackend as Backend>::insert_at(self, parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        let _t = crate::phase_timer::PhaseTimer::start("nc_remove_child");
        <WebBackend as Backend>::remove_child(self, parent, child)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        let _t = crate::phase_timer::PhaseTimer::start("nc_clear_children");
        <WebBackend as Backend>::clear_children(self, node)
    }

    fn create_anchor(&mut self) -> Self::Node {
        <WebBackend as Backend>::create_reactive_anchor(self)
    }

    fn supports_splice(&self) -> bool {
        // Hydration gate — the new-core equivalent of the old walker's
        // `!is_hydrating()` guard on the spliced `When`/`Switch` arms
        // (`walker/view.rs`): SSR renders with
        // `supports_child_splice() == false`, so the server DOM nests
        // every reactive region under a `display:contents` anchor.
        // Splicing during adoption would consume the SSR *anchor* as the
        // region's first content node — off by one tree level — and every
        // following sibling would mismatch (the `[hydrate] diverge`
        // remount cascade). Queried per region mount, so regions first
        // built after `finish` (which clears `is_hydrating`) splice as
        // usual. See `newcore_hydrate.rs`.
        !<WebBackend as Backend>::is_hydrating(self)
            && <WebBackend as Backend>::supports_child_splice(self)
    }
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

impl caps::AppEnvOps for WebBackend {
    fn color_scheme(&self) -> ColorScheme {
        <WebBackend as Backend>::color_scheme(self)
    }

    fn platform(&self) -> Platform {
        <WebBackend as Backend>::platform(self)
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        <WebBackend as Backend>::url_opener(self)
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        <WebBackend as Backend>::fullscreen_setter(self)
    }

    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        <WebBackend as Backend>::set_page_metadata(self, meta)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        <WebBackend as Backend>::set_app_background(self, color)
    }

    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
        <WebBackend as Backend>::set_scrollbar_theme(self, thumb, track)
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        // Dispatch-site glue: app-level key handlers run author code.
        let handler = handler.map(|f| -> primitives::key::KeyDownHandler {
            Rc::new(move |ev| {
                let outcome = f(ev);
                schedule_flush();
                outcome
            })
        });
        <WebBackend as Backend>::set_app_key_handler(self, handler)
    }
}

impl caps::LifecycleOps for WebBackend {
    fn finish(&mut self, root: Self::Node) {
        <WebBackend as Backend>::finish(self, root)
    }

    fn run_layout(&mut self) {
        <WebBackend as Backend>::run_layout(self)
    }

    fn schedule_layout_pass() {
        <WebBackend as Backend>::schedule_layout_pass()
    }

    fn is_hydrating(&self) -> bool {
        <WebBackend as Backend>::is_hydrating(self)
    }

    fn renders_lazy_chunks(&self) -> bool {
        <WebBackend as Backend>::renders_lazy_chunks(self)
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for WebBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        let _t = crate::phase_timer::PhaseTimer::start("nc_create_view");
        <WebBackend as Backend>::create_view(self, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        <WebBackend as Backend>::make_view_handle(self, node)
    }
}

impl caps::InputOps for WebBackend {
    fn install_touch_handler(&mut self, node: &Self::Node, handler: TouchHandler) {
        // Dispatch-site glue (module docs): flush after author code.
        let handler: TouchHandler = {
            let f = handler;
            Rc::new(move |ev| {
                let response = f(ev);
                schedule_flush();
                response
            })
        };
        <WebBackend as Backend>::install_touch_handler(self, node, handler)
    }

    fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
        <WebBackend as Backend>::claim_touch(self, node, touch_id)
    }

    fn install_wheel_handler(&mut self, node: &Self::Node, handler: WheelHandler) {
        let handler: WheelHandler = {
            let f = handler;
            Rc::new(move |ev| {
                let response = f(ev);
                schedule_flush();
                response
            })
        };
        <WebBackend as Backend>::install_wheel_handler(self, node, handler)
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        <WebBackend as Backend>::install_hover_handler(self, node, flushing1(handler))
    }

    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        <WebBackend as Backend>::mark_preserves_focus(self, node)
    }

    fn install_file_drop_handler(&mut self, node: &Self::Node, handler: FileDropHandler) {
        let handler: FileDropHandler = {
            let f = handler;
            Rc::new(move |ev| {
                let response = f(ev);
                schedule_flush();
                response
            })
        };
        <WebBackend as Backend>::install_file_drop_handler(self, node, handler)
    }
}

impl caps::PressableOps for WebBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        <WebBackend as Backend>::create_pressable(self, flushing0(on_click), a11y)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
        <WebBackend as Backend>::make_pressable_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for WebBackend {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
        let _t = crate::phase_timer::PhaseTimer::start("nc_create_text");
        <WebBackend as Backend>::create_text(self, content, a11y)
    }

    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
        <WebBackend as Backend>::create_styled_text(self, runs, a11y)
    }

    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
        <WebBackend as Backend>::update_styled_text(self, node, runs)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        <WebBackend as Backend>::update_text(self, node, content)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
        <WebBackend as Backend>::create_text_with_id(self, content, a11y)
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        <WebBackend as Backend>::update_text_by_id(self, id, content)
    }

    fn release_text_id(&mut self, id: u32) {
        <WebBackend as Backend>::release_text_id(self, id)
    }

    fn supports_js_text_bindings(&self) -> bool {
        <WebBackend as Backend>::supports_js_text_bindings(self)
    }

    fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[Rc<dyn Fn() -> String>],
    ) {
        // Fold into the new-core half of the JS sid namespace — see
        // [`js_sid`]; must match the `notify_signal_text_js` delivery
        // fold or commits go to a key no binding subscribed.
        let folded: Vec<u64> = signal_ids.iter().map(|id| js_sid(*id)).collect();
        <WebBackend as Backend>::register_reactive_text_binding(
            self,
            text_id,
            &folded,
            template_parts,
            initial_values,
            stringifiers,
        )
    }

    fn release_reactive_text_binding(&mut self, text_id: u32) {
        <WebBackend as Backend>::release_reactive_text_binding(self, text_id)
    }

    /// New-core-only channel (string sibling of
    /// `StyleOps::notify_signal_value_js`): the vocabulary's per-signal
    /// TEXT notifier effect ships committed values here — world signals
    /// have no `Signal::set` hook to fire the old-core notifier closure
    /// `register_reactive_text_binding` auto-installs, so this is the
    /// delivery path that makes JS text bindings live on the new core.
    fn notify_signal_text_js(&mut self, signal_id: u64, value: &str) {
        // Ensure the dispatcher exists FIRST: the notifier's seeding
        // first-run happens BEFORE the first
        // `register_reactive_text_binding` (which is what normally
        // injects `text_bindings.js`), and shipping into a missing
        // `__idealystOnSignalChanged` panics (same rationale as
        // `notify_signal_value_js`'s ensure_class_bindings_shim).
        self.ensure_text_bindings_shim();
        self.ship_signal_change_to_js(js_sid(signal_id), value);
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        <WebBackend as Backend>::make_text_handle(self, node)
    }
}

impl caps::ButtonOps for WebBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: wrap the Action's runtime evaluator; the
        // serialization metadata passes through untouched.
        let on_click = Action {
            method: on_click.method,
            inputs: on_click.inputs.clone(),
            initial: on_click.initial.clone(),
            output: on_click.output,
            fire: flushing0(on_click.fire.clone()),
        };
        <WebBackend as Backend>::create_button(self, label, &on_click, leading_icon, trailing_icon, a11y)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        <WebBackend as Backend>::update_button_label(self, node, label)
    }

    fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
        <WebBackend as Backend>::make_button_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for WebBackend {
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node {
        <WebBackend as Backend>::create_image(self, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        <WebBackend as Backend>::update_image_src(self, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        <WebBackend as Backend>::update_image_alt(self, node, alt)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        let handler: ImageLoadHandler = {
            let f = handler;
            Rc::new(move |ev| {
                f(ev);
                schedule_flush();
            })
        };
        <WebBackend as Backend>::install_image_load_handler(self, node, handler)
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        <WebBackend as Backend>::install_image_error_handler(self, node, flushing0(handler))
    }

    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        <WebBackend as Backend>::make_image_handle(self, node)
    }
}

impl caps::IconOps for WebBackend {
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WebBackend as Backend>::create_icon(self, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        <WebBackend as Backend>::update_icon_color(self, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
        <WebBackend as Backend>::update_icon_data(self, node, data)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        <WebBackend as Backend>::update_icon_stroke(self, node, progress)
    }

    fn animate_icon_stroke(
        &mut self,
        node: &Self::Node,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: Easing,
        infinite: bool,
        autoreverses: bool,
    ) {
        <WebBackend as Backend>::animate_icon_stroke(
            self,
            node,
            from,
            to,
            duration_ms,
            easing,
            infinite,
            autoreverses,
        )
    }

    fn make_icon_handle(&self, node: &Self::Node) -> primitives::icon::IconHandle {
        <WebBackend as Backend>::make_icon_handle(self, node)
    }
}

impl caps::LinkOps for WebBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: link activation dispatches navigation
        // (stages nav-queue tick signals on the new core).
        let mut config = config;
        config.on_activate = flushing0(config.on_activate.clone());
        <WebBackend as Backend>::create_link(self, config, a11y)
    }

    fn update_link_url(&mut self, node: &Self::Node, url: &str) {
        <WebBackend as Backend>::update_link_url(self, node, url)
    }

    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        <WebBackend as Backend>::make_link_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for WebBackend {
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WebBackend as Backend>::create_text_input(
            self,
            initial_value,
            placeholder,
            flushing1(on_change),
            on_key_down.map(flushing_key),
            on_blur.map(|f| -> primitives::text_input::BlurHandler {
                Rc::new(move || {
                    let outcome = f();
                    schedule_flush();
                    outcome
                })
            }),
            secure,
            a11y,
        )
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        <WebBackend as Backend>::update_text_input_value(self, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        <WebBackend as Backend>::update_text_input_secure(self, node, secure)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
        <WebBackend as Backend>::set_text_input_focus_handler(self, node, flushing1(handler))
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        <WebBackend as Backend>::update_text_input_placeholder(self, node, placeholder)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        wrap: bool,
        min_rows: Option<u32>,
        max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WebBackend as Backend>::create_text_area(
            self,
            initial_value,
            placeholder,
            wrap,
            min_rows,
            max_rows,
            flushing1(on_change),
            on_key_down.map(flushing_key),
            a11y,
        )
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        <WebBackend as Backend>::update_text_area_value(self, node, value)
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        <WebBackend as Backend>::make_text_input_handle(self, node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        <WebBackend as Backend>::make_text_area_handle(self, node)
    }
}

impl caps::ToggleOps for WebBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WebBackend as Backend>::create_toggle(self, initial_value, flushing1(on_change), a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        <WebBackend as Backend>::update_toggle_value(self, node, value)
    }

    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        <WebBackend as Backend>::make_toggle_handle(self, node)
    }
}

impl caps::SliderOps for WebBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WebBackend as Backend>::create_slider(
            self,
            initial_value,
            min,
            max,
            step,
            flushing1(on_change),
            a11y,
        )
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        <WebBackend as Backend>::update_slider_value(self, node, value)
    }

    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        <WebBackend as Backend>::make_slider_handle(self, node)
    }
}

impl caps::ActivityIndicatorOps for WebBackend {
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WebBackend as Backend>::create_activity_indicator(self, size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        <WebBackend as Backend>::update_activity_indicator_size(self, node, size)
    }

    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        <WebBackend as Backend>::make_activity_indicator_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for WebBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: on_scroll fires per scroll event; the
        // flush microtask is deduped so a burst costs one commit.
        let on_scroll = on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
            Rc::new(move |x, y| {
                f(x, y);
                schedule_flush();
            })
        });
        <WebBackend as Backend>::create_scroll_view(self, horizontal, on_scroll, a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        <WebBackend as Backend>::node_scroll(self, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        <WebBackend as Backend>::set_node_scroll(self, node, x, y)
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        <WebBackend as Backend>::make_scroll_view_handle(self, node)
    }
}

impl caps::SafeAreaOps for WebBackend {
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <WebBackend as Backend>::apply_safe_area_padding(self, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <WebBackend as Backend>::apply_scroll_view_safe_area_inset(self, node, sides)
    }
}

impl caps::VirtualizerOps for WebBackend {
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: mount/release run author render closures
        // and scope cleanups (which may stage writes) from the
        // backend's own scroll handling; measured-size reports feed the
        // handler's layout cache. item_count/item_key/item_size are
        // pure reads and stay unwrapped.
        //
        // mount/release additionally run WORLD-ENTERED
        // (`enter_mounted_world`): `mount_item` realizes the row —
        // creation-side work (`theme_ctx` → `inject::<ThemeCtx>`) that
        // aborts outside `World::enter` (the flat_list-renders-zero-rows
        // bug); `release_item` drops the row scope, whose cleanups get
        // the same ambient guarantee the old walker's teardown had.
        let VirtualizerCallbacks {
            item_count,
            item_key,
            item_size,
            measure_sizes,
            mount_item,
            release_item,
            set_measured_size,
        } = callbacks;
        let callbacks = VirtualizerCallbacks {
            item_count,
            item_key,
            item_size,
            measure_sizes,
            mount_item: {
                let f = mount_item;
                Rc::new(move |i| {
                    let mounted = enter_mounted_world(|| f(i));
                    schedule_flush();
                    mounted
                })
            },
            release_item: {
                let f = release_item;
                Rc::new(move |scope_id| {
                    enter_mounted_world(|| f(scope_id));
                    schedule_flush();
                })
            },
            set_measured_size: {
                let f = set_measured_size;
                Rc::new(move |key, size| {
                    f(key, size);
                    schedule_flush();
                })
            },
        };
        <WebBackend as Backend>::create_virtualizer(self, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        <WebBackend as Backend>::virtualizer_data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        <WebBackend as Backend>::release_virtualizer(self, node)
    }

    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle {
        <WebBackend as Backend>::make_virtualizer_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for WebBackend {
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: surface lifecycle callbacks run author
        // code (draw-scene setup that creates/sets signals).
        let on_ready: primitives::graphics::OnReady = {
            let mut f = on_ready;
            Box::new(move |ev| {
                f(ev);
                schedule_flush();
            })
        };
        let on_resize: primitives::graphics::OnResize = {
            let mut f = on_resize;
            Box::new(move |ev| {
                f(ev);
                schedule_flush();
            })
        };
        let on_lost: primitives::graphics::OnLost = {
            let mut f = on_lost;
            Box::new(move || {
                f();
                schedule_flush();
            })
        };
        <WebBackend as Backend>::create_graphics(self, on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        <WebBackend as Backend>::release_graphics(self, node)
    }

    fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
        <WebBackend as Backend>::make_graphics_handle(self, node)
    }
}

impl caps::PortalOps for WebBackend {
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: backdrop click / Escape dismissal runs
        // the author's on_dismiss.
        let on_dismiss = on_dismiss.map(flushing0);
        <WebBackend as Backend>::create_portal(self, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        <WebBackend as Backend>::release_portal(self, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        <WebBackend as Backend>::set_portal_hidden(self, node, hidden)
    }

    fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
        <WebBackend as Backend>::make_portal_handle(self, node)
    }
}

impl caps::PresenceOps for WebBackend {
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        <WebBackend as Backend>::create_presence_placeholder(self, a11y)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        <WebBackend as Backend>::apply_presence(self, node, state, transition)
    }

    fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
        <WebBackend as Backend>::make_presence_handle(self, node)
    }
}

impl caps::NavigatorOps for WebBackend {
    fn create_navigator(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        presentation: Rc<dyn Any>,
        host: primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // NOT wrapped: NavigatorHost's callbacks (mount_screen etc.)
        // belong to the OLD-core navigator path, which the new core
        // does not route through (the vocabulary navigator handlers own
        // screens; their browser-back arrives via `newcore_url_sync`,
        // which stages commands and fires `schedule_flush` itself).
        // Author-initiated navigation stages via handlers already
        // wrapped above and commits inside the flush.
        <WebBackend as Backend>::create_navigator(self, type_id, type_name, presentation, host, a11y)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        <WebBackend as Backend>::release_navigator(self, node)
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        <WebBackend as Backend>::apply_navigator_slot_style(self, node, slot, style)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
        <WebBackend as Backend>::make_navigator_handle(self, node)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        <WebBackend as Backend>::navigator_attach_initial(self, navigator, screen, scope_id, options)
    }
}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for WebBackend {
    fn create_external(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WebBackend as Backend>::create_external(self, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node) {
        <WebBackend as Backend>::release_external(self, node)
    }

    fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
        <WebBackend as Backend>::missing_primitive_placeholder(self, label)
    }
}

impl caps::DocumentOps for WebBackend {
    fn create_element(&mut self, tag: &str) -> Self::Node {
        <WebBackend as Backend>::create_element(self, tag)
    }

    fn attach_html_id(&self, node: &Self::Node, id: &str) {
        <WebBackend as Backend>::attach_html_id(self, node, id)
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        <WebBackend as Backend>::attach_html_class(self, node, class)
    }

    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
        <WebBackend as Backend>::attach_html_style(self, node, prop, value)
    }

    fn register_raw_css(&mut self, css: &str) {
        <WebBackend as Backend>::register_raw_css(self, css)
    }
}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for WebBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let _t = crate::phase_timer::PhaseTimer::start("nc_apply_style");
        <WebBackend as Backend>::apply_style(self, node, style)
    }

    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        <WebBackend as Backend>::mint_style_class(self, style)
    }

    fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
        let _t = crate::phase_timer::PhaseTimer::start("nc_mint_class_for_app");
        <WebBackend as Backend>::mint_class_for_app(self, app)
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        <WebBackend as Backend>::apply_styled_states(self, node, base, overlays)
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        let _t = crate::phase_timer::PhaseTimer::start("nc_apply_styled_variants");
        <WebBackend as Backend>::apply_styled_variants(
            self,
            node,
            base,
            state_overlays,
            breakpoint_overlays,
            container_overlays,
        )
    }

    fn mark_container(&mut self, node: &Self::Node) {
        <WebBackend as Backend>::mark_container(self, node)
    }

    fn handles_states_natively(&self) -> bool {
        <WebBackend as Backend>::handles_states_natively(self)
    }

    fn token_updates_propagate_via_cascade(&self) -> bool {
        <WebBackend as Backend>::token_updates_propagate_via_cascade(self)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <WebBackend as Backend>::register_stylesheet(self, rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <WebBackend as Backend>::unregister_stylesheet(self, rules)
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        <WebBackend as Backend>::install_tokens(self, tokens)
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        <WebBackend as Backend>::update_tokens(self, tokens)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        let _t = crate::phase_timer::PhaseTimer::start("nc_on_node_unstyled");
        <WebBackend as Backend>::on_node_unstyled(self, node)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        // Dispatch-site glue: hover/press/focus state flips can stage
        // writes when the style path routes states through signals.
        // (Web resolves states via CSS pseudo-classes, so this is
        // rarely exercised — wrapped for uniformity.)
        let setter: Rc<dyn Fn(StateBits, bool)> = {
            let f = setter;
            Rc::new(move |bits, on| {
                f(bits, on);
                schedule_flush();
            })
        };
        <WebBackend as Backend>::attach_states(self, node, setter)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        <WebBackend as Backend>::set_disabled(self, node, disabled)
    }

    fn supports_preminted_styles(&self) -> bool {
        <WebBackend as Backend>::supports_preminted_styles(self)
    }

    fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {
        <WebBackend as Backend>::apply_default_text_font(self, font)
    }

    fn supports_js_class_bindings(&self) -> bool {
        <WebBackend as Backend>::supports_js_class_bindings(self)
    }

    fn register_reactive_class_binding(
        &mut self,
        node: &Self::Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: Rc<dyn Fn() -> u32>,
    ) -> u32 {
        // Folded into the new-core sid half — see [`js_sid`]; must
        // match the `notify_signal_value_js` delivery fold.
        <WebBackend as Backend>::register_reactive_class_binding(
            self,
            node,
            js_sid(signal_id),
            values,
            classes,
            value_reader,
        )
    }

    fn release_reactive_class_binding(&mut self, binding_id: u32) {
        <WebBackend as Backend>::release_reactive_class_binding(self, binding_id)
    }

    fn notify_signal_value_js(&mut self, signal_id: u64, value: u32) {
        // New-core-only channel (no Backend counterpart): the
        // vocabulary's per-signal notifier effect ships world-signal
        // commits here; JS fans out to every bound node
        // (`__idealystOnSignalChanged`). Ensure the shim exists — the
        // notifier's seeding first-run happens BEFORE the first
        // `register_reactive_class_binding` (which is what normally
        // injects it), and shipping into a missing dispatcher panics.
        self.ensure_class_bindings_shim();
        self.ship_signal_change_to_js(js_sid(signal_id), &value.to_string());
    }
}

impl caps::AssetOps for WebBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        <WebBackend as Backend>::register_asset(self, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        <WebBackend as Backend>::unregister_asset(self, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        <WebBackend as Backend>::register_typeface(self, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        <WebBackend as Backend>::unregister_typeface(self, id)
    }
}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for WebBackend {
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        <WebBackend as Backend>::update_accessibility(self, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        <WebBackend as Backend>::announce_for_accessibility(self, msg, priority)
    }

    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
        <WebBackend as Backend>::dump_accessibility_tree(self)
    }
}

impl caps::AnimationOps for WebBackend {
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        <WebBackend as Backend>::set_animated_f32(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        <WebBackend as Backend>::set_animated_color(self, node, prop, value)
    }
}

impl caps::IntrospectionOps for WebBackend {
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <WebBackend as Backend>::frame(self, node)
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <WebBackend as Backend>::absolute_frame(self, node)
    }

    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <WebBackend as Backend>::device_frame(self, node)
    }

    fn supports_native_introspection(&self) -> bool {
        <WebBackend as Backend>::supports_native_introspection(self)
    }

    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        <WebBackend as Backend>::introspect_native(self, node)
    }

    fn note_introspection_root(&self, node: &Self::Node) {
        <WebBackend as Backend>::note_introspection_root(self, node)
    }

    fn supports_screenshot(&self) -> bool {
        <WebBackend as Backend>::supports_screenshot(self)
    }

    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        <WebBackend as Backend>::capture_screenshot(self, done)
    }
}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for WebBackend {
    fn supports_batched_repeat(&self) -> bool {
        <WebBackend as Backend>::supports_batched_repeat(self)
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        <WebBackend as Backend>::execute_batch(self, batch)
    }

    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        <WebBackend as Backend>::execute_batch_with_attach(self, batch, parent, attach_locals)
    }
}

impl caps::WireBindingOps for WebBackend {
    fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
        <WebBackend as Backend>::note_text_binding(self, node, signal_ids, method)
    }

    fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value) {
        <WebBackend as Backend>::note_signal_initial(self, signal_id, value)
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        <WebBackend as Backend>::note_when_binding(
            self,
            anchor,
            signal_ids,
            cond_method,
            then_node,
            otherwise_node,
        )
    }

    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(runtime_core::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        <WebBackend as Backend>::note_switch_binding(
            self,
            anchor,
            signal_ids,
            cond_method,
            arms,
            default_node,
        )
    }

    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        <WebBackend as Backend>::note_repeat_binding(
            self,
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
        )
    }

    fn note_virtualizer_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
        horizontal: bool,
    ) {
        <WebBackend as Backend>::note_virtualizer_binding(
            self,
            anchor,
            signal_ids,
            count_method,
            row_template,
            row_index_signal_id,
            horizontal,
        )
    }

    fn supports_lazy_slot_capture(&self) -> bool {
        <WebBackend as Backend>::supports_lazy_slot_capture(self)
    }

    fn begin_slot_capture(&mut self) {
        <WebBackend as Backend>::begin_slot_capture(self)
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        <WebBackend as Backend>::end_slot_capture(self, slot_root)
    }
}

// ===========================================================================
// Browser-side tests (the crate's only test seam — backend-web compiles
// for wasm32 only, so there is no native-Rust test path; see tests.rs).
// Run with:
//   cd crates/backend/web
//   wasm-pack test --headless --chrome -- --features new-core
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_vocabulary::{button, text, toggle, view};
    use runtime_world::signal;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// Recreate a fresh `#app` mount point (same shape as tests.rs's
    /// helper — duplicated because that one is `#[cfg(test)]`-private to
    /// its module).
    fn setup_mount() -> web_sys::Element {
        let document = web_sys::window().unwrap().document().unwrap();
        if let Some(prior) = document.get_element_by_id("app") {
            prior.remove();
        }
        let el = document.create_element("div").unwrap();
        el.set_id("app");
        document.body().unwrap().append_child(&el).unwrap();
        el
    }

    /// Await one microtask checkpoint (lets `schedule_flush`'s queued
    /// `Promise.then` run).
    async fn microtask() {
        let promise = js_sys::Promise::resolve(&JsValue::UNDEFINED);
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    /// Await a real macrotask boundary (`setTimeout(ms)`) so scheduler
    /// timer callbacks — and the flush microtask the post-dispatch hook
    /// queues after them — have run.
    async fn sleep_ms(ms: i32) {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms)
                .unwrap();
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    /// Regression (dispatch-site glue): a REAL DOM click on a button
    /// commits its staged write with NO window-level event listener and
    /// NO rAF poll — the wrapped author callback itself schedules the
    /// flush. Fails if the caps-seam wrapping is removed (the write
    /// would stay staged forever now that the bubble-listener heuristic
    /// is gone).
    #[wasm_bindgen_test]
    async fn regression_click_flushes_via_dispatch_site_glue() {
        let mount = setup_mount();
        start(move || {
            let count = signal(0i32);
            view()
                .child(text().content(move || format!("n={}", count.get())))
                .child(
                    button()
                        .label("inc")
                        .on_press(move || count.update(|n| n + 1)),
                )
                .build()
        });
        let body_text = || mount.text_content().unwrap();
        // Initial Dyn-text content lands via the batched-text microtask.
        microtask().await;
        assert!(body_text().contains("n=0"), "boot mounted the tree");

        let btn: web_sys::HtmlElement = mount
            .query_selector("button")
            .unwrap()
            .expect("button rendered")
            .unchecked_into();
        // Synchronous DOM dispatch: the author handler stages, the
        // dispatch-site wrapper queues the flush microtask.
        btn.click();
        assert!(body_text().contains("n=0"), "staged, not committed");
        microtask().await;
        microtask().await;
        assert!(
            body_text().contains("n=1"),
            "dispatch-site glue committed the click's write"
        );
        stop();
    }

    /// Regression (post-dispatch hook): a signal set from an `after_ms`
    /// timer callback commits without any event and without the old
    /// rAF safety net — the scheduler fires the dispatch hook after the
    /// timer body returns. Fails if the scheduler's `after_ms` hook
    /// fire is removed.
    #[wasm_bindgen_test]
    async fn regression_after_ms_staged_write_commits() {
        let mount = setup_mount();
        let slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
        let slot_for_build = slot.clone();
        start(move || {
            let count = signal(0i32);
            slot_for_build.set(Some(count));
            view()
                .child(text().content(move || format!("t={}", count.get())))
                .build()
        });
        let body_text = || mount.text_content().unwrap();
        // Initial Dyn-text content lands via the batched-text microtask.
        microtask().await;
        assert!(body_text().contains("t=0"), "boot mounted the tree");

        let count = slot.get().expect("build ran");
        runtime_core::scheduling::after_ms_detached(0, move || count.set(9));
        assert!(body_text().contains("t=0"), "timer not fired yet");
        sleep_ms(20).await;
        assert!(
            body_text().contains("t=9"),
            "after_ms staged write committed via the post-dispatch hook"
        );
        stop();
    }

    /// Host's seven ops forward to the real DOM machinery: insert /
    /// insert_at (move semantics) / remove_child / clear_children /
    /// create_anchor, and web CSR reports splice support.
    #[wasm_bindgen_test]
    fn host_ops_forward_to_dom() {
        setup_mount();
        let mut backend = WebBackend::new("#app");
        let a11y = AccessibilityProps::default();

        let mut parent = caps::ViewOps::create_view(&mut backend, &a11y);
        let child_a = caps::TextOps::create_text(&mut backend, "a", &a11y);
        let child_b = caps::TextOps::create_text(&mut backend, "b", &a11y);
        let child_c = caps::TextOps::create_text(&mut backend, "c", &a11y);

        Host::insert(&mut backend, &mut parent, child_a.clone());
        Host::insert(&mut backend, &mut parent, child_c.clone());
        // insert_at splices b between a and c.
        Host::insert_at(&mut backend, &mut parent, child_b.clone(), 1);
        assert_eq!(parent.text_content().unwrap(), "abc");
        // insert_at on an already-mounted node is a MOVE (insertBefore
        // semantics) — the keyed reconciler depends on this.
        Host::insert_at(&mut backend, &mut parent, child_a.clone(), 3);
        assert_eq!(parent.text_content().unwrap(), "bca");

        Host::remove_child(&mut backend, &parent, &child_c);
        assert_eq!(parent.text_content().unwrap(), "ba");
        Host::clear_children(&mut backend, &parent);
        assert_eq!(parent.child_nodes().length(), 0);

        // The anchor is layout-transparent (`display: contents`).
        let anchor = Host::create_anchor(&mut backend);
        let anchor_el: &web_sys::Element = anchor.unchecked_ref();
        assert!(anchor_el
            .get_attribute("style")
            .unwrap_or_default()
            .contains("contents"));

        assert!(Host::supports_splice(&backend), "web CSR splices children");
    }

    /// End-to-end through the registry: realize a tree with a dynamic
    /// text binding and a keyed list against the real DOM, then commit
    /// staged writes with `world.flush()` and observe the DOM update —
    /// the registry-dispatched render path, minus the boot glue.
    #[wasm_bindgen_test]
    async fn registry_render_updates_dom_on_flush() {
        setup_mount();
        let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
        // Dyn text uses the batched `update_text_by_id` fast path, whose
        // microtask flush needs the global weak self-handle (same
        // install `start_in` performs).
        crate::install_global_self(&backend);
        let mut registry: Registry<WebBackend> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();

        let (count, rows, realized) = world.enter(|| {
            let count = signal(0i32);
            let rows = signal(vec![1u32, 2]);
            let tree = view()
                .child(text().content(move || format!("count={}", count.get())))
                .child(runtime_scene::keyed(
                    move || rows.get(),
                    |n| *n,
                    |n| text().content(format!("row{n}")).build(),
                ))
                .build();
            (count, rows, realize(&backend, &registry, tree))
        });

        let root = realized.collect_nodes().pop().expect("one root");
        setup_mount().append_child(&root).unwrap();
        let body_text = || root.text_content().unwrap();
        // Initial Dyn-text content lands via the batched-text
        // microtask — settle it before asserting.
        microtask().await;
        assert!(body_text().contains("count=0"));
        assert!(body_text().contains("row1") && body_text().contains("row2"));

        // Staged: nothing observable until the driver flushes.
        count.set(5);
        rows.update(|r| {
            let mut r = r.clone();
            r.push(9);
            r
        });
        microtask().await;
        assert!(body_text().contains("count=0"), "writes stage until flush");
        world.flush();
        // The flush re-runs the text effect; the DOM write itself rides
        // the batched-text microtask.
        microtask().await;
        assert!(body_text().contains("count=5"));
        assert!(body_text().contains("row9"));

        // Drop-as-teardown: unmounting is dropping the Realized.
        drop(realized);
        drop(world);
    }

    /// Regression (flat_list rendered ZERO rows on new-core web): the
    /// JS windowing machinery invokes the vocabulary handler's
    /// `mount_item` from its own deferred fill — OUTSIDE `World::enter`
    /// — and row realization is creation-side (row signals, Dyn text
    /// effects, `theme_ctx` injects), which aborts there. The caps
    /// wrapper must enter the boot-stored world around mount/release
    /// (`enter_mounted_world`); without it every row realize dies with
    /// "signal()/effect() called outside World::enter" and the list
    /// stays empty (the website /primitives Lists cell repro).
    #[wasm_bindgen_test]
    async fn regression_flat_list_mounts_rows_on_new_core() {
        let mount = setup_mount();
        start(move || {
            // A fixed-height list surface: the JS window fill mounts
            // nothing into a zero-extent viewport.
            let sheet = Rc::new(runtime_core::StyleSheet::r#static(runtime_core::StyleRules {
                height: Some(runtime_core::Tokenized::Literal(runtime_core::Length::Px(120.0))),
                ..Default::default()
            }));
            runtime_vocabulary::builders::virtualizer(
                || 3usize,
                |i| i as u64,
                runtime_core::primitives::virtualizer::ItemSize::Known(Rc::new(|_| 20.0)),
                |i| {
                    // Creation-side row work — the aborting class: a
                    // row-local signal plus a Dyn text effect.
                    let n = signal(i as i32);
                    text().content(move || format!("row-{}", n.get())).build()
                },
            )
            .style(sheet)
            .build()
        });
        // The initial window fill rides the virtualizer's deferred
        // callbacks (microtask + ResizeObserver), and row text rides the
        // batched-text microtask — settle across a real macrotask
        // boundary before asserting.
        sleep_ms(50).await;
        let body_text = mount.text_content().unwrap();
        for row in ["row-0", "row-1", "row-2"] {
            assert!(
                body_text.contains(row),
                "expected {row} mounted by the world-entered mount_item, got: {body_text}"
            );
        }
        stop();
    }

    /// The full boot path: `start` mounts into `#app`, and the flush
    /// driver's microtask hook commits an event-staged write without an
    /// explicit `flush()` call.
    #[wasm_bindgen_test]
    async fn start_boot_path_and_microtask_flush_driver() {
        let mount = setup_mount();
        let count_slot: Rc<Cell<Option<runtime_world::Signal<i32>>>> = Rc::new(Cell::new(None));
        let slot = count_slot.clone();
        start(move || {
            let count = signal(0i32);
            slot.set(Some(count));
            view()
                .child(text().content(move || format!("n={}", count.get())))
                .child(
                    button()
                        .label("inc")
                        .on_press(move || count.update(|n| n + 1)),
                )
                .child(toggle())
                .build()
        });
        let body_text = || mount.text_content().unwrap();
        // Initial Dyn-text content lands via the batched-text microtask.
        microtask().await;
        assert!(body_text().contains("n=0"), "boot mounted the tree");

        // Stage a write the way an event handler would, then ride the
        // driver's flush microtask (what the dispatch-site glue queues).
        let count = count_slot.get().expect("build ran");
        count.set(3);
        assert!(body_text().contains("n=0"), "staged, not committed");
        schedule_flush();
        microtask().await;
        microtask().await;
        assert!(body_text().contains("n=3"), "microtask flush committed");

        stop();
        assert!(
            APP.with(|s| s.borrow().is_none()),
            "stop() released the app"
        );
    }


    /// Batched repeat (the old core's `Element::Repeat` fast path,
    /// ported): a static-range repeat of styled view+text rows mounts
    /// through ONE `execute_batch_with_attach` into the real DOM, a
    /// keyed sibling AFTER the repeat bases its splice index correctly
    /// (reorders land in the right positions), and swapping the region
    /// out removes every row.
    #[wasm_bindgen_test]
    async fn repeat_batch_mounts_rows_and_keyed_sibling_stays_correct() {
        setup_mount();
        let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
        crate::install_global_self(&backend);
        let mut registry: Registry<WebBackend> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();

        let sheet = {
            let rules = runtime_core::StyleRules {
                opacity: Some(runtime_core::Tokenized::Literal(0.9)),
                ..Default::default()
            };
            Rc::new(runtime_core::StyleSheet::r#static(rules))
        };
        let (rows, realized) = world.enter(|| {
            let rows = signal(vec![1u32, 2, 3]);
            let sheet = sheet.clone();
            let tree = view()
                .children({
                    let mut children =
                        runtime_vocabulary::glue::__static_repeat(3, move |i| {
                            view()
                                .style(runtime_core::StyleApplication::new(sheet.clone()))
                                .child(text().content(format!("batch{i}")))
                                .build()
                        });
                    children.push(runtime_scene::keyed(
                        move || rows.get(),
                        |n| *n,
                        |n| text().content(format!("k{n}")).build(),
                    ));
                    children
                })
                .build();
            (rows, realize(&backend, &registry, tree))
        });
        let root = realized.collect_nodes().pop().expect("one root");
        setup_mount().append_child(&root).unwrap();
        let txt = || root.text_content().unwrap();
        assert_eq!(
            txt(),
            "batch0batch1batch2k1k2k3",
            "3 batched rows then the keyed rows, in order"
        );
        // Keyed reorder AFTER a batched repeat: the reconciler's base
        // index must account for the repeat's rows.
        rows.set(vec![3, 1, 2]);
        world.flush();
        assert_eq!(
            txt(),
            "batch0batch1batch2k3k1k2",
            "reorder repositions ONLY the keyed rows, after the repeat"
        );
        drop(realized);
        drop(world);
        setup_mount(); // leave a clean #app for whatever test runs next
    }

    /// Fallback repeat semantics survive batching being available: a
    /// row shape with reactive text bails to per-row mounts, its
    /// bindings stay live post-mount, and teardown runs per-row
    /// cleanups (drop-as-teardown through the enclosing Realized).
    #[wasm_bindgen_test]
    async fn repeat_fallback_rows_stay_reactive_and_clean_up() {
        setup_mount();
        let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
        crate::install_global_self(&backend);
        let mut registry: Registry<WebBackend> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();

        let cleanups: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let (label, realized) = world.enter(|| {
            let label = signal(String::from("a"));
            let cleanups = cleanups.clone();
            let tree = view()
                .children(runtime_vocabulary::glue::__static_repeat(2, move |_i| {
                    let cleanups = cleanups.clone();
                    // Row-scoped teardown probe (the vocabulary's
                    // on_teardown — a probe effect in the ambient
                    // collector; plain on_cleanup needs a running
                    // effect, which a mount handler is not).
                    runtime_vocabulary::style_attach::on_teardown(move || {
                        cleanups.set(cleanups.get() + 1)
                    });
                    view().child(text().content(move || label.get())).build()
                }))
                .build();
            (label, realize(&backend, &registry, tree))
        });
        let root = realized.collect_nodes().pop().expect("one root");
        setup_mount().append_child(&root).unwrap();
        let txt = || root.text_content().unwrap();
        microtask().await; // batched-text microtask delivers Dyn content
        assert_eq!(txt(), "aa", "per-row reactive text mounted");

        label.set("b".to_string());
        world.flush();
        microtask().await;
        assert_eq!(txt(), "bb", "fallback rows keep live bindings");

        drop(realized);
        // 3, not 2: the batch ATTEMPT builds row 0 first (running its
        // probe registration) before the reactive-text shape bails it to
        // the fallback, which rebuilds all rows — so row 0's builder ran
        // twice. Old-core parity: `try_build_repeat_batched` also ran
        // `row_builder` again after a bail, double-registering any
        // scope-level cleanups a row builder creates.
        assert_eq!(cleanups.get(), 3, "per-row cleanups fire at teardown");
        drop(world);
        setup_mount();
    }

    /// The f-string JS text-binding fast path, end to end: mount
    /// registers the structured binding (initial content painted by the
    /// JS shim), a committed signal write repaints the DOM through the
    /// per-signal notifier + `__idealystOnSignalChanged` — with NO
    /// per-leaf Rust effect (nothing rides the batched-text microtask).
    #[wasm_bindgen_test]
    async fn js_text_binding_repaints_dom_via_notifier() {
        setup_mount();
        let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
        crate::install_global_self(&backend);
        let mut registry: Registry<WebBackend> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();

        use runtime_vocabulary::glue::{
            ReactiveTextSlot as _, TextSlotPart, __idealyst_text_from_parts,
        };
        let (n, realized) = world.enter(|| {
            let n = signal(1u32);
            let assembled = __idealyst_text_from_parts(vec![
                TextSlotPart::Lit("n="),
                TextSlotPart::Slot(n.__idealyst_text_slot(|d| format!("{d}"))),
            ]);
            let tree = view().child(text().content(assembled)).build();
            (n, realize(&backend, &registry, tree))
        });
        let root = realized.collect_nodes().pop().expect("one root");
        setup_mount().append_child(&root).unwrap();
        let txt = || root.text_content().unwrap();
        assert_eq!(txt(), "n=1", "registration paints the initial value synchronously");

        n.set(7);
        world.flush();
        // The repaint happens INSIDE the flush (the notifier effect ships
        // synchronously to the JS dispatcher) — no microtask needed.
        assert_eq!(txt(), "n=7", "notifier + JS fan-out repainted the leaf");

        drop(realized);
        drop(world);
        setup_mount();
    }

    /// Regression (cross-core JS sid aliasing): world signal ids used
    /// to enter the PAGE-global JS binding tables as their bare low-32
    /// slot — the same small integers old-core arena signals use — so a
    /// live new-core binding could poison a later old-core binding's
    /// first paint at the aliased sid (surfaced as
    /// `tests::regression_fstring_two_bindings_one_signal` painting a
    /// sibling test's cached value once a boot-order change shifted
    /// old-arena ids). New-core sids must land in the folded high-bit
    /// range (`js_sid`), on BOTH the registration and delivery paths.
    #[wasm_bindgen_test]
    async fn regression_newcore_sids_stay_out_of_oldcore_arena_range() {
        setup_mount();
        let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
        crate::install_global_self(&backend);
        let mut registry: Registry<WebBackend> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();

        use runtime_vocabulary::glue::{
            ReactiveTextSlot as _, TextSlotPart, __idealyst_text_from_parts,
        };
        let (n, realized) = world.enter(|| {
            let n = signal(3u32);
            let assembled = __idealyst_text_from_parts(vec![
                TextSlotPart::Lit("k="),
                TextSlotPart::Slot(n.__idealyst_text_slot(|d| format!("{d}"))),
            ]);
            let tree = view().child(text().content(assembled)).build();
            (n, realize(&backend, &registry, tree))
        });
        let root = realized.collect_nodes().pop().expect("one root");
        setup_mount().append_child(&root).unwrap();

        // Delivery keeps working through the folded key.
        n.set(8);
        world.flush();
        assert!(root.text_content().unwrap().contains("k=8"));

        let folded = super::js_sid(n.raw_id()) as u32;
        assert!(folded >= 0x8000_0000, "fold must set the high bit");
        let values: js_sys::Map = js_sys::Reflect::get(
            &web_sys::window().unwrap(),
            &JsValue::from_str("__idealystSignalValues"),
        )
        .unwrap()
        .unchecked_into();
        assert!(
            values.has(&JsValue::from(folded)),
            "the binding's cached value must live at the FOLDED sid"
        );
        assert!(
            !values.has(&JsValue::from(n.raw_id() as u32)),
            "the bare low-32 slot (old-core arena range) must stay untouched"
        );

        drop(realized);
        drop(world);
        setup_mount();
    }

    /// Regression (found by the bench gate): a TEXT-binding notifier
    /// firing BEFORE the first class binding registers caches the
    /// pre-wrap `__idealystOnSignalChanged` handle — class bindings
    /// registered later would never hear signal changes (sclass rows
    /// froze). `ensure_class_bindings_shim` now invalidates the cached
    /// handle after wrapping.
    #[wasm_bindgen_test]
    async fn regression_class_binding_registered_after_text_binding_still_swaps() {
        setup_mount();
        let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
        crate::install_global_self(&backend);
        let mut registry: Registry<WebBackend> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        let registry = Rc::new(registry);
        let world = World::new();

        use runtime_vocabulary::glue::{
            ReactiveTextSlot as _, TextSlotPart, __idealyst_text_from_parts,
        };
        let sheet = {
            static KEY: u8 = 0;
            runtime_core::cached_stylesheet(&KEY as *const u8 as usize, || {
                Rc::new(runtime_core::StyleSheet::r#static(
                    runtime_core::StyleRules::default(),
                ))
            })
        };
        let (t, c, realized) = world.enter(|| {
            let t = signal(0u32);
            let c = signal(0u32);
            // TEXT binding first (installs text_bindings.js + caches the
            // dispatcher handle via the seeding notifier run)...
            let assembled = __idealyst_text_from_parts(vec![
                TextSlotPart::Lit("t="),
                TextSlotPart::Slot(t.__idealyst_text_slot(|d| format!("{d}"))),
            ]);
            let sheet = sheet.clone();
            let tree = view()
                .child(text().content(assembled))
                // ...then a signal-CLASS binding (class_bindings.js wraps
                // the dispatcher afterwards).
                .child(
                    view().style(runtime_vocabulary::signal_class(
                        c,
                        &[0, 1],
                        move |v| {
                            let mut rules = runtime_core::StyleRules::default();
                            rules.opacity =
                                Some(runtime_core::Tokenized::Literal(if v == 0 {
                                    0.25
                                } else {
                                    0.75
                                }));
                            runtime_core::StyleApplication::new(sheet.clone())
                                .with_overrides(rules)
                        },
                    )),
                )
                .build();
            (t, c, realize(&backend, &registry, tree))
        });
        let root = realized.collect_nodes().pop().expect("one root");
        setup_mount().append_child(&root).unwrap();
        let row: web_sys::Element = root
            .child_nodes()
            .item(1)
            .expect("styled row")
            .unchecked_into();
        let class_of = || row.get_attribute("class").unwrap_or_default();
        let initial_class = class_of();

        c.set(1);
        world.flush();
        microtask().await; // class swaps ride the class-batch microtask
        assert_ne!(
            class_of(),
            initial_class,
            "class binding registered AFTER a text binding must still hear commits"
        );

        // And the text binding keeps repainting too.
        t.set(9);
        world.flush();
        assert!(root.text_content().unwrap().contains("t=9"));

        drop(realized);
        drop(world);
        setup_mount();
    }

    /// Force `window.innerWidth` to report `w` (headless Chrome won't
    /// actually resize) so a synthetic `resize` event exercises the
    /// viewport source end-to-end.
    fn force_inner_width(w: f64) {
        let win = web_sys::window().unwrap();
        let desc = js_sys::Object::new();
        js_sys::Reflect::set(&desc, &"configurable".into(), &true.into()).unwrap();
        js_sys::Reflect::set(
            &desc,
            &"get".into(),
            &js_sys::Function::new_no_args(&format!("return {w};")),
        )
        .unwrap();
        js_sys::Object::define_property(
            win.unchecked_ref::<js_sys::Object>(),
            &"innerWidth".into(),
            &desc,
        );
    }

    /// Regression (the idea-ui-docs "hamburger visible at desktop
    /// width" bug): the per-world breakpoint signal was SEED-ONLY on
    /// the new core — no web resize source — so author reactivity
    /// reading `current_breakpoint()` (the shell's
    /// `when(!sidebar_pinned(Lg))`) never re-fired. The boot must seed
    /// the real window size, and a window `resize` must re-fire
    /// breakpoint-dependent bindings through the staged-write → flush
    /// pipeline.
    #[wasm_bindgen_test]
    async fn regression_hamburger_breakpoint_refires_on_window_resize() {
        use runtime_vocabulary::glue;

        let mount = setup_mount();
        // Pin a known starting bucket BEFORE boot: the boot seed reads
        // the (forced) window size.
        force_inner_width(500.0); // Xs
        start(move || {
            view()
                .child(text().content(move || {
                    format!("bp={:?}", glue::current_breakpoint().get())
                }))
                .build()
        });
        microtask().await; // batched-text microtask paints the initial content
        let body_text = || mount.text_content().unwrap();
        assert!(
            body_text().contains("bp=Xs"),
            "boot seeded the real window size (got {})",
            body_text()
        );

        // Cross the Lg threshold and fire the resize source.
        force_inner_width(1280.0); // Xl
        let win = web_sys::window().unwrap();
        win.dispatch_event(&web_sys::Event::new("resize").unwrap())
            .unwrap();
        assert!(
            body_text().contains("bp=Xs"),
            "resize stages; nothing commits before the scheduled flush"
        );
        microtask().await; // flush microtask commits the staged viewport
        microtask().await; // batched-text microtask repaints the binding
        assert!(
            body_text().contains("bp=Xl"),
            "breakpoint-dependent binding re-fired after resize (got {})",
            body_text()
        );

        // `stop` removes the listener: further resizes are inert.
        stop();
        force_inner_width(500.0);
        win.dispatch_event(&web_sys::Event::new("resize").unwrap())
            .unwrap();
        microtask().await;
        setup_mount();
    }
}

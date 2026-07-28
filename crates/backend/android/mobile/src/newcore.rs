//! New-core adoption for the Android backend (idea-lite migration, P5).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on `AndroidBackend` — the
//! production shape of the migration: no `LegacyBridge` wrapper in the
//! render path. Every trait method delegates via UFCS
//! (`<AndroidBackend as Backend>::method(self, …)`) to the existing
//! `Backend` impl, so the JNI mechanism code (View creation via the
//! Kotlin runtime, Taffy layout, GradientDrawable styling, animators,
//! the RecyclerView virtualizer) is REUSED verbatim — this module adds
//! a second *front* onto the same machinery, exactly like
//! `backend-web/src/newcore.rs` does for the DOM and
//! `backend-macos/src/newcore.rs` for AppKit (this file mirrors those
//! modules mechanically; keep the three in sync).
//!
//! # Delegation status — every capability accounted for
//!
//! | Trait | Status |
//! |---|---|
//! | `runtime_scene::Host` (7 ops) | direct (`create_anchor` → `create_reactive_anchor`, `supports_splice` → `supports_child_splice` — the P1 renames) |
//! | `AppEnvOps` | direct (+ dispatch-site glue on `set_app_key_handler`) |
//! | `LifecycleOps` | direct (`is_hydrating` is always `false` on this backend — no hydration on native) |
//! | `ViewOps` | direct |
//! | `InputOps` | direct (+ glue on touch/wheel/hover/file-drop handlers) |
//! | `PressableOps` | direct (+ glue on `on_click`) |
//! | `TextOps` | direct (the js-binding methods resolve to the trait-default no-ops, same as the old walker: `supports_js_text_bindings` is `false` here) |
//! | `ButtonOps` | direct (+ glue on the `Action` evaluator) |
//! | `ImageOps` | direct (+ glue on load/error handlers) |
//! | `IconOps` | direct |
//! | `LinkOps` | direct (+ glue on `on_activate`) |
//! | `TextInputOps` | direct (+ glue on change/key/blur/focus) |
//! | `ToggleOps` | direct (+ glue on `on_change`) |
//! | `SliderOps` | direct (+ glue on `on_change`) |
//! | `ActivityIndicatorOps` | direct |
//! | `ScrollOps` | direct (+ glue on `on_scroll`) |
//! | `SafeAreaOps` | direct |
//! | `VirtualizerOps` | direct (+ glue on mount/release/measured-size) |
//! | `GraphicsOps` | direct (+ glue on surface lifecycle) |
//! | `PortalOps` | direct (+ glue on `on_dismiss`) |
//! | `PresenceOps` | direct |
//! | `NavigatorOps` | direct |
//! | `ExternalOps` | direct |
//! | `DocumentOps` | direct (web-flavored methods — `create_element`, `attach_html_*`, `register_raw_css` — resolve to the same trait-default no-ops the old walker hit on this backend) |
//! | `StyleOps` | direct (class-minting methods return the trait-default `None` on native; the vocabulary's `attach_style` then takes the `apply_styled_variants` path — identical routing to the old walker. Token updates fan out through the theme cohort, not a cascade, exactly like macOS.) |
//! | `AssetOps` | direct |
//! | `A11yOps` | direct |
//! | `AnimationOps` | direct |
//! | `IntrospectionOps` | direct |
//! | `BatchOps` | direct |
//! | `WireBindingOps` | direct (wire-recorder no-ops on this backend, same as today) |
//!
//! **30/30 direct, 0 adapted, 0 stubbed.** Nothing panics, nothing
//! silently no-ops beyond what the wrapped `Backend` impl already does.
//! Where the Android backend does not override a `Backend` method (the
//! DOM-only and wire-only families), the UFCS call resolves to the same
//! trait-default fallback the old walker would hit — behavior is
//! identical by construction. The impl bodies below are generated
//! mechanically from `backend-web/src/newcore.rs` /
//! `backend-macos/src/newcore.rs` (themselves generated from
//! `runtime_vocabulary::bridge`, the compile-time proof of the
//! signature freeze), with the backend type renamed. The delegated old
//! bodies keep their documented invariants — notably the
//! clear-children/Taffy sync and "scroll writes route via the
//! scroll-handle ops, never a held `borrow_mut`" rules
//! ([[project_ios_clear_children_taffy_sync]],
//! [[project_codeblock_author_driven_padding]]) — because those bodies
//! are literally the ones running.
//!
//! # Boot path — [`start`]
//!
//! Client-render-only mount of a `runtime_scene::Element` tree against
//! the live Android View hierarchy through the registry. Like macOS,
//! host duties are split: the CLI-generated JNI wrapper
//! (`Java_<pkg>_NativeBridge_attach`) owns the Activity handoff —
//! constructing `AndroidBackend::new(context, root)`, then
//! `install_global_self` + `install_scheduler` (+ `install_render_loop`
//! under `async-driver`) + `register_extensions` — and THIS function
//! owns everything from "backend is wired to a host `ViewGroup`"
//! onward. See `crates/tools/build/android`'s generated wrapper (its
//! `new-core` feature) for the canonical caller, and
//! `crates/dev/newcore-android-smoke` for a full app.
//!
//! Sequence (mirrors `runtime_core::mount`'s ordering where they
//! overlap):
//!
//! 1. Install the default monotonic time source (the Android analogue
//!    of web `start_in`'s `install_time_source` — without it the
//!    animation clock and `PhaseTimer` read 0). The old `mount`
//!    preamble's other ambient installs (current platform, color
//!    scheme, URL opener, announcer) are runtime-core-private and
//!    skipped here, exactly as on the web/macOS new-core boots — a
//!    public seam for them is a later-phase migration item.
//! 2. `Registry` (`register_builtins` + the `register` seam) + `World`
//!    + `world.enter(realize)`.
//! 3. `runtime_core::scheduling::drain_buffered_microtasks()` — a
//!    no-op on this backend today (the Android scheduler posts
//!    microtasks straight to the main looper and never buffers; only
//!    web's hydration window and macOS's mount-buffering window do),
//!    kept so the boot sequence stays line-for-line comparable across
//!    the three new-core backends. Build-time microtasks run on later
//!    looper messages, after `attach` returns — same as the old boot.
//! 4. `Backend::finish(root)` — appends the single root View into the
//!    host `ViewGroup` and schedules the retrying Taffy layout pass
//!    (the host reads back 0×0 until Android's first measure; the
//!    retry loop in `imp::scheduler` handles it — old-boot mechanism,
//!    reused).
//! 5. `world.flush()` — commit anything staged during mount (ref-fill
//!    callbacks, handler setup) before the first paint.
//! 6. Install the flush driver (dispatch hook → [`schedule_flush`])
//!    and retain `{Realized, backend, registry, world}` in the
//!    module's thread-local `APP` slot (page…process-lifetime, same
//!    retention convention as `backend-web`'s `APP`). The JNI
//!    wrapper's `detach` calls [`stop`].
//!
//! **Hydration is NOT in scope** (native never hydrates).
//!
//! # Flush driver (design §3: precise dispatch-site glue, the settled
//! web shape)
//!
//! The new kernel stages writes; nothing is observable until the host
//! driver calls [`World::flush`]. Android needs **no event-monitor or
//! per-frame safety net**: unlike AppKit (whose control tracking loops
//! pull events around `sendEvent:`), every author-visible event on
//! Android arrives as a JNI callback into a Rust closure the backend
//! itself installed (`RustClickListener` → `on_click`,
//! `RustTextWatcher` → `on_change`, `RustTouchListener` → the touch
//! handler, …). Two hooks cover everything:
//!
//! 1. **Author-callback wrapping (this module).** Every callback-taking
//!    capability impl below wraps the author callback before delegating
//!    to the `Backend` machinery: press/click, input/change, toggle,
//!    slider, scroll, hover, wheel, touch, key, blur/focus, file-drop,
//!    image load/error, link activation, portal dismiss, graphics
//!    lifecycle, virtualizer row mount/release, state setters, and the
//!    app-level key handler. The wrapper calls the author fn, then
//!    [`schedule_flush`] — one deduped
//!    `runtime_core::scheduling::schedule_microtask`, which on this
//!    platform is `Handler.post` to the main looper: it runs on a
//!    LATER looper message, strictly after the current JNI event
//!    dispatch returns to Java. Net effect: stage during dispatch,
//!    commit at the looper-turn boundary right after — the idea-lite
//!    contract. Because the wrapping happens in these new-core-only
//!    impls, the shared `imp` event closures are reused verbatim and
//!    the old core never pays for it.
//! 2. **Post-dispatch hook ([`crate::dispatch_hook`]).** Author code
//!    also runs from non-event surfaces: `after_ms` timers,
//!    `after_animation_frame` one-shots, `raf_loop` iterations
//!    (`imp/scheduler.rs`), and async-executor future polls
//!    (`imp/async_executor.rs`). Those sites fire the thread-local
//!    hook after each such callback; [`start`] installs
//!    [`schedule_flush`] into the slot (no-op default, so the old
//!    core is untouched). Scheduled *microtasks* deliberately do NOT
//!    fire the hook — the flush itself rides the microtask queue and
//!    would re-arm itself forever (see `dispatch_hook`'s module docs;
//!    on Android microtasks and timers share one JNI trampoline, so
//!    the hook is fired by wrapping at the `Scheduler` impl, never in
//!    the trampoline).
//!
//! Residual surfaces NOT covered (documented, not silent):
//!
//! - **`Element::External` SDK glue** (codeblock's `RustCodeBlock`
//!   scroll callbacks, `RustTextureListener`, drawer-navigator chrome,
//!   `RustActionBarHelper` header buttons, `RustActivityResult`
//!   dispatch): these belong to old-core SDK surfaces that predate the
//!   new core; their ports must call [`schedule_flush`] after author
//!   callbacks — same residual as web's External note.
//! - **Viewport resize** (`RustViewportResizeListener`): feeds the
//!   old-core viewport signal + a backend-internal layout pass. The
//!   new core has no viewport source yet on any backend (web is
//!   seed-only) — the layout pass itself still runs, so views reflow;
//!   only new-core *breakpoint re-resolution* waits on that seam.
//!
//! Everything funnels through [`schedule_flush`]/`flush_now`, which
//! skips re-entrant flushes (`world.is_flushing()`) — belt and braces;
//! a posted looper message can't actually preempt a synchronous flush.
//!
//! # TLS audit (bionic's 128 pthread-key budget — the constraint that
//! crashed idea-ui-docs once, see
//! [[project_android_tls_key_limit_stylesheets]])
//!
//! The new-core path adds, in this crate:
//!
//! - [`FLUSH`] (this module): ONE dtor-bearing `thread_local!`
//!   (`RefCell<Option<World>>` + dedup flag folded into a single slot
//!   struct precisely to spend one key, not two).
//! - `APP` (android-gated, this module): ONE dtor-bearing
//!   `thread_local!` (kept separate from `FLUSH` because a flush may
//!   run while the app value is mid-construction inside [`start`] —
//!   same split as web/macOS, but each of those spends a slot per
//!   static; here the remaining flag rides inside `FLUSH`).
//! - `crate::dispatch_hook::HOOK`: `Cell<Option<fn()>>`, const-init,
//!   no destructor → plain ELF-TLS, **zero** pthread keys.
//!
//! Net: **+2 pthread TLS keys** for a new-core boot (the kernel itself
//! adds one more: `runtime-world`'s single world-registry key — by
//! design the whole `runtime-world` crate owns exactly one). The
//! vocabulary's handler statics (`navigator` queue/tick, robot
//! registry under the `robot` feature) are shared across all new-core
//! backends and add ≤4 more; nothing here re-introduces a per-sheet or
//! per-node thread-local.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::primitives;
use runtime_world::World;

// ===========================================================================
// Flush driver (host-compilable half — regression-tested from any
// platform via `cargo test -p backend-android-mobile --features new-core`)
// ===========================================================================

/// The flush driver's per-thread state. World handle + dedup flag are
/// folded into ONE `thread_local!` on purpose: each dtor-bearing
/// thread-local costs a bionic pthread TLS key (module docs, "TLS
/// audit"), and the two fields are only ever touched together.
struct FlushSlot {
    /// The world the flush driver commits. Kept out of the `APP`
    /// slot's custody so [`schedule_flush`] never touches app state (a
    /// flush can run while the app value is mid-construction inside
    /// [`start`]).
    world: RefCell<Option<World>>,
    /// Dedup flag: one queued flush microtask at a time.
    queued: Cell<bool>,
}

thread_local! {
    static FLUSH: FlushSlot = const {
        FlushSlot {
            world: RefCell::new(None),
            queued: Cell::new(false),
        }
    };
}

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// The author-callback wrappers below and the scheduler/executor
/// post-dispatch hook call this right after author-visible dispatch.
/// On Android the microtask is a `Handler.post` to the main looper, so
/// the commit lands on the next looper turn — after the current JNI
/// event dispatch has returned to Java.
pub fn schedule_flush() {
    if FLUSH.with(|f| f.queued.replace(true)) {
        return;
    }
    runtime_core::scheduling::schedule_microtask(|| {
        FLUSH.with(|f| f.queued.set(false));
        flush_now();
    });
}

/// Flush the mounted world immediately (skipped while it is already
/// mid-flush).
fn flush_now() {
    let world = FLUSH.with(|f| f.world.borrow().clone());
    if let Some(world) = world {
        if !world.is_flushing() {
            world.flush();
        }
    }
}

/// Synchronously commit staged writes (skipped mid-flush; no-op before
/// [`start`]). JNI-interop seam, mirroring `backend_web::newcore::
/// flush_sync`: an export that staged writes and must return to Java
/// with the tree updated (robot verbs, diagnostics) cannot ride the
/// posted looper message the dispatch-site glue queues — it flushes
/// before returning instead.
pub fn flush_sync() {
    flush_now();
}

/// Run `f` with the mounted app's world ambient (`World::enter`).
/// JNI-interop seam: exports that must CREATE reactive state
/// (`signal()`, `memo()`) run outside any handler/effect, where no
/// world is ambient — creation would panic. Reads and writes on
/// existing handles do NOT need this. `None` before [`start`].
pub fn with_world_entered<R>(f: impl FnOnce() -> R) -> Option<R> {
    let world = FLUSH.with(|fl| fl.world.borrow().clone());
    world.map(|w| w.enter(f))
}

/// True while a new-core app is mounted (`start` ran, `stop` hasn't).
/// Core-selection probe for shared transports (robot relay).
pub fn is_booted() -> bool {
    FLUSH.with(|f| f.world.borrow().is_some())
}

fn set_flush_world(world: Option<World>) {
    FLUSH.with(|f| *f.world.borrow_mut() = world);
}

// ---------------------------------------------------------------------------
// Dispatch-site glue: author-callback wrappers
// ---------------------------------------------------------------------------
//
// Each helper wraps an author callback so that, AFTER the author code
// returns, one deduped flush microtask is queued. These are used by the
// callback-taking caps impls below — the precise dispatch-site glue the
// flush driver is built on (module docs, "Flush driver"). Wrapping here
// (instead of inside the shared `imp` event closures) keeps the
// old-core render path byte-identical: the old core applies writes
// synchronously and must not pay a flush per event. Host-compilable so
// the wrap-then-flush contract has regression tests that run from any
// platform (the caps impls that consume them are android-gated).

/// Wrap a zero-arg author callback (`on_press`, `on_dismiss`,
/// `on_activate`, image error, …).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn flushing0(f: Rc<dyn Fn()>) -> Rc<dyn Fn()> {
    Rc::new(move || {
        f();
        schedule_flush();
    })
}

/// Wrap a one-value author callback (`on_change(String/bool/f32)`,
/// hover, focus …).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn flushing1<A: 'static>(f: Rc<dyn Fn(A)>) -> Rc<dyn Fn(A)> {
    Rc::new(move |a| {
        f(a);
        schedule_flush();
    })
}

/// Wrap a key handler (`&KeyEvent -> KeyOutcome`; outcome passes
/// through so the backend's consume/pass-through decision is
/// unchanged).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn flushing_key(f: primitives::key::KeyDownHandler) -> primitives::key::KeyDownHandler {
    Rc::new(move |ev| {
        let outcome = f(ev);
        schedule_flush();
        outcome
    })
}

// ===========================================================================
// Boot path + Host + capability-trait delegation (android-gated: the
// real `AndroidBackend` only exists under `target_os = "android"`)
// ===========================================================================

#[cfg(target_os = "android")]
mod native {
    use std::any::{Any, TypeId};
    use std::cell::RefCell;
    use std::rc::Rc;

    use jni::objects::{GlobalRef, JObject, JValue};
    use runtime_core::accessibility::{
        AccessibilityProps, AccessibilityTree, LiveRegionPriority, Role,
    };
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

    use super::{flushing0, flushing1, flushing_key, schedule_flush, set_flush_world};
    use crate::imp::{self, AndroidBackend};

    // Re-exported so JNI wrappers and app crates can name the boot-path
    // types without a direct runtime-scene dependency — mirrors how
    // consumers reach the old core's `Element` through `runtime_core`.
    pub use runtime_scene::Element as SceneElement;
    pub use runtime_scene::Registry as SceneRegistry;

    // -----------------------------------------------------------------------
    // Boot path
    // -----------------------------------------------------------------------

    thread_local! {
        /// The mounted new-core app: dropping this tears everything
        /// down (Realized first — unmount — then the World). Process-
        /// lifetime unless the wrapper's `detach` calls [`stop`]; same
        /// retention convention as `backend-web`'s `APP`. Separate
        /// from [`super::FLUSH`] (module docs, "TLS audit": this is
        /// the second — and last — pthread key the adoption spends).
        static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    }

    /// Everything the boot path must keep alive. Field order is drop
    /// order: the realized tree unmounts before the world (its slots'
    /// owner) dies.
    struct App {
        realized: Realized<GlobalRef>,
        _backend: Rc<RefCell<AndroidBackend>>,
        _registry: Rc<Registry<AndroidBackend>>,
        world: World,
    }

    /// Mount a new-core element tree into an already-Activity-wired
    /// backend.
    ///
    /// The JNI wrapper must have: constructed the backend
    /// (`AndroidBackend::new(context, root)`), installed the global
    /// self-handle (`install_global_self`) and the main-looper
    /// scheduler (`install_scheduler` — the flush driver rides it), and
    /// optionally the render loop. See the module docs for the exact
    /// sequence and the generated `NativeBridge.attach` (new-core
    /// branch) for the canonical caller.
    ///
    /// `register` runs after [`runtime_vocabulary::register_builtins`],
    /// so apps/SDKs can register their own payload handlers on the same
    /// registry before the tree realizes. The build closure runs inside
    /// `world.enter`, so free `signal()`/`effect()` calls work;
    /// top-level creations are world-root-owned (they live until
    /// [`stop`] / process teardown).
    pub fn start(
        backend: Rc<RefCell<AndroidBackend>>,
        register: impl FnOnce(&mut Registry<AndroidBackend>),
        build: impl FnOnce() -> Element,
    ) {
        // Idempotent like the old attach's `OWNER.take()`: a re-attach
        // without an intervening detach tears the previous mount down
        // FIRST — otherwise the prior `Realized`/`World` would only
        // drop on slot overwrite below, after the new tree already
        // realized, and its pending scope cleanups would fire under
        // the new mount's feet. (Activity recreation normally routes
        // detach → attach, but the old wrapper tolerated bare
        // re-attach and this path must too.)
        stop();

        // Monotonic clock (step 1 in the module docs) — the Android
        // analogue of web `start_in`'s `install_time_source`.
        // Idempotent, first install wins. The old `mount` preamble's
        // other ambient installs live in a runtime-core-private module
        // and are NOT reachable from a backend crate — same situation
        // as the web/macOS new-core boots (later-phase seam).
        let platform = backend.borrow().platform();
        runtime_core::time::install_default_time_source(platform);

        let mut registry: Registry<AndroidBackend> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        register(&mut registry);
        let registry = Rc::new(registry);

        let world = World::new();
        let realized = world.enter(|| {
            let element = build();
            realize(&backend, &registry, element)
        });

        // Step 3: no-op on this backend (the Android scheduler never
        // buffers microtasks) — kept for boot-sequence symmetry with
        // web/macOS. Must run with NO backend borrow held.
        runtime_core::scheduling::drain_buffered_microtasks();

        // Single-root contract, matching the old-core mount: `finish`
        // appends the root view into the host ViewGroup and schedules
        // the retrying first layout pass.
        let mut roots = realized.collect_nodes();
        let root = match roots.len() {
            1 => roots.pop().expect("len checked"),
            n => panic!(
                "backend_android::newcore::start: the app root must contribute exactly one \
                 top-level node (got {n}) — wrap fragment/multi-root trees in a view"
            ),
        };
        Backend::finish(&mut *backend.borrow_mut(), root);

        // Commit anything staged during mount before the first paint.
        world.flush();

        // Install the flush driver: schedule_flush becomes reachable
        // from (a) the author-callback wrappers in the caps impls below
        // and (b) the scheduler/executor post-dispatch hook.
        crate::dispatch_hook::install_dispatch_hook(schedule_flush);
        set_flush_world(Some(world.clone()));
        APP.with(|slot| {
            *slot.borrow_mut() = Some(App {
                realized,
                _backend: backend,
                _registry: registry,
                world,
            })
        });
    }

    /// Unmount the app started by [`start`]: drops the `Realized`
    /// (cleanups fire, views detach from the live tree's point of
    /// view), uninstalls the flush hook, and drops the world. Called by
    /// the JNI wrapper's `detach` (new-core branch).
    pub fn stop() {
        crate::dispatch_hook::clear_dispatch_hook();
        set_flush_world(None);
        APP.with(|slot| {
            if let Some(app) = slot.borrow_mut().take() {
                // Explicit for readability; struct field order
                // guarantees the same sequence.
                let App { realized, _backend, _registry, world } = app;
                drop(realized);
                drop(world);
            }
        });
    }

    /// Borrow the mounted app's live tree (tests, diagnostics).
    pub fn with_realized<R>(f: impl FnOnce(&Realized<GlobalRef>) -> R) -> Option<R> {
        APP.with(|slot| slot.borrow().as_ref().map(|app| f(&app.realized)))
    }

    /// Run `f` with the mounted world (tests can flush it explicitly).
    pub fn with_world<R>(f: impl FnOnce(&World) -> R) -> Option<R> {
        APP.with(|slot| slot.borrow().as_ref().map(|app| f(&app.world)))
    }

    /// Total live `View`s under the backend's host root, counted from
    /// the REAL Android view hierarchy over JNI (`ViewGroup.getChildAt`
    /// recursion, host root inclusive) — proof that realize/finish
    /// attached real views, not just Rust-side bookkeeping. The smoke
    /// app's self-test logs this next to its committed-write check
    /// (mirrors `newcore-macos-smoke`'s NSView count). `0` when no
    /// backend is installed (or the count genuinely is empty).
    pub fn live_view_count() -> usize {
        let Some(weak) = imp::backend_self_weak() else {
            return 0;
        };
        let Some(rc) = weak.upgrade() else {
            return 0;
        };
        let root = rc.borrow().host_root().clone();
        imp::with_env(|env| count_views(env, root.as_obj()))
    }

    fn count_views(env: &mut jni::JNIEnv, view: &JObject) -> usize {
        let mut total = 1usize;
        let is_group = env
            .is_instance_of(view, "android/view/ViewGroup")
            .unwrap_or(false);
        if !is_group {
            return total;
        }
        let n = env
            .call_method(view, "getChildCount", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        for i in 0..n {
            if let Ok(child) = env
                .call_method(view, "getChildAt", "(I)Landroid/view/View;", &[JValue::Int(i)])
                .and_then(|v| v.l())
            {
                total += count_views(env, &child);
            }
        }
        total
    }

    // =======================================================================
    // Host + capability-trait delegation (generated from
    // runtime_vocabulary::bridge — keep mechanically in sync; the
    // scene-parity goldens + the AllCaps bound on register_builtins are
    // the compile gates)
    // =======================================================================

    // -----------------------------------------------------------------------
    // Host — the P1 structural seam
    // -----------------------------------------------------------------------

    impl Host for AndroidBackend {
        type Node = GlobalRef;

        fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
            <AndroidBackend as Backend>::insert(self, parent, child)
        }

        fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
            <AndroidBackend as Backend>::insert_many(self, parent, children)
        }

        fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
            <AndroidBackend as Backend>::insert_at(self, parent, child, index)
        }

        fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
            <AndroidBackend as Backend>::remove_child(self, parent, child)
        }

        fn clear_children(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::clear_children(self, node)
        }

        fn create_anchor(&mut self) -> Self::Node {
            <AndroidBackend as Backend>::create_reactive_anchor(self)
        }

        fn supports_splice(&self) -> bool {
            <AndroidBackend as Backend>::supports_child_splice(self)
        }
    }

    // -----------------------------------------------------------------------
    // App environment + lifecycle
    // -----------------------------------------------------------------------

    impl caps::AppEnvOps for AndroidBackend {
        fn color_scheme(&self) -> ColorScheme {
            <AndroidBackend as Backend>::color_scheme(self)
        }

        fn platform(&self) -> Platform {
            <AndroidBackend as Backend>::platform(self)
        }

        fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
            <AndroidBackend as Backend>::url_opener(self)
        }

        fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
            <AndroidBackend as Backend>::fullscreen_setter(self)
        }

        fn set_page_metadata(&mut self, meta: &PageMetadata) {
            <AndroidBackend as Backend>::set_page_metadata(self, meta)
        }

        fn set_app_background(&mut self, color: &Tokenized<Color>) {
            <AndroidBackend as Backend>::set_app_background(self, color)
        }

        fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
            <AndroidBackend as Backend>::set_scrollbar_theme(self, thumb, track)
        }

        fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
            // Dispatch-site glue: app-level key handlers run author code.
            let handler = handler.map(flushing_key);
            <AndroidBackend as Backend>::set_app_key_handler(self, handler)
        }
    }

    impl caps::LifecycleOps for AndroidBackend {
        fn finish(&mut self, root: Self::Node) {
            <AndroidBackend as Backend>::finish(self, root)
        }

        fn run_layout(&mut self) {
            <AndroidBackend as Backend>::run_layout(self)
        }

        fn schedule_layout_pass() {
            <AndroidBackend as Backend>::schedule_layout_pass()
        }

        fn is_hydrating(&self) -> bool {
            <AndroidBackend as Backend>::is_hydrating(self)
        }

        fn renders_lazy_chunks(&self) -> bool {
            <AndroidBackend as Backend>::renders_lazy_chunks(self)
        }
    }

    // -----------------------------------------------------------------------
    // View + input + pressable
    // -----------------------------------------------------------------------

    impl caps::ViewOps for AndroidBackend {
        fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
            <AndroidBackend as Backend>::create_view(self, a11y)
        }

        fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
            <AndroidBackend as Backend>::make_view_handle(self, node)
        }
    }

    impl caps::InputOps for AndroidBackend {
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
            <AndroidBackend as Backend>::install_touch_handler(self, node, handler)
        }

        fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
            <AndroidBackend as Backend>::claim_touch(self, node, touch_id)
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
            <AndroidBackend as Backend>::install_wheel_handler(self, node, handler)
        }

        fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
            <AndroidBackend as Backend>::install_hover_handler(self, node, flushing1(handler))
        }

        fn mark_preserves_focus(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::mark_preserves_focus(self, node)
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
            <AndroidBackend as Backend>::install_file_drop_handler(self, node, handler)
        }
    }

    impl caps::PressableOps for AndroidBackend {
        fn create_pressable(
            &mut self,
            on_click: Rc<dyn Fn()>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <AndroidBackend as Backend>::create_pressable(self, flushing0(on_click), a11y)
        }

        fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
            <AndroidBackend as Backend>::make_pressable_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Text + button
    // -----------------------------------------------------------------------

    impl caps::TextOps for AndroidBackend {
        fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
            <AndroidBackend as Backend>::create_text(self, content, a11y)
        }

        fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
            <AndroidBackend as Backend>::create_styled_text(self, runs, a11y)
        }

        fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
            <AndroidBackend as Backend>::update_styled_text(self, node, runs)
        }

        fn update_text(&mut self, node: &Self::Node, content: &str) {
            <AndroidBackend as Backend>::update_text(self, node, content)
        }

        fn create_text_with_id(
            &mut self,
            content: &str,
            a11y: &AccessibilityProps,
        ) -> Option<(Self::Node, u32)> {
            <AndroidBackend as Backend>::create_text_with_id(self, content, a11y)
        }

        fn update_text_by_id(&mut self, id: u32, content: String) {
            <AndroidBackend as Backend>::update_text_by_id(self, id, content)
        }

        fn release_text_id(&mut self, id: u32) {
            <AndroidBackend as Backend>::release_text_id(self, id)
        }

        fn supports_js_text_bindings(&self) -> bool {
            <AndroidBackend as Backend>::supports_js_text_bindings(self)
        }

        fn register_reactive_text_binding(
            &mut self,
            text_id: u32,
            signal_ids: &[u64],
            template_parts: &[&str],
            initial_values: &[&str],
            stringifiers: &[Rc<dyn Fn() -> String>],
        ) {
            <AndroidBackend as Backend>::register_reactive_text_binding(
                self,
                text_id,
                signal_ids,
                template_parts,
                initial_values,
                stringifiers,
            )
        }

        fn release_reactive_text_binding(&mut self, text_id: u32) {
            <AndroidBackend as Backend>::release_reactive_text_binding(self, text_id)
        }

        fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
            <AndroidBackend as Backend>::make_text_handle(self, node)
        }
    }

    impl caps::ButtonOps for AndroidBackend {
        fn create_button(
            &mut self,
            label: &str,
            on_click: &Action,
            leading_icon: Option<&primitives::icon::IconData>,
            trailing_icon: Option<&primitives::icon::IconData>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            // Dispatch-site glue: wrap the Action's runtime evaluator;
            // the serialization metadata passes through untouched.
            let on_click = Action {
                method: on_click.method,
                inputs: on_click.inputs.clone(),
                initial: on_click.initial.clone(),
                output: on_click.output,
                fire: flushing0(on_click.fire.clone()),
            };
            <AndroidBackend as Backend>::create_button(
                self,
                label,
                &on_click,
                leading_icon,
                trailing_icon,
                a11y,
            )
        }

        fn update_button_label(&mut self, node: &Self::Node, label: &str) {
            <AndroidBackend as Backend>::update_button_label(self, node, label)
        }

        fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
            <AndroidBackend as Backend>::make_button_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Image + icon + link
    // -----------------------------------------------------------------------

    impl caps::ImageOps for AndroidBackend {
        fn create_image(
            &mut self,
            src: &str,
            alt: Option<&str>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <AndroidBackend as Backend>::create_image(self, src, alt, a11y)
        }

        fn update_image_src(&mut self, node: &Self::Node, src: &str) {
            <AndroidBackend as Backend>::update_image_src(self, node, src)
        }

        fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
            <AndroidBackend as Backend>::update_image_alt(self, node, alt)
        }

        fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
            let handler: ImageLoadHandler = {
                let f = handler;
                Rc::new(move |ev| {
                    f(ev);
                    schedule_flush();
                })
            };
            <AndroidBackend as Backend>::install_image_load_handler(self, node, handler)
        }

        fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
            <AndroidBackend as Backend>::install_image_error_handler(self, node, flushing0(handler))
        }

        fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
            <AndroidBackend as Backend>::make_image_handle(self, node)
        }
    }

    impl caps::IconOps for AndroidBackend {
        fn create_icon(
            &mut self,
            data: &primitives::icon::IconData,
            color: Option<&Color>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <AndroidBackend as Backend>::create_icon(self, data, color, a11y)
        }

        fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
            <AndroidBackend as Backend>::update_icon_color(self, node, color)
        }

        fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
            <AndroidBackend as Backend>::update_icon_data(self, node, data)
        }

        fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
            <AndroidBackend as Backend>::update_icon_stroke(self, node, progress)
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
            <AndroidBackend as Backend>::animate_icon_stroke(
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
            <AndroidBackend as Backend>::make_icon_handle(self, node)
        }
    }

    impl caps::LinkOps for AndroidBackend {
        fn create_link(
            &mut self,
            config: primitives::link::LinkConfig,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            // Dispatch-site glue: link activation dispatches navigation
            // (stages nav-queue tick signals on the new core).
            let mut config = config;
            config.on_activate = flushing0(config.on_activate.clone());
            <AndroidBackend as Backend>::create_link(self, config, a11y)
        }

        fn update_link_url(&mut self, node: &Self::Node, url: &str) {
            <AndroidBackend as Backend>::update_link_url(self, node, url)
        }

        fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
            <AndroidBackend as Backend>::make_link_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Form widgets
    // -----------------------------------------------------------------------

    impl caps::TextInputOps for AndroidBackend {
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
            <AndroidBackend as Backend>::create_text_input(
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
            <AndroidBackend as Backend>::update_text_input_value(self, node, value)
        }

        fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
            <AndroidBackend as Backend>::update_text_input_secure(self, node, secure)
        }

        fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
            <AndroidBackend as Backend>::set_text_input_focus_handler(self, node, flushing1(handler))
        }

        fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
            <AndroidBackend as Backend>::update_text_input_placeholder(self, node, placeholder)
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
            <AndroidBackend as Backend>::create_text_area(
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
            <AndroidBackend as Backend>::update_text_area_value(self, node, value)
        }

        fn make_text_input_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::text_input::TextInputHandle {
            <AndroidBackend as Backend>::make_text_input_handle(self, node)
        }

        fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
            <AndroidBackend as Backend>::make_text_area_handle(self, node)
        }
    }

    impl caps::ToggleOps for AndroidBackend {
        fn create_toggle(
            &mut self,
            initial_value: bool,
            on_change: Rc<dyn Fn(bool)>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <AndroidBackend as Backend>::create_toggle(self, initial_value, flushing1(on_change), a11y)
        }

        fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
            <AndroidBackend as Backend>::update_toggle_value(self, node, value)
        }

        fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
            <AndroidBackend as Backend>::make_toggle_handle(self, node)
        }
    }

    impl caps::SliderOps for AndroidBackend {
        fn create_slider(
            &mut self,
            initial_value: f32,
            min: f32,
            max: f32,
            step: Option<f32>,
            on_change: Rc<dyn Fn(f32)>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <AndroidBackend as Backend>::create_slider(
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
            <AndroidBackend as Backend>::update_slider_value(self, node, value)
        }

        fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
            <AndroidBackend as Backend>::make_slider_handle(self, node)
        }
    }

    impl caps::ActivityIndicatorOps for AndroidBackend {
        fn create_activity_indicator(
            &mut self,
            size: primitives::activity_indicator::ActivityIndicatorSize,
            color: Option<&Color>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <AndroidBackend as Backend>::create_activity_indicator(self, size, color, a11y)
        }

        fn update_activity_indicator_size(
            &mut self,
            node: &Self::Node,
            size: primitives::activity_indicator::ActivityIndicatorSize,
        ) {
            <AndroidBackend as Backend>::update_activity_indicator_size(self, node, size)
        }

        fn make_activity_indicator_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::activity_indicator::ActivityIndicatorHandle {
            <AndroidBackend as Backend>::make_activity_indicator_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Scroll + safe area + virtualizer
    // -----------------------------------------------------------------------

    impl caps::ScrollOps for AndroidBackend {
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
            <AndroidBackend as Backend>::create_scroll_view(self, horizontal, on_scroll, a11y)
        }

        fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
            <AndroidBackend as Backend>::node_scroll(self, node)
        }

        fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
            <AndroidBackend as Backend>::set_node_scroll(self, node, x, y)
        }

        fn make_scroll_view_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::scroll_view::ScrollViewHandle {
            <AndroidBackend as Backend>::make_scroll_view_handle(self, node)
        }
    }

    impl caps::SafeAreaOps for AndroidBackend {
        fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
            <AndroidBackend as Backend>::apply_safe_area_padding(self, node, sides)
        }

        fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
            <AndroidBackend as Backend>::apply_scroll_view_safe_area_inset(self, node, sides)
        }
    }

    impl caps::VirtualizerOps for AndroidBackend {
        fn create_virtualizer(
            &mut self,
            callbacks: VirtualizerCallbacks<Self::Node>,
            overscan: f32,
            layout: primitives::virtualizer::VirtualLayout,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            // Dispatch-site glue: mount/release run author render
            // closures and scope cleanups (which may stage writes) from
            // the backend's own scroll handling (`RustListAdapter`);
            // measured-size reports feed the handler's layout cache.
            // item_count/item_key/item_size are pure reads and stay
            // unwrapped.
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
                        let mounted = f(i);
                        schedule_flush();
                        mounted
                    })
                },
                release_item: {
                    let f = release_item;
                    Rc::new(move |scope_id| {
                        f(scope_id);
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
            <AndroidBackend as Backend>::create_virtualizer(self, callbacks, overscan, layout, a11y)
        }

        fn virtualizer_data_changed(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::virtualizer_data_changed(self, node)
        }

        fn release_virtualizer(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::release_virtualizer(self, node)
        }

        fn make_virtualizer_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::virtualizer::VirtualizerHandle {
            <AndroidBackend as Backend>::make_virtualizer_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Graphics + portal + presence + navigator
    // -----------------------------------------------------------------------

    impl caps::GraphicsOps for AndroidBackend {
        fn create_graphics(
            &mut self,
            on_ready: primitives::graphics::OnReady,
            on_resize: primitives::graphics::OnResize,
            on_lost: primitives::graphics::OnLost,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            // Dispatch-site glue: surface lifecycle callbacks run
            // author code (draw-scene setup that creates/sets signals).
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
            <AndroidBackend as Backend>::create_graphics(self, on_ready, on_resize, on_lost, a11y)
        }

        fn release_graphics(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::release_graphics(self, node)
        }

        fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
            <AndroidBackend as Backend>::make_graphics_handle(self, node)
        }
    }

    impl caps::PortalOps for AndroidBackend {
        fn create_portal(
            &mut self,
            target: primitives::portal::PortalTarget,
            on_dismiss: Option<Rc<dyn Fn()>>,
            trap_focus: bool,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            // Dispatch-site glue: back-press / outside-tap dismissal
            // runs the author's on_dismiss.
            let on_dismiss = on_dismiss.map(flushing0);
            <AndroidBackend as Backend>::create_portal(self, target, on_dismiss, trap_focus, a11y)
        }

        fn release_portal(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::release_portal(self, node)
        }

        fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
            <AndroidBackend as Backend>::set_portal_hidden(self, node, hidden)
        }

        fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
            <AndroidBackend as Backend>::make_portal_handle(self, node)
        }
    }

    impl caps::PresenceOps for AndroidBackend {
        fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
            <AndroidBackend as Backend>::create_presence_placeholder(self, a11y)
        }

        fn apply_presence(
            &mut self,
            node: &Self::Node,
            state: primitives::presence::PresenceState,
            transition: Option<(u32, Easing)>,
        ) {
            <AndroidBackend as Backend>::apply_presence(self, node, state, transition)
        }

        fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
            <AndroidBackend as Backend>::make_presence_handle(self, node)
        }
    }

    impl caps::NavigatorOps for AndroidBackend {
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
            // does not route through (the vocabulary navigator handlers
            // own screens; dispatch stages commands + a tick signal and
            // the driver effect commits inside the flush). Author-
            // initiated navigation stages via handlers already wrapped
            // above. Android's system back (native push surfaces) is a
            // named P5 residual in the vocabulary navigator module docs.
            <AndroidBackend as Backend>::create_navigator(
                self,
                type_id,
                type_name,
                presentation,
                host,
                a11y,
            )
        }

        fn release_navigator(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::release_navigator(self, node)
        }

        fn apply_navigator_slot_style(
            &mut self,
            node: &Self::Node,
            slot: &'static str,
            style: &Rc<StyleRules>,
        ) {
            <AndroidBackend as Backend>::apply_navigator_slot_style(self, node, slot, style)
        }

        fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
            <AndroidBackend as Backend>::make_navigator_handle(self, node)
        }

        fn navigator_attach_initial(
            &mut self,
            navigator: &Self::Node,
            screen: Self::Node,
            scope_id: u64,
            options: Box<dyn Any>,
        ) {
            <AndroidBackend as Backend>::navigator_attach_initial(
                self, navigator, screen, scope_id, options,
            )
        }
    }

    // -----------------------------------------------------------------------
    // External + document
    // -----------------------------------------------------------------------

    impl caps::ExternalOps for AndroidBackend {
        fn create_external(
            &mut self,
            type_id: TypeId,
            type_name: &'static str,
            payload: &Rc<dyn Any>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <AndroidBackend as Backend>::create_external(self, type_id, type_name, payload, a11y)
        }

        fn release_external(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::release_external(self, node)
        }

        fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
            <AndroidBackend as Backend>::missing_primitive_placeholder(self, label)
        }
    }

    impl caps::DocumentOps for AndroidBackend {
        fn create_element(&mut self, tag: &str) -> Self::Node {
            <AndroidBackend as Backend>::create_element(self, tag)
        }

        fn attach_html_id(&self, node: &Self::Node, id: &str) {
            <AndroidBackend as Backend>::attach_html_id(self, node, id)
        }

        fn attach_html_class(&self, node: &Self::Node, class: &str) {
            <AndroidBackend as Backend>::attach_html_class(self, node, class)
        }

        fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
            <AndroidBackend as Backend>::attach_html_style(self, node, prop, value)
        }

        fn register_raw_css(&mut self, css: &str) {
            <AndroidBackend as Backend>::register_raw_css(self, css)
        }
    }

    // -----------------------------------------------------------------------
    // Style + assets
    // -----------------------------------------------------------------------

    impl caps::StyleOps for AndroidBackend {
        fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
            <AndroidBackend as Backend>::apply_style(self, node, style)
        }

        fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
            <AndroidBackend as Backend>::mint_style_class(self, style)
        }

        fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
            <AndroidBackend as Backend>::mint_class_for_app(self, app)
        }

        fn apply_styled_states(
            &mut self,
            node: &Self::Node,
            base: &Rc<StyleRules>,
            overlays: &[(StateBits, Rc<StyleRules>)],
        ) {
            <AndroidBackend as Backend>::apply_styled_states(self, node, base, overlays)
        }

        fn apply_styled_variants(
            &mut self,
            node: &Self::Node,
            base: &Rc<StyleRules>,
            state_overlays: &[(StateBits, Rc<StyleRules>)],
            breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
            container_overlays: &[(f32, Rc<StyleRules>)],
        ) {
            <AndroidBackend as Backend>::apply_styled_variants(
                self,
                node,
                base,
                state_overlays,
                breakpoint_overlays,
                container_overlays,
            )
        }

        fn mark_container(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::mark_container(self, node)
        }

        fn handles_states_natively(&self) -> bool {
            <AndroidBackend as Backend>::handles_states_natively(self)
        }

        fn token_updates_propagate_via_cascade(&self) -> bool {
            <AndroidBackend as Backend>::token_updates_propagate_via_cascade(self)
        }

        fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
            <AndroidBackend as Backend>::register_stylesheet(self, rules)
        }

        fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
            <AndroidBackend as Backend>::unregister_stylesheet(self, rules)
        }

        fn install_tokens(&mut self, tokens: &[TokenEntry]) {
            <AndroidBackend as Backend>::install_tokens(self, tokens)
        }

        fn update_tokens(&mut self, tokens: &[TokenEntry]) {
            <AndroidBackend as Backend>::update_tokens(self, tokens)
        }

        fn on_node_unstyled(&mut self, node: &Self::Node) {
            <AndroidBackend as Backend>::on_node_unstyled(self, node)
        }

        fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
            // Dispatch-site glue: hover/press/focus state flips arrive
            // via `RustStateListener` and can stage writes when the
            // style path routes states through signals (the
            // static-style divert sends state-overlay nodes through the
            // reactive path on native — see
            // [[project_static_style_state_machine_divert]]).
            let setter: Rc<dyn Fn(StateBits, bool)> = {
                let f = setter;
                Rc::new(move |bits, on| {
                    f(bits, on);
                    schedule_flush();
                })
            };
            <AndroidBackend as Backend>::attach_states(self, node, setter)
        }

        fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
            <AndroidBackend as Backend>::set_disabled(self, node, disabled)
        }

        fn supports_preminted_styles(&self) -> bool {
            <AndroidBackend as Backend>::supports_preminted_styles(self)
        }

        fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {
            <AndroidBackend as Backend>::apply_default_text_font(self, font)
        }

        fn supports_js_class_bindings(&self) -> bool {
            <AndroidBackend as Backend>::supports_js_class_bindings(self)
        }

        fn register_reactive_class_binding(
            &mut self,
            node: &Self::Node,
            signal_id: u64,
            values: &[u32],
            classes: &[&str],
            value_reader: Rc<dyn Fn() -> u32>,
        ) -> u32 {
            <AndroidBackend as Backend>::register_reactive_class_binding(
                self,
                node,
                signal_id,
                values,
                classes,
                value_reader,
            )
        }

        fn release_reactive_class_binding(&mut self, binding_id: u32) {
            <AndroidBackend as Backend>::release_reactive_class_binding(self, binding_id)
        }
    }

    impl caps::AssetOps for AndroidBackend {
        fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
            <AndroidBackend as Backend>::register_asset(self, id, kind, source)
        }

        fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
            <AndroidBackend as Backend>::unregister_asset(self, id, kind)
        }

        fn register_typeface(
            &mut self,
            id: TypefaceId,
            family_name: &str,
            faces: &[TypefaceFace],
            fallback: SystemFallback,
        ) {
            <AndroidBackend as Backend>::register_typeface(self, id, family_name, faces, fallback)
        }

        fn unregister_typeface(&mut self, id: TypefaceId) {
            <AndroidBackend as Backend>::unregister_typeface(self, id)
        }
    }

    // -----------------------------------------------------------------------
    // A11y + animation + introspection
    // -----------------------------------------------------------------------

    impl caps::A11yOps for AndroidBackend {
        fn update_accessibility(
            &mut self,
            node: &Self::Node,
            a11y: &AccessibilityProps,
            inferred_role: Option<Role>,
        ) {
            <AndroidBackend as Backend>::update_accessibility(self, node, a11y, inferred_role)
        }

        fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
            <AndroidBackend as Backend>::announce_for_accessibility(self, msg, priority)
        }

        fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
            <AndroidBackend as Backend>::dump_accessibility_tree(self)
        }
    }

    impl caps::AnimationOps for AndroidBackend {
        fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
            <AndroidBackend as Backend>::set_animated_f32(self, node, prop, value)
        }

        fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
            <AndroidBackend as Backend>::set_animated_color(self, node, prop, value)
        }
    }

    impl caps::IntrospectionOps for AndroidBackend {
        fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
            <AndroidBackend as Backend>::frame(self, node)
        }

        fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
            <AndroidBackend as Backend>::absolute_frame(self, node)
        }

        fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
            <AndroidBackend as Backend>::device_frame(self, node)
        }

        fn supports_native_introspection(&self) -> bool {
            <AndroidBackend as Backend>::supports_native_introspection(self)
        }

        fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
            <AndroidBackend as Backend>::introspect_native(self, node)
        }

        fn note_introspection_root(&self, node: &Self::Node) {
            <AndroidBackend as Backend>::note_introspection_root(self, node)
        }

        fn supports_screenshot(&self) -> bool {
            <AndroidBackend as Backend>::supports_screenshot(self)
        }

        fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
            <AndroidBackend as Backend>::capture_screenshot(self, done)
        }
    }

    // -----------------------------------------------------------------------
    // Batch + wire bindings
    // -----------------------------------------------------------------------

    impl caps::BatchOps for AndroidBackend {
        fn supports_batched_repeat(&self) -> bool {
            <AndroidBackend as Backend>::supports_batched_repeat(self)
        }

        fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
            <AndroidBackend as Backend>::execute_batch(self, batch)
        }

        fn execute_batch_with_attach(
            &mut self,
            batch: BackendBatch,
            parent: &mut Self::Node,
            attach_locals: &[u32],
        ) -> Vec<Self::Node> {
            <AndroidBackend as Backend>::execute_batch_with_attach(self, batch, parent, attach_locals)
        }
    }

    impl caps::WireBindingOps for AndroidBackend {
        fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
            <AndroidBackend as Backend>::note_text_binding(self, node, signal_ids, method)
        }

        fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value) {
            <AndroidBackend as Backend>::note_signal_initial(self, signal_id, value)
        }

        fn note_when_binding(
            &mut self,
            anchor: &Self::Node,
            signal_ids: &[u64],
            cond_method: &'static str,
            then_node: &Self::Node,
            otherwise_node: &Self::Node,
        ) {
            <AndroidBackend as Backend>::note_when_binding(
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
            <AndroidBackend as Backend>::note_switch_binding(
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
            <AndroidBackend as Backend>::note_repeat_binding(
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
            <AndroidBackend as Backend>::note_virtualizer_binding(
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
            <AndroidBackend as Backend>::supports_lazy_slot_capture(self)
        }

        fn begin_slot_capture(&mut self) {
            <AndroidBackend as Backend>::begin_slot_capture(self)
        }

        fn end_slot_capture(&mut self, slot_root: &Self::Node) {
            <AndroidBackend as Backend>::end_slot_capture(self, slot_root)
        }
    }
}

#[cfg(target_os = "android")]
pub use native::{
    live_view_count, start, stop, with_realized, with_world, SceneElement, SceneRegistry,
};

// ===========================================================================
// Host tests — the flush driver + dispatch-site glue contract. These
// run on the cargo-test thread from ANY platform (`cargo test -p
// backend-android-mobile --features new-core`): they exercise the
// scheduler-facing glue only. The Host/caps delegation and the boot
// path are exercised by building + launching `newcore-android-smoke`
// (real Android Views only exist inside an Android process — same
// limitation as every `imp/` module; see the smoke crate) and by
// `cargo check --target aarch64-linux-android --features new-core`,
// which type-checks the full android-gated half.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::scheduling::{ScheduleHandle, Scheduler};
    use runtime_world::{effect, signal};
    use std::collections::VecDeque;

    // A queuing scheduler standing in for the Android main looper:
    // `schedule_microtask` queues (like `Handler.post`) instead of the
    // no-scheduler synchronous fallback, so the tests exercise the REAL
    // contract — stage during dispatch, commit on a later looper turn.
    // Unit struct + thread-local queue mirrors `AndroidScheduler`'s
    // shape (per-thread queues also keep parallel test threads
    // isolated). Installed process-wide (first install wins) — fine:
    // every test in this crate that schedules anything drains
    // explicitly via `pump`.
    thread_local! {
        static QUEUE: std::cell::RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
            std::cell::RefCell::new(VecDeque::new());
    }

    struct LooperStandIn;
    unsafe impl Send for LooperStandIn {}
    unsafe impl Sync for LooperStandIn {}

    impl Scheduler for LooperStandIn {
        fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
            QUEUE.with(|q| q.borrow_mut().push_back(f));
        }
        fn after_animation_frame(&self, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            QUEUE.with(|q| q.borrow_mut().push_back(f));
            Box::new(Inert)
        }
        fn after_ms(&self, _delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
            QUEUE.with(|q| q.borrow_mut().push_back(f));
            Box::new(Inert)
        }
        fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
            Box::new(Inert)
        }
    }

    struct Inert;
    impl ScheduleHandle for Inert {
        fn cancel(&mut self) {}
    }

    fn install_test_scheduler() {
        runtime_core::scheduling::install_scheduler(Box::new(LooperStandIn));
    }

    /// Drain the fake looper queue (including tasks queued by drained
    /// tasks) — one "turn of the main looper".
    fn pump() {
        loop {
            let task = QUEUE.with(|q| q.borrow_mut().pop_front());
            match task {
                Some(f) => f(),
                None => break,
            }
        }
    }

    /// `schedule_flush` queues exactly one deduped microtask; staged
    /// writes commit when the looper turn drains it — the exact
    /// stage-during-dispatch / commit-at-the-boundary contract the
    /// dispatch-site glue relies on.
    #[test]
    fn schedule_flush_dedups_and_commits_on_looper_turn() {
        install_test_scheduler();
        let world = World::new();
        set_flush_world(Some(world.clone()));

        let log: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
        let count = world.enter(|| {
            let count = signal(0i32);
            let log = log.clone();
            effect(move || log.borrow_mut().push(count.get()));
            count
        });
        assert_eq!(*log.borrow(), vec![0], "effect ran once at creation");

        // Stage twice, schedule twice: the second schedule_flush must
        // dedup (flag already set), and nothing commits until the
        // looper turn.
        count.set(1);
        count.set(2);
        schedule_flush();
        assert!(FLUSH.with(|f| f.queued.get()), "first call arms the flag");
        schedule_flush();
        assert_eq!(
            QUEUE.with(|q| q.borrow().len()),
            1,
            "second call deduped — ONE posted flush runnable"
        );
        assert_eq!(*log.borrow(), vec![0], "staged, not committed");

        pump();
        assert_eq!(
            *log.borrow(),
            vec![0, 2],
            "ONE flush committed the latest staged value"
        );
        assert!(!FLUSH.with(|f| f.queued.get()), "drain disarms the dedup flag");

        // The flag re-arms for the next write→flush cycle.
        count.set(3);
        schedule_flush();
        pump();
        assert_eq!(*log.borrow(), vec![0, 2, 3]);

        set_flush_world(None);
    }

    /// `flush_now` with no mounted world is a no-op (the dispatch hook
    /// can fire before `start` finishes wiring), and a re-entrant flush
    /// is skipped via `world.is_flushing()`.
    #[test]
    fn flush_now_tolerates_no_world_and_reentry() {
        install_test_scheduler();
        set_flush_world(None);
        flush_now(); // must not panic

        let world = World::new();
        set_flush_world(Some(world.clone()));
        let observed: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let (sig, obs) = world.enter(|| {
            let sig = signal(0i32);
            let obs = observed.clone();
            effect(move || {
                let v = sig.get();
                if v == 1 {
                    // Re-entrant flush attempt from inside a flush:
                    // world.is_flushing() short-circuits it.
                    flush_now();
                    obs.set(true);
                }
            });
            (sig, observed.clone())
        });
        sig.set(1);
        flush_now();
        assert!(obs.get(), "effect ran; re-entrant flush_now didn't recurse/panic");
        set_flush_world(None);
    }

    /// The author-callback wrappers call the author fn FIRST, then
    /// queue the flush — a wrapped `on_press` that stages a write is
    /// committed by the following looper turn without any other event.
    /// This is the caps-seam contract; it fails if the wrap-then-flush
    /// ordering is removed.
    #[test]
    fn flushing_wrappers_commit_staged_writes_after_author_returns() {
        install_test_scheduler();
        let world = World::new();
        set_flush_world(Some(world.clone()));

        let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let (pressed, typed) = world.enter(|| {
            let pressed = signal(false);
            let typed = signal(String::new());
            let seen = seen.clone();
            effect(move || {
                seen.borrow_mut()
                    .push(format!("{}:{}", pressed.get(), typed.get()));
            });
            (pressed, typed)
        });
        assert_eq!(seen.borrow().len(), 1);

        // flushing0: the shape create_pressable/on_dismiss use.
        let on_press = flushing0(Rc::new(move || pressed.set(true)));
        on_press();
        assert_eq!(seen.borrow().len(), 1, "staged during dispatch, not committed");
        pump();
        assert_eq!(seen.borrow().last().unwrap(), "true:", "committed on the turn");

        // flushing1: the on_change(String) shape.
        let on_change = flushing1(Rc::new(move |v: String| typed.set(v)));
        on_change("hi".to_string());
        pump();
        assert_eq!(seen.borrow().last().unwrap(), "true:hi");

        set_flush_world(None);
    }

    /// `flushing_key` preserves the author's outcome (the backend's
    /// consume/pass-through decision must be unchanged by the glue)
    /// while still queueing the flush.
    #[test]
    fn flushing_key_passes_outcome_through() {
        install_test_scheduler();
        let world = World::new();
        set_flush_world(Some(world.clone()));
        let hits = world.enter(|| signal(0i32));

        let handler: runtime_core::primitives::key::KeyDownHandler = {
            Rc::new(move |_ev| {
                hits.update(|n| n + 1);
                runtime_core::primitives::key::KeyOutcome::PreventDefault
            })
        };
        let wrapped = flushing_key(handler);
        let ev = runtime_core::primitives::key::KeyEvent {
            key: "a".to_string(),
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
            selection_start: 0,
            selection_end: 0,
        };
        let outcome = wrapped(&ev);
        assert!(
            matches!(
                outcome,
                runtime_core::primitives::key::KeyOutcome::PreventDefault
            ),
            "outcome must pass through the glue unchanged"
        );
        pump();
        assert_eq!(
            world.enter(|| hits.get()),
            1,
            "staged update committed by the queued flush"
        );
        set_flush_world(None);
    }

    /// The dispatch hook wired to `schedule_flush` (what `start`
    /// installs) commits writes staged from a timer-shaped callback —
    /// the scheduler fire-site contract. Regression for "my after_ms
    /// callback ran but the UI never updated".
    #[test]
    fn dispatch_hook_commits_timer_staged_writes() {
        install_test_scheduler();
        let world = World::new();
        set_flush_world(Some(world.clone()));
        crate::dispatch_hook::install_dispatch_hook(schedule_flush);

        let ticks: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
        let counter = world.enter(|| {
            let counter = signal(0i32);
            let ticks = ticks.clone();
            effect(move || ticks.borrow_mut().push(counter.get()));
            counter
        });

        // Simulate what imp/scheduler.rs does for an `after_ms`
        // callback: run the author closure, then fire the hook.
        let author_timer = move || counter.set(7);
        author_timer();
        crate::dispatch_hook::fire_dispatch_hook();
        assert_eq!(*ticks.borrow(), vec![0], "hook queues — commit is next turn");
        pump();
        assert_eq!(*ticks.borrow(), vec![0, 7], "hook-driven flush committed");

        crate::dispatch_hook::clear_dispatch_hook();
        set_flush_world(None);
    }

    /// `is_booted` / `flush_sync` / `with_world_entered` are safe both
    /// sides of a mount — the JNI-interop seam surface.
    #[test]
    fn interop_seams_safe_before_and_after_boot() {
        install_test_scheduler();
        set_flush_world(None);
        assert!(!is_booted());
        flush_sync(); // no-op, must not panic
        assert!(with_world_entered(|| 1).is_none());

        let world = World::new();
        set_flush_world(Some(world.clone()));
        assert!(is_booted());
        let sig = with_world_entered(|| signal(5i32)).expect("world ambient");
        sig.set(6);
        flush_sync();
        assert_eq!(world.enter(|| sig.get()), 6, "flush_sync committed synchronously");
        set_flush_world(None);
        assert!(!is_booted());
    }
}

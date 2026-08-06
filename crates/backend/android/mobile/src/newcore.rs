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
//! Sequence (mirrors `runtime_shared::mount`'s ordering where they
//! overlap):
//!
//! 1. Install the default monotonic time source (the Android analogue
//!    of web `start_in`'s `install_time_source` — without it the
//!    animation clock and `PhaseTimer` read 0), then
//!    `runtime_vocabulary::backend::install_env_services` for the
//!    ambient environment reads (current platform, color scheme, URL
//!    opener, full-screen setter, AX announcer). Both precede the
//!    build — a component body may read `platform()` while constructing.
//! 2. `Registry` (`register_builtins` + the `register` seam) + `World`
//!    + `world.enter(realize)`.
//! 3. `runtime_shared::scheduling::drain_buffered_microtasks()` — a
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
//!    `runtime_shared::scheduling::schedule_microtask`, which on this
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
//! - ~~Viewport resize~~ — WIRED: the deferred viewport mirror in
//!   `imp::viewport_size()` (the seam `RustViewportResizeListener`'s
//!   layout pass funnels into) now pushes BOTH sinks — the old-core
//!   TLS signal and, via [`forward_viewport`], the mounted world's
//!   `ViewportCtx` — so new-core breakpoint re-resolution follows
//!   rotation/resize live (see the "Viewport source" section).
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

use runtime_shared::primitives;
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
    runtime_shared::scheduling::schedule_microtask(|| {
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

// ===========================================================================
// Viewport source (the new-core Android resize seam — host-compilable
// half, regression-tested below)
// ===========================================================================
//
// The vocabulary's per-world viewport/breakpoint ctx
// (`runtime_vocabulary::viewport`) is SEED-ONLY unless the platform
// pushes live sizes (its module docs). This backend's resize seam is
// the deferred viewport mirror inside `viewport_size()` (imp/mod.rs —
// every layout pass samples the host `ViewGroup`, and a CHANGED size
// schedules one mirror microtask; configuration changes and rotations
// route through it because they re-measure the host). The seam keeps
// writing the shared old-core TLS value
// (`runtime_shared::set_viewport_size`) — the old core subscribes to it,
// and the world ctx SEEDS from it — and additionally calls
// [`forward_viewport`] so breakpoint-dependent author reactivity
// re-fires on rotation/resize instead of freezing at its seed.
//
// Activity recreation: `start` idempotently re-runs (stop → fresh
// world) and re-installs the sink for the NEW world's ctx, exactly like
// [`FLUSH`]; a mirror microtask that races teardown either hits a
// cleared sink (no-op) or a dead world (silent kernel no-op) — the
// P5 dead-world discipline holds.
//
// Discipline (mirrors `backend_web::newcore`'s resize listener): the
// seam runs OUTSIDE `World::enter` (a posted looper microtask), so the
// boot CAPTURES the world's signal handle — capture, don't inject —
// and the push stages through the handle (routes to its own world,
// equality-guarded) then rides one deduped [`schedule_flush`].
//
// TLS audit note: [`VIEWPORT_SINK`] is a const-init `Cell` of a `Copy`
// handle — no destructor, so it lowers to plain ELF-TLS and spends
// ZERO bionic pthread keys (same class as `dispatch_hook::HOOK`; the
// module-docs "+2 keys" budget is unchanged).

thread_local! {
    /// The mounted world's viewport signal (`Copy` handle). `None`
    /// outside a new-core boot, so the shared old-core seam costs one
    /// TLS read and nothing else when the old core is driving.
    static VIEWPORT_SINK: Cell<Option<runtime_world::Signal<runtime_shared::ViewportSize>>> =
        const { Cell::new(None) };
}

fn set_viewport_sink(sig: Option<runtime_world::Signal<runtime_shared::ViewportSize>>) {
    VIEWPORT_SINK.with(|s| s.set(sig));
}

/// Forward one platform viewport mirror into the mounted world's
/// viewport ctx (no-op before [`start`] / after [`stop`]). Called by
/// the same Android seams that write `runtime_shared::set_viewport_size`,
/// with the same dp values — the two sinks never diverge.
pub(crate) fn forward_viewport(size: runtime_shared::ViewportSize) {
    let Some(sig) = VIEWPORT_SINK.with(|s| s.get()) else {
        return;
    };
    // Staged write outside `enter` + one deduped flush — commits on
    // the next looper turn, like every wrapped callback.
    sig.set(size);
    schedule_flush();
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

/// The world mounted by [`start`] (a cheap handle clone; `None` before
/// boot / after stop).
///
/// Host-integration seam: an embedded renderer mounted INSIDE this
/// app's tree — the wgpu simulator preview
/// (`host_android_mobile::mount_newcore`) — realizes its scene into
/// this SAME world, so the app's existing flush driver (dispatch-site
/// wrappers + the scheduler/executor post-dispatch hook) commits the
/// embedded app's staged writes with no second driver: one thread, one
/// world, one logical update stream. Mirrors
/// `backend_web::newcore::mounted_world`.
pub fn mounted_world() -> Option<World> {
    FLUSH.with(|f| f.world.borrow().clone())
}

/// Run a platform-invoked vocabulary callback with the mounted world
/// ambient (`World::enter`).
///
/// WHY (bug: flat_list rendered ZERO rows on new-core web — every
/// backend shared the gap): virtualizer `mount_item` REALIZES a row
/// from the backend's own scroll/window machinery, and realization is
/// creation-side (`signal()`/`effect()`/`inject` for `ThemeCtx`), which
/// panics outside `World::enter`. Ordinary author callbacks only stage
/// writes through captured handles, so the dispatch-site glue never
/// needed entry — mount/release are the one callback family that
/// BUILDS, and the vocabulary contract (handlers/virtualizer.rs)
/// assigns the entry to the backend. Pre-boot (world slot empty — the
/// initial mount's realize) the boot's own `enter` is still ambient, so
/// a bare call is already entered; nesting `enter` is a legal stack, so
/// the ambient fallback never double-books. Host-compilable (like the
/// flushing* wrappers) so the glue tests can pin it without JNI.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn enter_mounted_world<R>(f: impl FnOnce() -> R) -> R {
    match FLUSH.with(|fl| fl.world.borrow().clone()) {
        Some(world) => world.enter(f),
        None => f(),
    }
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
    use runtime_shared::accessibility::{
        AccessibilityProps, AccessibilityTree, LiveRegionPriority, Role,
    };
    use runtime_shared::animation::AnimProp;
    use runtime_shared::assets::{
        AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId,
    };
    use runtime_shared::breakpoint::Breakpoint;
    use runtime_shared::introspect::NativeNode;
    use runtime_shared::primitives;
    use runtime_shared::primitives::portal::ViewportRect;
    use runtime_shared::styled_text::TextRun;
    use runtime_shared::{
        Action, BackendBatch, Color, ColorScheme, Easing, FileDropHandler, FontFamily,
        HoverHandler, ImageErrorHandler, ImageLoadHandler, PageMetadata, Platform, SafeAreaSides,
        Screenshot, StateBits, StyleApplication, StyleRules, TokenEntry, Tokenized, TouchHandler,
        TouchId, VirtualizerCallbacks, WheelHandler,
    };
    use runtime_scene::{realize, Element, Host, Realized, Registry};
    use runtime_vocabulary::caps;
    use runtime_world::World;

    use super::{
        flushing0, flushing1, flushing_key, schedule_flush, set_flush_world, set_viewport_sink,
    };
    use crate::imp::{self, AndroidBackend};

    // Re-exported so JNI wrappers and app crates can name the boot-path
    // types without a direct runtime-scene dependency — mirrors how
    // consumers reach the old core's `Element` through `runtime_shared`.
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
    #[inline]
    pub fn start(
        backend: Rc<RefCell<AndroidBackend>>,
        register: impl FnOnce(&mut Registry<AndroidBackend>),
        build: impl FnOnce() -> Element,
    ) {
        // `#[inline]` is load-bearing, not a perf hint: as a plain
        // non-generic `pub` fn this is codegen'd into the rlib and can
        // survive to the final link, instantiating
        // `register_builtins_with::<_, AllBuiltins>` and re-anchoring
        // the WHOLE builtin vocabulary even for an app that selected a
        // smaller set through `start_with`.
        start_with::<runtime_vocabulary::AllBuiltins, _, _>(backend, register, build)
    }

    /// [`start`], booting only the builtin primitives `S` selects.
    ///
    /// Compile-time selection: an unselected primitive's registration
    /// folds away, nothing names its handler, and the linker drops it
    /// along with the backend code it alone reached. See
    /// [`runtime_vocabulary::BuiltinSet`]. Realizing a payload this set
    /// omits panics at mount.
    pub fn start_with<S, R, B>(
        backend: Rc<RefCell<AndroidBackend>>,
        register: R,
        build: B,
    ) where
        S: runtime_vocabulary::BuiltinSet,
        R: FnOnce(&mut Registry<AndroidBackend>),
        B: FnOnce() -> Element,
    {
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
        // Idempotent, first install wins. The other ambient installs
        // ride `install_env_services` below, same as web/macOS.
        let platform = backend.borrow().platform_impl();
        runtime_shared::time::install_default_time_source(platform);

        // Ambient environment services (platform identity, color scheme, URL
        // opener, full-screen setter, AX announcer) -> the thread-locals
        // `platform()` / `open_url()` / `announce()` etc. read. MUST precede
        // the build: a component body may read `platform()` while
        // constructing. See `runtime_vocabulary::backend`.
        runtime_vocabulary::backend::install_env_services(&backend);
        let mut registry: Registry<AndroidBackend> = Registry::new();
        runtime_vocabulary::register_builtins_with::<_, S>(&mut registry);
        register(&mut registry);
        let registry = Rc::new(registry);

        let world = World::new();
        let (vp_sig, realized) = world.enter(|| {
            let element = build();
            let realized = realize(&backend, &registry, element);
            // Capture the per-world viewport ctx AFTER the build,
            // never before: the ctx's bucket memo pins the breakpoint
            // TABLE at creation and apps `install_breakpoints` inside
            // their root component (see the viewport-source section
            // and backend-web's identical ordering comment).
            let vp_sig = runtime_vocabulary::viewport::viewport_ctx().size_signal();
            (vp_sig, realized)
        });

        // Step 3: no-op on this backend (the Android scheduler never
        // buffers microtasks) — kept for boot-sequence symmetry with
        // web/macOS. Must run with NO backend borrow held.
        runtime_shared::scheduling::drain_buffered_microtasks();

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
        AndroidBackend::finish_impl(&mut *backend.borrow_mut(), root);

        // Commit anything staged during mount before the first paint.
        world.flush();

        // Install the flush driver: schedule_flush becomes reachable
        // from (a) the author-callback wrappers in the caps impls below
        // and (b) the scheduler/executor post-dispatch hook.
        crate::dispatch_hook::install_dispatch_hook(schedule_flush);
        set_flush_world(Some(world.clone()));
        // Live viewport source: the deferred mirror in
        // `imp::viewport_size()` now reaches the world's ctx through
        // `forward_viewport`. The first post-mount layout pass (the
        // retrying pass `finish` scheduled) pushes the REAL host size —
        // the ctx seeded from the pre-build TLS value, which is stale
        // on a cold boot because Android measures after `attach`
        // returns. Survives Activity recreation: this install runs
        // again for the new world (see the viewport-source section).
        set_viewport_sink(Some(vp_sig));
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
        set_viewport_sink(None);
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
            AndroidBackend::insert_impl(self, parent, child)
        }

        fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
            AndroidBackend::insert_at_impl(self, parent, child, index)
        }

        fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
            AndroidBackend::remove_child_impl(self, parent, child)
        }

        fn clear_children(&mut self, node: &Self::Node) {
            AndroidBackend::clear_children_impl(self, node)
        }

        fn create_anchor(&mut self) -> Self::Node {
            // Runtime v2: `Host::create_anchor` is REQUIRED, and this backend
            // never overrode the old `Backend::create_reactive_anchor`, whose
            // default was exactly this. Reproduced verbatim: a plain view is a
            // correct anchor on this backend (only web needs the
            // `display: contents` variant so the branch's children keep the
            // surrounding flex context). See
            // docs/runtime-v2-deletion-baseline.md §2.2.
            AndroidBackend::create_view_impl(self, &AccessibilityProps::default())
        }

        fn supports_splice(&self) -> bool {
            AndroidBackend::supports_child_splice_impl(self)
        }
    }

    // -----------------------------------------------------------------------
    // App environment + lifecycle
    // -----------------------------------------------------------------------

    impl caps::AppEnvOps for AndroidBackend {
        fn color_scheme(&self) -> ColorScheme {
            AndroidBackend::color_scheme_impl(self)
        }

        fn platform(&self) -> Platform {
            AndroidBackend::platform_impl(self)
        }

        fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
            AndroidBackend::url_opener_impl(self)
        }

        fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
            AndroidBackend::fullscreen_setter_impl(self)
        }

        fn set_app_background(&mut self, color: &Tokenized<Color>) {
            AndroidBackend::set_app_background_impl(self, color)
        }

        fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
            // Dispatch-site glue: app-level key handlers run author code.
            let handler = handler.map(flushing_key);
            AndroidBackend::set_app_key_handler_impl(self, handler)
        }
    }

    impl caps::LifecycleOps for AndroidBackend {
        fn finish(&mut self, root: Self::Node) {
            AndroidBackend::finish_impl(self, root)
        }

        fn run_layout(&mut self) {
            AndroidBackend::run_layout_impl(self)
        }

        fn schedule_layout_pass() {
            AndroidBackend::schedule_layout_pass_impl()
        }
    }

    // -----------------------------------------------------------------------
    // View + input + pressable
    // -----------------------------------------------------------------------

    impl caps::ViewOps for AndroidBackend {
        fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
            AndroidBackend::create_view_impl(self, a11y)
        }

        fn make_view_handle(&self, node: &Self::Node) -> runtime_shared::ViewHandle {
            AndroidBackend::make_view_handle_impl(self, node)
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
            AndroidBackend::install_touch_handler_impl(self, node, handler)
        }

        fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
            AndroidBackend::claim_touch_impl(self, node, touch_id)
        }
    }

    impl caps::PressableOps for AndroidBackend {
        fn create_pressable(
            &mut self,
            on_click: Rc<dyn Fn()>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            AndroidBackend::create_pressable_impl(self, flushing0(on_click), a11y)
        }
    }

    // -----------------------------------------------------------------------
    // Text + button
    // -----------------------------------------------------------------------

    impl caps::TextOps for AndroidBackend {
        fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
            AndroidBackend::create_text_impl(self, content, a11y)
        }

        fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
            AndroidBackend::create_styled_text_impl(self, runs, a11y)
        }

        fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
            AndroidBackend::update_styled_text_impl(self, node, runs)
        }

        fn update_text(&mut self, node: &Self::Node, content: &str) {
            AndroidBackend::update_text_impl(self, node, content)
        }

        fn make_text_handle(&self, node: &Self::Node) -> runtime_shared::TextHandle {
            AndroidBackend::make_text_handle_impl(self, node)
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
            AndroidBackend::create_button_impl(
                self,
                label,
                &on_click,
                leading_icon,
                trailing_icon,
                a11y,
            )
        }

        fn make_button_handle(&self, node: &Self::Node) -> runtime_shared::ButtonHandle {
            AndroidBackend::make_button_handle_impl(self, node)
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
            AndroidBackend::create_image_impl(self, src, alt, a11y)
        }
    }

    impl caps::IconOps for AndroidBackend {
        fn create_icon(
            &mut self,
            data: &primitives::icon::IconData,
            color: Option<&Color>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            AndroidBackend::create_icon_impl(self, data, color, a11y)
        }

        fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
            AndroidBackend::update_icon_color_impl(self, node, color)
        }

        fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
            AndroidBackend::update_icon_stroke_impl(self, node, progress)
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
            AndroidBackend::animate_icon_stroke_impl(
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
            AndroidBackend::create_link_impl(self, config, a11y)
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
            AndroidBackend::create_text_input_impl(
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
            AndroidBackend::update_text_input_value_impl(self, node, value)
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
            AndroidBackend::create_text_area_impl(
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
            AndroidBackend::update_text_area_value_impl(self, node, value)
        }

        fn make_text_input_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::text_input::TextInputHandle {
            AndroidBackend::make_text_input_handle_impl(self, node)
        }

        fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
            AndroidBackend::make_text_area_handle_impl(self, node)
        }
    }

    impl caps::ToggleOps for AndroidBackend {
        fn create_toggle(
            &mut self,
            initial_value: bool,
            on_change: Rc<dyn Fn(bool)>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            AndroidBackend::create_toggle_impl(self, initial_value, flushing1(on_change), a11y)
        }

        fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
            AndroidBackend::update_toggle_value_impl(self, node, value)
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
            AndroidBackend::create_slider_impl(
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
            AndroidBackend::update_slider_value_impl(self, node, value)
        }
    }

    impl caps::ActivityIndicatorOps for AndroidBackend {
        fn create_activity_indicator(
            &mut self,
            size: primitives::activity_indicator::ActivityIndicatorSize,
            color: Option<&Color>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            AndroidBackend::create_activity_indicator_impl(self, size, color, a11y)
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
            AndroidBackend::create_scroll_view_impl(self, horizontal, on_scroll, a11y)
        }

        fn make_scroll_view_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::scroll_view::ScrollViewHandle {
            AndroidBackend::make_scroll_view_handle_impl(self, node)
        }
    }

    impl caps::SafeAreaOps for AndroidBackend {
        fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
            AndroidBackend::apply_safe_area_padding_impl(self, node, sides)
        }

        fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
            AndroidBackend::apply_scroll_view_safe_area_inset_impl(self, node, sides)
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
            //
            // mount/release additionally run WORLD-ENTERED
            // (`enter_mounted_world`): `mount_item` realizes the row —
            // creation-side work (`theme_ctx` → `inject::<ThemeCtx>`)
            // that aborts outside `World::enter` (the
            // flat_list-renders-zero-rows bug); `release_item` drops the
            // row scope, whose cleanups get the same ambient guarantee
            // the old walker's teardown had.
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
                        let mounted = super::enter_mounted_world(|| f(i));
                        schedule_flush();
                        mounted
                    })
                },
                release_item: {
                    let f = release_item;
                    Rc::new(move |scope_id| {
                        super::enter_mounted_world(|| f(scope_id));
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
            AndroidBackend::create_virtualizer_impl(self, callbacks, overscan, layout, a11y)
        }

        fn virtualizer_data_changed(&mut self, node: &Self::Node) {
            AndroidBackend::virtualizer_data_changed_impl(self, node)
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
            AndroidBackend::create_graphics_impl(self, on_ready, on_resize, on_lost, a11y)
        }

        fn release_graphics(&mut self, node: &Self::Node) {
            AndroidBackend::release_graphics_impl(self, node)
        }

        fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
            AndroidBackend::make_graphics_handle_impl(self, node)
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
            AndroidBackend::create_portal_impl(self, target, on_dismiss, trap_focus, a11y)
        }

        fn release_portal(&mut self, node: &Self::Node) {
            AndroidBackend::release_portal_impl(self, node)
        }
    }

    impl caps::PresenceOps for AndroidBackend {

        fn apply_presence(
            &mut self,
            node: &Self::Node,
            state: primitives::presence::PresenceState,
            transition: Option<(u32, Easing)>,
        ) {
            AndroidBackend::apply_presence_impl(self, node, state, transition)
        }
    }

    impl caps::NavigatorOps for AndroidBackend {
    // Every method of this capability is now the caps trait's default.
    // The old-core `create_navigator` (which registered the per-instance
    // `NavigatorHandler` these four methods dispatched to) was DELETED
    // with the old core — it does not fall back to a default
    // (docs/runtime-v2-deletion-baseline.md §2.3), so no handler is ever
    // registered and the vocabulary's navigator handler never calls
    // them — it mounts navigators over `ViewOps`/`LifecycleOps` and
    // folds screen chrome itself. Native push / header chrome on this
    // backend is the documented native-nav seam, tracked in the module
    // docs; it must be re-entered through the scene registry, not
    // through a resurrected mega-trait cap.
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
            AndroidBackend::create_external_impl(self, type_id, type_name, payload, a11y)
        }

        fn release_external(&mut self, node: &Self::Node) {
            AndroidBackend::release_external_impl(self, node)
        }
    }

    impl caps::DocumentOps for AndroidBackend {
    }

    // -----------------------------------------------------------------------
    // Style + assets
    // -----------------------------------------------------------------------

    impl caps::StyleOps for AndroidBackend {
        fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
            AndroidBackend::apply_style_impl(self, node, style)
        }

        fn on_node_unstyled(&mut self, node: &Self::Node) {
            AndroidBackend::on_node_unstyled_impl(self, node)
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
            AndroidBackend::attach_states_impl(self, node, setter)
        }

        fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
            AndroidBackend::set_disabled_impl(self, node, disabled)
        }
    }

    impl caps::AssetOps for AndroidBackend {
        fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
            AndroidBackend::register_asset_impl(self, id, kind, source)
        }

        fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
            AndroidBackend::unregister_asset_impl(self, id, kind)
        }

        fn register_typeface(
            &mut self,
            id: TypefaceId,
            family_name: &str,
            faces: &[TypefaceFace],
            fallback: SystemFallback,
        ) {
            AndroidBackend::register_typeface_impl(self, id, family_name, faces, fallback)
        }

        fn unregister_typeface(&mut self, id: TypefaceId) {
            AndroidBackend::unregister_typeface_impl(self, id)
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
            AndroidBackend::update_accessibility_impl(self, node, a11y, inferred_role)
        }

        fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
            AndroidBackend::announce_for_accessibility_impl(self, msg, priority)
        }
    }

    impl caps::AnimationOps for AndroidBackend {
        fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
            AndroidBackend::set_animated_f32_impl(self, node, prop, value)
        }

        fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
            AndroidBackend::set_animated_color_impl(self, node, prop, value)
        }
    }

    impl caps::IntrospectionOps for AndroidBackend {
        fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
            AndroidBackend::frame_impl(self, node)
        }

        fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
            AndroidBackend::device_frame_impl(self, node)
        }

        fn supports_screenshot(&self) -> bool {
            AndroidBackend::supports_screenshot_impl(self)
        }

        fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
            AndroidBackend::capture_screenshot_impl(self, done)
        }
    }

    // -----------------------------------------------------------------------
    // Batch + wire bindings
    // -----------------------------------------------------------------------

    impl caps::BatchOps for AndroidBackend {
    }

    impl caps::WireBindingOps for AndroidBackend {
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
    use runtime_shared::scheduling::{ScheduleHandle, Scheduler};
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
        runtime_shared::scheduling::install_scheduler(Box::new(LooperStandIn));
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

        let handler: runtime_shared::primitives::key::KeyDownHandler = {
            Rc::new(move |_ev| {
                hits.update(|n| n + 1);
                runtime_shared::primitives::key::KeyOutcome::PreventDefault
            })
        };
        let wrapped = flushing_key(handler);
        let ev = runtime_shared::primitives::key::KeyEvent {
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
                runtime_shared::primitives::key::KeyOutcome::PreventDefault
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

    /// Regression (native new-core breakpoints frozen at their seed —
    /// rotation/configuration change never re-fired breakpoint-
    /// dependent `when`s): the Android resize seam
    /// ([`forward_viewport`], called from `imp::viewport_size()`'s
    /// deferred mirror microtask right after its `set_viewport_size`
    /// write) must stage the size into the mounted world's viewport
    /// ctx from OUTSIDE `World::enter` and commit it through the flush
    /// driver, re-firing a breakpoint-reading effect exactly when the
    /// BUCKET changes — and must go inert once the sink is cleared,
    /// so a mirror racing Activity-recreation teardown is a no-op (the
    /// P5 dead-world discipline). The JNI layout machinery that fires
    /// the mirror only exists inside an Android process (module header
    /// above — same limitation as every `imp/` surface); this
    /// host-side test is the reachable unit gate for everything from
    /// the seam fn down.
    #[test]
    fn regression_resize_seam_recomputes_breakpoint_via_viewport_sink() {
        install_test_scheduler();
        let world = World::new();
        set_flush_world(Some(world.clone()));

        // What `start` does: capture the world's ctx (inside enter,
        // post-build position) and install the sink.
        let (vp_sig, runs, last) = world.enter(|| {
            let ctx = runtime_vocabulary::viewport::viewport_ctx();
            let bp = ctx.breakpoint();
            let runs = Rc::new(Cell::new(0usize));
            let last = Rc::new(Cell::new(runtime_shared::Breakpoint::Xs));
            let runs_c = runs.clone();
            let last_c = last.clone();
            // Stand-in for the shell's `when(!sidebar_pinned(Lg))`.
            let _e = effect(move || {
                last_c.set(bp.get());
                runs_c.set(runs_c.get() + 1);
            });
            (ctx.size_signal(), runs, last)
        });
        world.flush();
        assert_eq!(runs.get(), 1);
        set_viewport_sink(Some(vp_sig));

        // The seam: fires outside `enter` (a posted looper microtask),
        // stages, flush commits on the next looper turn.
        forward_viewport(runtime_shared::ViewportSize {
            width: 1280.0,
            height: 800.0,
        });
        assert_eq!(runs.get(), 1, "staged — commits on the next looper turn");
        pump();
        assert_eq!(
            last.get(),
            runtime_shared::Breakpoint::Xl,
            "bucket followed the resize"
        );
        assert_eq!(runs.get(), 2);

        // Same-bucket resize: per-pixel change, no bucket flip, no
        // re-fire (memo equality cut).
        forward_viewport(runtime_shared::ViewportSize {
            width: 1290.0,
            height: 800.0,
        });
        pump();
        assert_eq!(runs.get(), 2, "per-pixel resizes inside a bucket stay silent");

        // Rotation-shaped crossing below the threshold.
        forward_viewport(runtime_shared::ViewportSize {
            width: 700.0,
            height: 800.0,
        });
        pump();
        assert_eq!(last.get(), runtime_shared::Breakpoint::Sm);
        assert_eq!(runs.get(), 3);

        // Teardown severs the route (what `stop` does — the Activity-
        // recreation race): a late mirror microtask forwards into a
        // cleared sink and nothing is staged or scheduled.
        set_viewport_sink(None);
        forward_viewport(runtime_shared::ViewportSize {
            width: 1280.0,
            height: 800.0,
        });
        assert!(
            !FLUSH.with(|f| f.queued.get()),
            "cleared sink schedules nothing"
        );
        pump();
        assert_eq!(runs.get(), 3, "no re-fire after teardown");

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

    /// Regression (flat_list rendered ZERO rows on new-core — every
    /// backend shared the gap): `enter_mounted_world`, the virtualizer
    /// mount/release dispatch-site wrapper, runs its callback with the
    /// boot-stored world ambient so creation-side row work
    /// (`signal()`/`effect()`/`inject`) is legal; without a stored
    /// world it falls back to a bare call, which an ambient boot-time
    /// `enter` still covers. (The JNI caps impl consuming this is
    /// `cfg(target_os = "android")`; this host-side test is the
    /// reachable unit gate.)
    #[test]
    fn enter_mounted_world_enters_stored_world_and_falls_back_bare() {
        let world = World::new();
        set_flush_world(Some(world.clone()));
        // Creation-side work inside the wrapper — the class that
        // aborted when mount_item ran outside `World::enter`.
        let sig = enter_mounted_world(|| signal(41i32));
        assert_eq!(world.enter(|| sig.get()), 41, "created in the stored world");
        set_flush_world(None);
        // No stored world: bare invocation…
        assert_eq!(enter_mounted_world(|| 7), 7);
        // …which an ambient enter (the pre-mount-store window) still
        // covers for creation-side work.
        let sig2 = world.enter(|| enter_mounted_world(|| signal(8i32)));
        assert_eq!(world.enter(|| sig2.get()), 8);
    }

    /// `mounted_world` hands an embedded mount
    /// (`host_android_mobile::mount_newcore`) the SAME world the boot
    /// stored — creations made through the clone land in the boot
    /// world — and reports `None` once the slot clears, so a
    /// mis-sequenced embed fails fast (`MountError::NoHostWorld`)
    /// instead of realizing into a dead world.
    #[test]
    fn mounted_world_clones_boot_world_for_embedded_mounts() {
        let world = World::new();
        set_flush_world(Some(world.clone()));
        let mounted = mounted_world().expect("boot stored a world");
        let sig = mounted.enter(|| signal(9i32));
        assert_eq!(world.enter(|| sig.get()), 9, "clone is the boot world");
        set_flush_world(None);
        assert!(mounted_world().is_none(), "cleared after stop");
    }
}

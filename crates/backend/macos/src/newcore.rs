//! New-core adoption for the macOS backend (idea-lite migration, P4a).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on [`MacosBackend`] — the
//! production shape of the migration: no `LegacyBridge` wrapper in the
//! render path. Every trait method delegates via UFCS
//! (`<MacosBackend as Backend>::method(self, …)`) to the existing
//! `Backend` impl, so the AppKit mechanism code (NSView creation, Taffy
//! layout, style translation, transitions, virtualizer cells) is REUSED
//! verbatim — this module adds a second *front* onto the same machinery,
//! exactly like `backend-web/src/newcore.rs` does for the DOM (this file
//! mirrors that module mechanically; keep the two in sync).
//!
//! # Delegation status — every capability accounted for
//!
//! | Trait | Status |
//! |---|---|
//! | `runtime_scene::Host` (7 ops) | direct (`create_anchor` → `create_reactive_anchor`, `supports_splice` → `supports_child_splice` — the P1 renames) |
//! | `AppEnvOps` | direct (app-level key handler wrapped — dispatch-site glue) |
//! | `LifecycleOps` | direct (`is_hydrating` is always `false` on this backend — no hydration on native) |
//! | `ViewOps` | direct |
//! | `InputOps` | direct (touch / wheel / hover / file-drop handlers wrapped) |
//! | `PressableOps` | direct (on_click wrapped) |
//! | `TextOps` | direct (the js-binding methods resolve to the trait-default no-ops, same as the old walker: `supports_js_text_bindings` is `false` here) |
//! | `ButtonOps` | direct (`Action::fire` wrapped) |
//! | `ImageOps` | direct (load / error handlers wrapped) |
//! | `IconOps` | direct |
//! | `LinkOps` | direct (on_activate wrapped) |
//! | `TextInputOps` | direct (on_change / on_key_down / on_blur / focus wrapped) |
//! | `ToggleOps` | direct (on_change wrapped) |
//! | `SliderOps` | direct (on_change wrapped) |
//! | `ActivityIndicatorOps` | direct |
//! | `ScrollOps` | direct (on_scroll wrapped) |
//! | `SafeAreaOps` | direct |
//! | `VirtualizerOps` | direct (mount / release / measured-size wrapped) |
//! | `GraphicsOps` | direct (ready / resize / lost wrapped) |
//! | `PortalOps` | direct (on_dismiss wrapped) |
//! | `PresenceOps` | direct |
//! | `NavigatorOps` | direct (host callbacks deliberately NOT wrapped — see impl) |
//! | `ExternalOps` | direct |
//! | `DocumentOps` | direct (web-flavored methods — `create_element`, `attach_html_*`, `register_raw_css` — resolve to the same trait-default no-ops the old walker hit on this backend) |
//! | `StyleOps` | direct (class-minting methods return the trait-default `None` on native; the vocabulary's `attach_style` then takes the `apply_styled_variants` path — identical routing to the old walker. `attach_states` setter wrapped. See *Styling* below.) |
//! | `AssetOps` | direct |
//! | `A11yOps` | direct |
//! | `AnimationOps` | direct |
//! | `IntrospectionOps` | direct |
//! | `BatchOps` | direct |
//! | `WireBindingOps` | direct (wire-recorder no-ops on this backend, same as today) |
//!
//! **30/30 direct, 0 adapted, 0 stubbed.** Nothing panics, nothing
//! silently no-ops beyond what the wrapped `Backend` impl already does.
//! Where the macOS backend does not override a `Backend` method (the
//! DOM-only and wire-only families), the UFCS call resolves to the same
//! trait-default fallback the old walker would hit — behavior is
//! identical by construction. The impl bodies below are generated
//! mechanically from `backend-web/src/newcore.rs` (itself generated from
//! `runtime_vocabulary::bridge`, the compile-time proof of the signature
//! freeze), with `WebBackend` → `MacosBackend`.
//!
//! # Styling note for the parallel P3c style-engine work
//!
//! On this backend `mint_style_class` / `mint_class_for_app` /
//! `supports_js_class_bindings` all take the trait-default path (`None` /
//! `false`): macOS styles are applied per-node via
//! `apply_style`/`apply_styled_variants` and re-fired through the
//! theme-cohort driver on token updates
//! (`token_updates_propagate_via_cascade` is `false`, so cohort fan-out —
//! not CSS cascade — is the token-update mechanism). Any new-core style
//! engine must keep driving the cohort re-apply path on native.
//!
//! # Boot path — [`start`]
//!
//! Client-render-only mount of a `runtime_scene::Element` tree against
//! live AppKit through the registry. Unlike web (where the browser is the
//! host and `backend_web::newcore::start` owns the whole boot), macOS
//! splits host duties: `host-appkit` owns NSApplication/NSWindow and the
//! content-view handoff (`create_host_root` → `setContentView` →
//! `set_host_root`), and THIS function owns everything from "backend is
//! wired to a window" onward. `host_appkit::newcore::run` is the
//! all-in-one entry apps call; it constructs the backend exactly like the
//! old-core `host_appkit::run_with` and then calls [`start`].
//!
//! Sequence (mirrors `runtime_shared::mount`'s ordering where they overlap):
//!
//! 1. Install the default monotonic time source (the macOS analogue of
//!    web `start_in`'s `install_time_source` — without it the animation
//!    clock and `PhaseTimer` read 0). The old `mount` preamble's other
//!    ambient installs (current platform, color scheme, URL opener,
//!    announcer) are runtime-core-private and skipped here, exactly as
//!    on the web new-core boot — public seams for them are a
//!    later-phase migration item.
//! 2. `Registry` (`register_builtins` + the `register` seam) + `World` +
//!    `world.enter(realize)`.
//! 3. `runtime_shared::scheduling::drain_buffered_microtasks()` — the host
//!    opened a mount-buffering window (`begin_mount_buffering`) before
//!    calling in, so microtasks scheduled during the build (deferred
//!    chrome, follow-up layout passes) run HERE, synchronously, before
//!    `finish` — landing in the first layout/paint exactly like the old
//!    boot (`runtime_shared::mount` drains at the same point; see
//!    `backend_apple_core::scheduler::MOUNT_BUFFER` and the macOS
//!    chrome-first-paint-buffering note). A [`schedule_flush`] issued
//!    during the window also buffers and therefore commits inside this
//!    drain — staged writes cannot leak past the first paint.
//! 4. `Backend::finish(root)` — parents the single root NSView into the
//!    host root (`addSubview`) and runs the first Taffy layout pass: the
//!    exact root-attachment mechanism of the old mount. `finish` is
//!    called with the backend `RefCell` borrowed and defers its
//!    viewport-signal mirror to a microtask for that reason — do not
//!    drain microtasks while the borrow is held.
//! 5. `world.flush()` — commit anything staged during mount (ref-fill
//!    callbacks, handler setup) before the first paint.
//! 6. Install the flush driver (the `backend-apple-core` post-dispatch
//!    hook), retain `{Realized, backend, registry, world}` in the
//!    returned [`NewCoreApp`]. The host closes the buffering window
//!    (`end_mount_buffering`) after this returns — leftover microtasks
//!    (e.g. `finish`'s viewport mirror) dispatch normally onto the main
//!    queue, same as the old boot.
//!
//! **Hydration is NOT in scope** (native never hydrates).
//!
//! # Flush driver (design §3) — dispatch-site glue, NO safety net
//!
//! The new kernel stages writes; nothing is observable until the host
//! driver calls [`World::flush`]. This backend uses the SETTLED driver
//! design (the web module's, adopted by iOS/Android and now here —
//! replacing the original P4a NSEvent-local-monitor + 60 Hz NSTimer
//! pair): **precise dispatch-site glue**, no polling.
//!
//! 1. **Author-callback wrapping (this module).** Every callback-taking
//!    capability impl below wraps the author callback before delegating
//!    to the `Backend` machinery: press/click, input/change, toggle,
//!    slider, scroll, hover, wheel, touch, key, blur/focus, file-drop,
//!    image load/error, link activation, portal dismiss, graphics
//!    lifecycle, virtualizer row mount/release/measure, state setters,
//!    and the app-level key handler. The wrapper calls the author fn,
//!    then [`schedule_flush`] — one deduped
//!    `runtime_shared::scheduling::schedule_microtask`, which on this
//!    platform is `dispatch_async(main_queue)` (see
//!    `backend_apple_core::scheduler`) and drains on a LATER run-loop
//!    iteration — strictly AFTER the current event's synchronous
//!    dispatch (responder chain, target-action, author `on_press`
//!    closures) completes. Net effect: stage during dispatch, commit at
//!    the run-loop turn boundary right after — the idea-lite contract.
//!    Because the wrapping happens in these new-core-only impls, the
//!    shared old-core event closures are reused verbatim and the old
//!    core never pays for it.
//! 2. **Post-dispatch hook (`backend_apple_core::dispatch_hook`).**
//!    Author code also runs from non-event surfaces: `after_ms` timers,
//!    `after_animation_frame` one-shots, `raf_loop` iterations (the
//!    animation clock's common-modes NSTimer on macOS), and
//!    async-executor future polls. The apple-core scheduler and executor
//!    fire a thread-local hook after each such callback; [`start`]
//!    installs [`schedule_flush`] into that slot (no-op default, so the
//!    old core is untouched).
//!
//! **Why the P4a monitor+timer are gone — the tracking-loop question.**
//! The original monitor+timer pair existed because AppKit control
//! **tracking loops** (NSButton/NSSlider press-drags, scroller-knob
//! drags, menu tracking — nested run-loop turns in
//! `NSEventTrackingRunLoopMode` that pull events via
//! `nextEventMatchingMask:`) bypass `sendEvent:` and therefore local
//! NSEvent monitors; the common-modes timer was the net that caught
//! writes staged inside them. With dispatch-site wrapping the monitor's
//! job is done at the source: an author callback invoked DURING a
//! tracking loop still comes through the wrapped caps closure, so the
//! flush is scheduled regardless of which run-loop turn is active. The
//! remaining question is whether the scheduled flush *fires* while the
//! nested tracking loop is still running (a default-mode source would
//! stall until tracking ends — the classic frozen-slider-label bug).
//! It does: `schedule_microtask` is `dispatch_async(main_queue)`, and
//! the main GCD queue is a **common-modes** run-loop source — CF drains
//! it whenever the main run loop runs in any common mode, and
//! `NSEventTrackingRunLoopMode` is a common mode. Verified LIVE, not
//! assumed: the `newcore-macos-smoke` self-test stages a write via
//! `schedule_flush`, then spins a nested
//! `CFRunLoopRunInMode(NSEventTrackingRunLoopMode)` turn (the exact
//! run-loop state a control tracking loop creates) and observes the
//! commit effect fire *inside* that nested turn, with
//! `CFRunLoopCopyCurrentMode` reporting the tracking mode at commit
//! time — including for a write staged by a scroll-view `on_scroll`
//! author callback (see the smoke crate's scroll/tracking self-test).
//! If libdispatch ever stopped draining the main queue in tracking
//! mode, that self-test is the regression tripwire; the fix would be
//! scheduling the flush like apple-core schedules common-modes timers,
//! NOT resurrecting the monitor.
//!
//! Everything funnels through [`schedule_flush`]/`flush_now`, which
//! skips re-entrant flushes (`world.is_flushing()`) — belt and braces;
//! a main-queue microtask can't actually preempt a synchronous flush.
//!
//! Residual surfaces NOT covered (documented, not silent): callbacks
//! installed by `Element::External` third-party macOS glue (the
//! External registry predates the new core; its port must call
//! [`schedule_flush`] after author callbacks).

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_shared::accessibility::{AccessibilityProps, AccessibilityTree, LiveRegionPriority, Role};
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

use crate::imp::{MacosBackend, MacosNode};

// Re-exported so the host shell (`host-appkit`) and app wrappers can
// name the boot-path types without a direct runtime-scene dependency —
// mirrors how consumers reach the old core's `Element` through
// `runtime_shared`.
pub use runtime_scene::Element as SceneElement;
pub use runtime_scene::Registry as SceneRegistry;

// ===========================================================================
// Boot path
// ===========================================================================

thread_local! {
    /// The world the flush driver commits. Kept out of [`NewCoreApp`]'s
    /// custody so [`schedule_flush`] never touches app state (a flush can
    /// run while the app value is mid-construction inside [`start`]).
    static FLUSH_WORLD: RefCell<Option<World>> = const { RefCell::new(None) };
    /// Dedup flag: one queued flush microtask at a time.
    static FLUSH_QUEUED: Cell<bool> = const { Cell::new(false) };
}

/// Everything the boot path must keep alive. Field order is drop order:
/// the realized tree unmounts before the world (its slots' owner) dies.
/// The host typically `std::mem::forget`s this before entering the run
/// loop (same retention as the old boot's `forget(owner)` — the process
/// exits with the run loop).
pub struct NewCoreApp {
    realized: Realized<MacosNode>,
    _backend: Rc<RefCell<MacosBackend>>,
    _registry: Rc<Registry<MacosBackend>>,
    world: World,
}

impl NewCoreApp {
    /// Borrow the live tree (tests, diagnostics).
    pub fn with_realized<R>(&self, f: impl FnOnce(&Realized<MacosNode>) -> R) -> R {
        f(&self.realized)
    }

    /// The mounted world (tests can flush it explicitly).
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Unmount: unhooks the flush driver (dispatch hook + world
    /// reference) and the viewport sink, then drops the `Realized`
    /// (cleanups fire, views detach from the live tree's point of view)
    /// and the world. Primarily for tests; a windowed app forgets the
    /// value instead.
    pub fn stop(self) {
        backend_apple_core::dispatch_hook::clear_dispatch_hook();
        set_viewport_sink(None);
        set_flush_world(None);
        drop(self);
    }
}

/// Mount a new-core element tree into an already-window-wired backend.
///
/// The host must have: constructed the backend, parented its host root
/// into an NSWindow (`create_host_root` → `setContentView` →
/// `set_host_root`), installed the scheduler + global self-handle, and
/// opened a mount-buffering window. See the module docs for the exact
/// sequence and `host_appkit::newcore::run` for the canonical caller.
///
/// `register` runs after [`runtime_vocabulary::register_builtins`], so
/// apps/SDKs can register their own payload handlers on the same
/// registry before the tree realizes. The build closure runs inside
/// `world.enter`, so free `signal()`/`effect()` calls work; top-level
/// creations are world-root-owned (they live until app teardown).
#[inline]
pub fn start(
    backend: Rc<RefCell<MacosBackend>>,
    register: impl FnOnce(&mut Registry<MacosBackend>),
    build: impl FnOnce() -> Element,
) -> NewCoreApp {
    // `#[inline]` is load-bearing, not a perf hint: as a plain
    // non-generic `pub` fn this is codegen'd into the rlib and can
    // survive to the final link, instantiating
    // `register_builtins_with::<_, AllBuiltins>` and re-anchoring the
    // WHOLE builtin vocabulary even for an app that selected a
    // smaller set through `start_with`.
    start_with::<runtime_vocabulary::AllBuiltins, _, _>(backend, register, build)
}

/// [`start`], booting only the builtin primitives `S` selects.
///
/// The selector is a type parameter so the choice is made at compile time:
/// an unselected primitive's registration folds away, nothing names its
/// handler, and the linker drops it along with the backend code it alone
/// reached. A runtime flag could not do this — it would still link every
/// handler. See [`runtime_vocabulary::BuiltinSet`].
///
/// Realizing a payload this set omits panics at mount, the same loud
/// failure an unregistered third-party payload gets.
pub fn start_with<S, R, B>(
    backend: Rc<RefCell<MacosBackend>>,
    register: R,
    build: B,
) -> NewCoreApp where
    S: runtime_vocabulary::BuiltinSet,
    R: FnOnce(&mut Registry<MacosBackend>),
    B: FnOnce() -> Element,
{
    // Monotonic clock (step 1 in the module docs) — the macOS analogue
    // of web `start_in`'s `install_time_source`. Idempotent, first
    // install wins. The old `mount` preamble's other ambient installs
    // (`install_current_platform` / color scheme / URL opener /
    // announcer) live in a runtime-core-private module and are NOT
    // reachable from a backend crate — same situation as
    // `backend_web::newcore::start`, which also boots without them.
    // Author code reading `runtime_shared::platform()` on the new core
    // gets the uninstalled default until the migration gives those
    // installs a public seam (later-phase item, noted in module docs).
    let platform = backend.borrow().platform_impl();
    runtime_shared::time::install_default_time_source(platform);

    let mut registry: Registry<MacosBackend> = Registry::new();
    runtime_vocabulary::register_builtins_with::<_, S>(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    let world = World::new();
    let (vp_sig, realized) = world.enter(|| {
        let element = build();
        let realized = realize(&backend, &registry, element);
        // Capture the per-world viewport ctx AFTER the build, never
        // before: the ctx's derived-bucket memo pins the breakpoint
        // TABLE at creation, and apps `install_breakpoints` inside
        // their root component — eager pre-build capture would pin the
        // default table (see backend-web's identical ordering comment
        // and the viewport-source section below).
        let vp_sig = runtime_vocabulary::viewport::viewport_ctx().size_signal();
        (vp_sig, realized)
    });

    // Pre-`finish` buffered drain (step 3) — deferred chrome and any
    // build-time `schedule_flush` land before the first layout. Must run
    // with NO backend borrow held (drained tasks re-borrow the backend).
    runtime_shared::scheduling::drain_buffered_microtasks();

    // Single-root contract, matching the old-core mount: `finish`
    // parents the root view into the host root and runs the first
    // layout pass.
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_macos::newcore::start: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    MacosBackend::finish_impl(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount before the first paint.
    world.flush();

    // Install the flush driver: schedule_flush becomes reachable from
    // (a) the author-callback wrappers in the caps impls below and
    // (b) the apple-core scheduler/executor post-dispatch hook.
    backend_apple_core::dispatch_hook::install_dispatch_hook(schedule_flush);
    set_flush_world(Some(world.clone()));
    // Live viewport source: the AppKit resize seams now reach the
    // world's ctx through [`forward_viewport`]. `finish`'s deferred
    // first mirror (a mount-buffered microtask that drains after the
    // host closes the buffering window) lands the REAL window size in
    // the ctx right after boot; window resizes follow from
    // `LayoutObserverView`. See the viewport-source section below.
    set_viewport_sink(Some(vp_sig));
    NewCoreApp {
        realized,
        _backend: backend,
        _registry: registry,
        world,
    }
}

fn set_flush_world(world: Option<World>) {
    FLUSH_WORLD.with(|w| *w.borrow_mut() = world);
}

// ===========================================================================
// Flush driver
// ===========================================================================

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// The dispatch-site wrappers and the apple-core post-dispatch hook
/// call this right after author-visible dispatch. During a
/// mount-buffering window the microtask buffers and commits inside the
/// host's synchronous drain (module docs, step 3).
pub fn schedule_flush() {
    if FLUSH_QUEUED.with(|q| q.replace(true)) {
        return;
    }
    runtime_shared::scheduling::schedule_microtask(|| {
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
            world.flush();
        }
    }
}

/// The world mounted by [`start`] (a cheap handle clone; `None` before
/// boot / after [`NewCoreApp::stop`]).
///
/// Host-integration seam: an embedded renderer mounted INSIDE this
/// app's tree — the wgpu simulator preview
/// (`host_macos_desktop::mount_newcore`) — realizes its scene into this
/// SAME world, so the app's existing flush driver (dispatch-site
/// wrappers + the apple-core scheduler/executor post-dispatch hook)
/// commits the embedded app's staged writes with no second driver:
/// one thread, one world, one logical update stream. Mirrors
/// `backend_web::newcore::mounted_world`.
pub fn mounted_world() -> Option<World> {
    FLUSH_WORLD.with(|w| w.borrow().clone())
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
/// assigns the entry to the backend. Pre-boot (`FLUSH_WORLD` empty —
/// the initial mount's realize) the boot's own `enter` is still
/// ambient, so a bare call is already entered; nesting `enter` is a
/// legal stack, so the ambient fallback never double-books.
fn enter_mounted_world<R>(f: impl FnOnce() -> R) -> R {
    match FLUSH_WORLD.with(|w| w.borrow().clone()) {
        Some(world) => world.enter(f),
        None => f(),
    }
}

// ===========================================================================
// Viewport source (the new-core macOS resize seam)
// ===========================================================================
//
// The vocabulary's per-world viewport/breakpoint ctx
// (`runtime_vocabulary::viewport`) is SEED-ONLY unless the platform
// pushes live sizes (its module docs). This backend's resize seams are
// the `LayoutObserverView` `setFrameSize:` override (imp/callbacks.rs)
// and `finish`'s deferred first mirror (imp/mod.rs); both keep writing
// the shared old-core TLS value (`runtime_shared::set_viewport_size`) —
// the old core subscribes to it, and the world ctx SEEDS from it — and
// additionally call [`forward_viewport`] so breakpoint-dependent author
// reactivity (`when(!sidebar_pinned(Lg))` — the idea-ui-docs hamburger
// class of bug) re-fires on window resize instead of freezing at its
// seed.
//
// Discipline (mirrors `backend_web::newcore`'s resize listener): the
// seams run OUTSIDE `World::enter` (AppKit callbacks / main-queue
// microtasks), so the boot CAPTURES the world's signal handle — capture,
// don't inject — and the push stages through the handle (routes to its
// own world, equality-guarded, dead-world writes are silent no-ops)
// then rides one deduped [`schedule_flush`]: the same event-boundary
// glue as every dispatch-site wrapper above.

thread_local! {
    /// The mounted world's viewport signal (`Copy` handle). `None`
    /// outside a new-core boot, so the shared old-core seams cost one
    /// TLS read and nothing else when the old core is driving.
    static VIEWPORT_SINK: Cell<Option<runtime_world::Signal<runtime_shared::ViewportSize>>> =
        const { Cell::new(None) };
}

fn set_viewport_sink(sig: Option<runtime_world::Signal<runtime_shared::ViewportSize>>) {
    VIEWPORT_SINK.with(|s| s.set(sig));
}

/// Forward one platform viewport mirror into the mounted world's
/// viewport ctx (no-op before [`start`] / after teardown). Called by
/// the same macOS seams that write `runtime_shared::set_viewport_size`,
/// with the same value — the two sinks never diverge.
pub(crate) fn forward_viewport(size: runtime_shared::ViewportSize) {
    let Some(sig) = VIEWPORT_SINK.with(|s| s.get()) else {
        return;
    };
    // Staged write outside `enter` + one deduped flush — commits at
    // the next run-loop turn boundary, like every wrapped callback.
    sig.set(size);
    schedule_flush();
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
// synchronously and must not pay a flush per event. Mirrors
// `backend_ios::newcore` mechanically.

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
/// through so the backend's suppress-default decision is unchanged).
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

impl Host for MacosBackend {
    type Node = MacosNode;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        MacosBackend::insert_impl(self, parent, child)
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        MacosBackend::insert_at_impl(self, parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        MacosBackend::remove_child_impl(self, parent, child)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        MacosBackend::clear_children_impl(self, node)
    }

    fn create_anchor(&mut self) -> Self::Node {
        // Runtime v2: `Host::create_anchor` is REQUIRED, and this backend
        // never overrode the old `Backend::create_reactive_anchor`, whose
        // default was exactly this. Reproduced verbatim: a plain view is a
        // correct anchor on this backend (only web needs the
        // `display: contents` variant so the branch's children keep the
        // surrounding flex context). See
        // docs/runtime-v2-deletion-baseline.md §2.2.
        MacosBackend::create_view_impl(self, &AccessibilityProps::default())
    }

    fn supports_splice(&self) -> bool {
        MacosBackend::supports_child_splice_impl(self)
    }
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

impl caps::AppEnvOps for MacosBackend {
    fn color_scheme(&self) -> ColorScheme {
        MacosBackend::color_scheme_impl(self)
    }

    fn platform(&self) -> Platform {
        MacosBackend::platform_impl(self)
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        MacosBackend::url_opener_impl(self)
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        MacosBackend::fullscreen_setter_impl(self)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        MacosBackend::set_app_background_impl(self, color)
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        // Dispatch-site glue: app-level key handlers run author code
        // (the imp/keyboard.rs NSEvent monitor dispatches into them).
        let handler = handler.map(flushing_key);
        MacosBackend::set_app_key_handler_impl(self, handler)
    }
}

impl caps::LifecycleOps for MacosBackend {
    fn finish(&mut self, root: Self::Node) {
        MacosBackend::finish_impl(self, root)
    }

    fn schedule_layout_pass() {
        MacosBackend::schedule_layout_pass_impl()
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for MacosBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        MacosBackend::create_view_impl(self, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_shared::ViewHandle {
        MacosBackend::make_view_handle_impl(self, node)
    }
}

impl caps::InputOps for MacosBackend {
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
        MacosBackend::install_touch_handler_impl(self, node, handler)
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
        MacosBackend::install_wheel_handler_impl(self, node, handler)
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        MacosBackend::install_hover_handler_impl(self, node, flushing1(handler))
    }

    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        MacosBackend::mark_preserves_focus_impl(self, node)
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
        MacosBackend::install_file_drop_handler_impl(self, node, handler)
    }
}

impl caps::PressableOps for MacosBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        MacosBackend::create_pressable_impl(self, flushing0(on_click), a11y)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_shared::PressableHandle {
        MacosBackend::make_pressable_handle_impl(self, node)
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for MacosBackend {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
        MacosBackend::create_text_impl(self, content, a11y)
    }

    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
        MacosBackend::create_styled_text_impl(self, runs, a11y)
    }

    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
        MacosBackend::update_styled_text_impl(self, node, runs)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        MacosBackend::update_text_impl(self, node, content)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_shared::TextHandle {
        MacosBackend::make_text_handle_impl(self, node)
    }
}

impl caps::ButtonOps for MacosBackend {
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
        MacosBackend::create_button_impl(
            self,
            label,
            &on_click,
            leading_icon,
            trailing_icon,
            a11y,
        )
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        MacosBackend::update_button_label_impl(self, node, label)
    }

    fn make_button_handle(&self, node: &Self::Node) -> runtime_shared::ButtonHandle {
        MacosBackend::make_button_handle_impl(self, node)
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for MacosBackend {
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node {
        MacosBackend::create_image_impl(self, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        MacosBackend::update_image_src_impl(self, node, src)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        // Dispatch-site glue: async image completion runs author code.
        let handler: ImageLoadHandler = {
            let f = handler;
            Rc::new(move |ev| {
                f(ev);
                schedule_flush();
            })
        };
        MacosBackend::install_image_load_handler_impl(self, node, handler)
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        MacosBackend::install_image_error_handler_impl(self, node, flushing0(handler))
    }
}

impl caps::IconOps for MacosBackend {
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        MacosBackend::create_icon_impl(self, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        MacosBackend::update_icon_color_impl(self, node, color)
    }
}

impl caps::LinkOps for MacosBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: link activation dispatches navigation
        // (stages nav-queue tick signals on the new core).
        let mut config = config;
        config.on_activate = flushing0(config.on_activate.clone());
        MacosBackend::create_link_impl(self, config, a11y)
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for MacosBackend {
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
        MacosBackend::create_text_input_impl(
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
        MacosBackend::update_text_input_value_impl(self, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        MacosBackend::update_text_input_secure_impl(self, node, secure)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
        MacosBackend::set_text_input_focus_handler_impl(self, node, flushing1(handler))
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        MacosBackend::update_text_input_placeholder_impl(self, node, placeholder)
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
        MacosBackend::create_text_area_impl(
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
        MacosBackend::update_text_area_value_impl(self, node, value)
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        MacosBackend::make_text_input_handle_impl(self, node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        MacosBackend::make_text_area_handle_impl(self, node)
    }
}

impl caps::ToggleOps for MacosBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        MacosBackend::create_toggle_impl(self, initial_value, flushing1(on_change), a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        MacosBackend::update_toggle_value_impl(self, node, value)
    }
}

impl caps::SliderOps for MacosBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        MacosBackend::create_slider_impl(
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
        MacosBackend::update_slider_value_impl(self, node, value)
    }
}

impl caps::ActivityIndicatorOps for MacosBackend {
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        MacosBackend::create_activity_indicator_impl(self, size, color, a11y)
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for MacosBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: on_scroll fires per scroll notification;
        // the flush microtask is deduped so a burst costs one commit.
        // (Apple on_scroll delivery is already microtask-deferred — see
        // the apple on_scroll async note — so the wrapper adds a flush
        // after that deferred author call, same ordering as every other
        // callback.)
        let on_scroll = on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
            Rc::new(move |x, y| {
                f(x, y);
                schedule_flush();
            })
        });
        MacosBackend::create_scroll_view_impl(self, horizontal, on_scroll, a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        MacosBackend::node_scroll_impl(self, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        MacosBackend::set_node_scroll_impl(self, node, x, y)
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        MacosBackend::make_scroll_view_handle_impl(self, node)
    }
}

impl caps::SafeAreaOps for MacosBackend {
}

impl caps::VirtualizerOps for MacosBackend {
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: mount/release run author render closures
        // and scope cleanups (which may stage writes) from the backend's
        // own scroll handling; measured-size reports feed the handler's
        // layout cache. item_count/item_key/item_size are pure reads and
        // stay unwrapped.
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
        MacosBackend::create_virtualizer_impl(self, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        MacosBackend::virtualizer_data_changed_impl(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        MacosBackend::release_virtualizer_impl(self, node)
    }
}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for MacosBackend {
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
        MacosBackend::create_graphics_impl(self, on_ready, on_resize, on_lost, a11y)
    }
}

impl caps::PortalOps for MacosBackend {
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: backdrop click dismissal runs the
        // author's on_dismiss.
        let on_dismiss = on_dismiss.map(flushing0);
        MacosBackend::create_portal_impl(self, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        MacosBackend::release_portal_impl(self, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        MacosBackend::set_portal_hidden_impl(self, node, hidden)
    }
}

impl caps::PresenceOps for MacosBackend {
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        MacosBackend::create_presence_placeholder_impl(self, a11y)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        MacosBackend::apply_presence_impl(self, node, state, transition)
    }
}

impl caps::NavigatorOps for MacosBackend {
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

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for MacosBackend {
    fn create_external(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        MacosBackend::create_external_impl(self, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node) {
        MacosBackend::release_external_impl(self, node)
    }
}

impl caps::DocumentOps for MacosBackend {
}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for MacosBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        MacosBackend::apply_style_impl(self, node, style)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        // Dispatch-site glue: hover/press/focus state flips can stage
        // writes when the style path routes states through signals — on
        // native this IS the live state path (no CSS pseudo-classes),
        // so the wrapper matters here, unlike web.
        let setter: Rc<dyn Fn(StateBits, bool)> = {
            let f = setter;
            Rc::new(move |bits, on| {
                f(bits, on);
                schedule_flush();
            })
        };
        MacosBackend::attach_states_impl(self, node, setter)
    }
}

impl caps::AssetOps for MacosBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        MacosBackend::register_asset_impl(self, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        MacosBackend::unregister_asset_impl(self, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        MacosBackend::register_typeface_impl(self, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        MacosBackend::unregister_typeface_impl(self, id)
    }
}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for MacosBackend {
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        MacosBackend::update_accessibility_impl(self, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        MacosBackend::announce_for_accessibility_impl(self, msg, priority)
    }
}

impl caps::AnimationOps for MacosBackend {
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        MacosBackend::set_animated_f32_impl(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        MacosBackend::set_animated_color_impl(self, node, prop, value)
    }
}

impl caps::IntrospectionOps for MacosBackend {
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        MacosBackend::frame_impl(self, node)
    }

    fn supports_native_introspection(&self) -> bool {
        MacosBackend::supports_native_introspection_impl(self)
    }

    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        MacosBackend::introspect_native_impl(self, node)
    }

    fn supports_screenshot(&self) -> bool {
        MacosBackend::supports_screenshot_impl(self)
    }

    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        MacosBackend::capture_screenshot_impl(self, done)
    }
}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for MacosBackend {
}

impl caps::WireBindingOps for MacosBackend {
}

// ===========================================================================
// Native tests — the flush driver's dedup + mount-buffering interplay.
// These run on the cargo-test thread (no main-thread AppKit required):
// they exercise the scheduler-facing glue only. The Host/caps delegation
// and the boot path are exercised by building + launching
// `newcore-macos-smoke` (AppKit objects can only be constructed on the
// main thread, so there is no unit-test seam for them — same limitation
// as every other imp/ module; see the smoke crate).
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_world::{effect, signal};

    /// `schedule_flush` queues exactly one deduped microtask; during a
    /// mount-buffering window it buffers (instead of dispatching to the
    /// main queue, which no test run loop would ever drain) and commits
    /// synchronously inside `drain_buffered_microtasks` — the exact
    /// interplay the boot path relies on (module docs, step 3).
    #[test]
    fn schedule_flush_dedups_and_commits_on_buffered_drain() {
        // Idempotent global install (first wins); the buffering window
        // below keeps every microtask on THIS thread.
        backend_apple_core::scheduler::install_scheduler();
        backend_apple_core::scheduler::end_mount_buffering(); // clean slate
        backend_apple_core::scheduler::begin_mount_buffering();

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
        // dedup (flag already set), and nothing commits until the drain.
        count.set(1);
        count.set(2);
        schedule_flush();
        assert!(FLUSH_QUEUED.with(|q| q.get()), "first call arms the flag");
        schedule_flush();
        assert_eq!(*log.borrow(), vec![0], "staged, not committed");

        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(
            *log.borrow(),
            vec![0, 2],
            "ONE flush committed the latest staged value"
        );
        assert!(
            !FLUSH_QUEUED.with(|q| q.get()),
            "drain disarms the dedup flag"
        );

        // The flag re-arms for the next write→flush cycle.
        count.set(3);
        schedule_flush();
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 2, 3]);

        set_flush_world(None);
        backend_apple_core::scheduler::end_mount_buffering();
    }

    /// `flush_now` with no mounted world is a no-op (the apple-core
    /// hook can fire before `start` finishes wiring on a cold boot),
    /// and a re-entrant flush is skipped via `world.is_flushing()`.
    #[test]
    fn flush_now_tolerates_no_world_and_reentry() {
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

    /// The apple-core post-dispatch hook wired to `schedule_flush` is
    /// THE driver for timer/frame/future-poll surfaces on macOS now
    /// that the P4a NSEvent monitor + 60 Hz NSTimer pair is gone
    /// (module docs). Regression: installing the hook and firing it
    /// after a staged write must commit the write on the next
    /// microtask drain, and clearing the hook must sever the path (a
    /// torn-down app's timers can't flush a dead world).
    #[test]
    fn dispatch_hook_route_commits_staged_writes() {
        backend_apple_core::scheduler::install_scheduler();
        backend_apple_core::scheduler::end_mount_buffering(); // clean slate
        backend_apple_core::scheduler::begin_mount_buffering();

        let world = World::new();
        set_flush_world(Some(world.clone()));
        // What `start` does (step 6) — the glue under test.
        backend_apple_core::dispatch_hook::install_dispatch_hook(schedule_flush);

        let log: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
        let count = world.enter(|| {
            let count = signal(0i32);
            let log = log.clone();
            effect(move || log.borrow_mut().push(count.get()));
            count
        });

        // Simulate an `after_ms` body: author code stages a write, then
        // the scheduler fires the hook (see apple-core scheduler's
        // `after_ms_inner` wrapping).
        count.set(7);
        backend_apple_core::dispatch_hook::fire_dispatch_hook();
        assert_eq!(*log.borrow(), vec![0], "staged, commits at the turn boundary");
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 7], "hook → schedule_flush → commit");

        // Teardown severs the route: a late timer fires the hook into a
        // no-op slot, nothing is scheduled.
        backend_apple_core::dispatch_hook::clear_dispatch_hook();
        set_flush_world(None);
        count.set(8);
        backend_apple_core::dispatch_hook::fire_dispatch_hook();
        assert!(
            !FLUSH_QUEUED.with(|q| q.get()),
            "cleared hook schedules nothing"
        );
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 7], "no commit after teardown");
        backend_apple_core::scheduler::end_mount_buffering();
    }

    /// The dispatch-site wrappers: author callback runs FIRST, then one
    /// deduped flush is queued — for all three shapes (zero-arg,
    /// one-value, key-outcome pass-through). These wrappers replace the
    /// removed NSEvent local monitor: an author callback invoked from
    /// inside an AppKit tracking loop (which bypasses monitors) still
    /// schedules its own flush.
    #[test]
    fn flushing_wrappers_run_author_code_then_queue_one_flush() {
        backend_apple_core::scheduler::install_scheduler();
        backend_apple_core::scheduler::end_mount_buffering(); // clean slate
        backend_apple_core::scheduler::begin_mount_buffering();

        let world = World::new();
        set_flush_world(Some(world.clone()));

        let log: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
        let count = world.enter(|| {
            let count = signal(0i32);
            let log = log.clone();
            effect(move || log.borrow_mut().push(count.get()));
            count
        });

        // flushing0 — the on_press shape.
        let wrapped = flushing0(Rc::new(move || count.set(1)));
        wrapped();
        assert!(FLUSH_QUEUED.with(|q| q.get()), "flush queued after author fn");
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 1], "on_press write committed");

        // flushing1 — the on_change shape (value passes through).
        let seen: Rc<Cell<f32>> = Rc::new(Cell::new(0.0));
        let seen2 = seen.clone();
        let wrapped = flushing1::<f32>(Rc::new(move |v| {
            seen2.set(v);
            count.set(2);
        }));
        wrapped(0.5);
        assert_eq!(seen.get(), 0.5, "value reached the author fn unchanged");
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 1, 2], "on_change write committed");

        // flushing_key — outcome passes through so the backend's
        // suppress-default decision is unchanged.
        let wrapped = flushing_key(Rc::new(move |_ev| {
            count.set(3);
            runtime_shared::primitives::key::KeyOutcome::PreventDefault
        }));
        let ev = runtime_shared::primitives::key::KeyEvent {
            key: "a".into(),
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
            selection_start: 0,
            selection_end: 0,
        };
        let outcome = wrapped(&ev);
        assert!(matches!(
            outcome,
            runtime_shared::primitives::key::KeyOutcome::PreventDefault
        ));
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 1, 2, 3], "key write committed");

        set_flush_world(None);
        backend_apple_core::scheduler::end_mount_buffering();
    }

    /// Regression (native new-core breakpoints frozen at their seed —
    /// resize/rotation never re-fired breakpoint-dependent `when`s):
    /// the macOS resize seam ([`forward_viewport`], called by
    /// `LayoutObserverView::setFrameSize:` and `finish`'s deferred
    /// mirror right after their `set_viewport_size` write) must stage
    /// the size into the mounted world's viewport ctx from OUTSIDE
    /// `World::enter` and commit it through the flush driver, re-firing
    /// a breakpoint-reading effect exactly when the BUCKET changes —
    /// and must go inert once the sink is cleared (teardown). The
    /// AppKit `setFrameSize:` delivery itself needs a live window
    /// (smoke-crate territory, resize self-test phase 4); this is the
    /// closest host-reachable seam, exercising everything from the
    /// seam fn down.
    #[test]
    fn regression_resize_seam_recomputes_breakpoint_via_viewport_sink() {
        backend_apple_core::scheduler::install_scheduler();
        backend_apple_core::scheduler::end_mount_buffering(); // clean slate
        backend_apple_core::scheduler::begin_mount_buffering();

        let world = World::new();
        set_flush_world(Some(world.clone()));

        // What `start` does: capture the world's ctx (inside enter,
        // post-build position) and install the sink.
        let (vp_sig, runs, last) = world.enter(|| {
            let ctx = runtime_vocabulary::viewport::viewport_ctx();
            let bp = ctx.breakpoint();
            let runs = Rc::new(Cell::new(0usize));
            let last = Rc::new(Cell::new(Breakpoint::Xs));
            let runs_c = runs.clone();
            let last_c = last.clone();
            // Stand-in for the shell's `when(!sidebar_pinned(Lg))`.
            let _e = runtime_world::effect(move || {
                last_c.set(bp.get());
                runs_c.set(runs_c.get() + 1);
            });
            (ctx.size_signal(), runs, last)
        });
        world.flush();
        assert_eq!(runs.get(), 1);
        set_viewport_sink(Some(vp_sig));

        // The seam: fires outside `enter`, stages, flush commits.
        forward_viewport(runtime_shared::ViewportSize {
            width: 1280.0,
            height: 800.0,
        });
        assert_eq!(runs.get(), 1, "staged — commits at the turn boundary");
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(last.get(), Breakpoint::Xl, "bucket followed the resize");
        assert_eq!(runs.get(), 2);

        // Same-bucket resize: per-pixel change, no bucket flip, no
        // re-fire (memo equality cut).
        forward_viewport(runtime_shared::ViewportSize {
            width: 1290.0,
            height: 800.0,
        });
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(runs.get(), 2, "per-pixel resizes inside a bucket stay silent");

        // Cross back below the threshold — the hamburger's `when` flips.
        forward_viewport(runtime_shared::ViewportSize {
            width: 700.0,
            height: 800.0,
        });
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(last.get(), Breakpoint::Sm);
        assert_eq!(runs.get(), 3);

        // Teardown severs the route (what `stop` does): a late AppKit
        // resize callback forwards into a cleared sink and nothing is
        // staged or scheduled.
        set_viewport_sink(None);
        forward_viewport(runtime_shared::ViewportSize {
            width: 1280.0,
            height: 800.0,
        });
        assert!(
            !FLUSH_QUEUED.with(|q| q.get()),
            "cleared sink schedules nothing"
        );
        runtime_shared::scheduling::drain_buffered_microtasks();
        assert_eq!(runs.get(), 3, "no re-fire after teardown");

        set_flush_world(None);
        backend_apple_core::scheduler::end_mount_buffering();
    }

    /// Regression (flat_list rendered ZERO rows on new-core — every
    /// backend shared the gap): `enter_mounted_world`, the virtualizer
    /// mount/release dispatch-site wrapper, runs its callback with the
    /// boot-stored world ambient so creation-side row work
    /// (`signal()`/`effect()`/`inject`) is legal; without a stored
    /// world it falls back to a bare call, which an ambient boot-time
    /// `enter` still covers. (The NSCollectionView machinery that
    /// invokes the wrapped callbacks needs a live AppKit run loop, so
    /// this unit test is the reachable host gate — the web/GPU legs
    /// carry the full row-mounting e2e.)
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
        // …which an ambient enter (the pre-`FLUSH_WORLD` mount window)
        // still covers for creation-side work.
        let sig2 = world.enter(|| enter_mounted_world(|| signal(8i32)));
        assert_eq!(world.enter(|| sig2.get()), 8);
    }

    /// `mounted_world` hands an embedded mount
    /// (`host_macos_desktop::mount_newcore`) the SAME world the boot
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

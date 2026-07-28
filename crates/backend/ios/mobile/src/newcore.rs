//! New-core adoption for the iOS backend (idea-lite migration, iOS
//! mirror of P4a).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on `IosBackend` — the
//! production shape of the migration: no `LegacyBridge` wrapper in the
//! render path. Every trait method delegates via UFCS
//! (`<IosBackend as Backend>::method(self, …)`) to the existing
//! `Backend` impl, so the UIKit mechanism code (UIView creation, Taffy
//! layout, style translation, transitions, UICollectionView
//! virtualizer cells — including the documented ScrollView
//! bounds/origin and clear_children/Taffy-sync quirks, whose comments
//! live with the reused bodies in `imp/`) is REUSED verbatim — this
//! module adds a second *front* onto the same machinery, exactly like
//! `backend-web/src/newcore.rs` does for the DOM and
//! `backend-macos/src/newcore.rs` for AppKit (this file mirrors those
//! modules mechanically; keep the three in sync).
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
//! | `StyleOps` | direct (class-minting methods return the trait-default `None` on native; the vocabulary's `attach_style` then takes the `apply_styled_variants` path — identical routing to the old walker. `attach_states` setter wrapped.) |
//! | `AssetOps` | direct |
//! | `A11yOps` | direct |
//! | `AnimationOps` | direct |
//! | `IntrospectionOps` | direct |
//! | `BatchOps` | direct |
//! | `WireBindingOps` | direct (wire-recorder no-ops on this backend, same as today) |
//!
//! **30/30 direct, 0 adapted, 0 stubbed.** Nothing panics, nothing
//! silently no-ops beyond what the wrapped `Backend` impl already does.
//! Where the iOS backend does not override a `Backend` method (the
//! DOM-only and wire-only families), the UFCS call resolves to the same
//! trait-default fallback the old walker would hit — behavior is
//! identical by construction. The impl bodies below are generated
//! mechanically from `backend-macos/src/newcore.rs` (itself generated
//! from `backend-web/src/newcore.rs` /
//! `runtime_vocabulary::bridge`, the compile-time proof of the
//! signature freeze), with `MacosBackend` → `IosBackend` and the web
//! module's dispatch-site callback wrapping folded in.
//!
//! # Styling note
//!
//! On this backend `mint_style_class` / `mint_class_for_app` /
//! `supports_js_class_bindings` all take the trait-default path
//! (`None` / `false`): iOS styles are applied per-node via
//! `apply_style`/`apply_styled_variants` and re-fired through the
//! theme-cohort driver on token updates
//! (`token_updates_propagate_via_cascade` is `false`, so cohort
//! fan-out — not CSS cascade — is the token-update mechanism). Any
//! new-core style engine must keep driving the cohort re-apply path
//! on native.
//!
//! # Boot path — [`start`] / [`run_in_view`]
//!
//! iOS splits host duties differently from both siblings: there is no
//! Rust host crate (macOS has `host-appkit`; `host-ios-mobile` is the
//! wgpu *embed* host, not the app host). The UIKit host is the
//! CLI-generated Swift shell (`crates/tools/run/ios/templates`) plus a
//! generated wrapper staticlib whose `ios_main(root_view)` is the
//! old-core boot (`crates/tools/build/ios`). [`run_in_view`] is that
//! wrapper body's new-core equivalent — the iOS counterpart of
//! `host_appkit::newcore::run` — and [`start`] owns everything from
//! "backend is wired to a root view" onward, mirroring
//! `backend_macos::newcore::start` step for step:
//!
//! 1. Install the default monotonic time source. The old `mount`
//!    preamble's other ambient installs (current platform, color
//!    scheme, URL opener, announcer) are runtime-core-private and
//!    skipped, exactly as on the web/macOS new-core boots — public
//!    seams for them are a later-phase migration item.
//! 2. `Registry` (`register_builtins` + the `register` seam) + `World`
//!    + `world.enter(realize)`.
//! 3. `runtime_core::scheduling::drain_buffered_microtasks()` — the
//!    boot opened a mount-buffering window (`begin_mount_buffering`)
//!    before realizing, so microtasks scheduled during the build
//!    (deferred chrome, follow-up layout passes) run HERE,
//!    synchronously, before `finish` — landing in the first
//!    layout/paint. iOS has the same first-paint microtask hazard the
//!    macOS chrome-first-paint-buffering note documents (a
//!    `dispatch_async` microtask lands a run-loop turn AFTER the first
//!    paint); the old iOS wrapper never opted in, so this boot is
//!    stricter than the old one, not looser. A [`schedule_flush`]
//!    issued during the window also buffers and therefore commits
//!    inside this drain — staged writes cannot leak past the first
//!    paint.
//! 4. `Backend::finish(root)` — parents the single root UIView into
//!    the host root and runs the first Taffy layout pass: the exact
//!    root-attachment mechanism of the old mount.
//! 5. `world.flush()` — commit anything staged during mount (ref-fill
//!    callbacks, handler setup) before the first paint.
//! 6. Install the flush driver (the `backend-apple-core`
//!    post-dispatch hook), retain `{Realized, backend, registry,
//!    world}` in the returned [`NewCoreApp`]. The caller (Swift shell
//!    wrapper) stashes the value for the process lifetime, same as the
//!    old wrapper's `OWNER` slot.
//!
//! **Hydration is NOT in scope** (native never hydrates).
//!
//! # Flush driver (design §3) — dispatch-site glue, NO safety net
//!
//! The new kernel stages writes; nothing is observable until the host
//! driver calls [`World::flush`]. iOS uses the SETTLED driver design
//! (the web module's, which post-dates macOS's P4a monitor+timer
//! pair): **precise dispatch-site glue**, no polling.
//!
//! 1. **Author-callback wrapping (this module).** Every
//!    callback-taking capability impl below wraps the author callback
//!    before delegating to the `Backend` machinery: press/click,
//!    input/change, toggle, slider, scroll, hover, wheel, touch, key,
//!    blur/focus, file-drop, image load/error, link activation, portal
//!    dismiss, graphics lifecycle, virtualizer row mount/release,
//!    state setters, and the app-level key handler. The wrapper calls
//!    the author fn, then [`schedule_flush`] — one deduped
//!    `runtime_core::scheduling::schedule_microtask`, which on this
//!    platform is `dispatch_async(main_queue)` (see
//!    `backend_apple_core::scheduler`) and drains on a LATER run-loop
//!    iteration — strictly AFTER the current UIKit event's synchronous
//!    dispatch (gesture recognizers, target-action, delegate
//!    callbacks) completes. Net effect: stage during dispatch, commit
//!    at the run-loop turn boundary right after — the idea-lite
//!    contract. Because the wrapping happens in these new-core-only
//!    impls, the shared old-core event closures are reused verbatim
//!    and the old core never pays for it.
//! 2. **Post-dispatch hook (`backend_apple_core::dispatch_hook`).**
//!    Author code also runs from non-event surfaces: `after_ms`
//!    timers, `after_animation_frame` one-shots, `raf_loop` iterations
//!    (the animation clock), and async-executor future polls. The
//!    apple-core scheduler and executor fire a thread-local hook after
//!    each such callback; [`start`] installs [`schedule_flush`] into
//!    that slot (no-op default, so the old core — and macOS, which
//!    keeps its P4a monitor driver for now — is untouched).
//!
//! **Why no NSEvent-monitor / frame-tick safety net (the macOS P4a
//! shape):** UIKit has no `NSEvent addLocalMonitor…` equivalent — there
//! is no app-level "observe every event before dispatch" seam — and
//! none is needed: unlike AppKit, UIKit controls do NOT run nested
//! `nextEventMatchingMask:` tracking loops that bypass wrapped
//! callbacks (UIControl/UIGestureRecognizer deliver through
//! target-action and delegate callbacks, all of which the backend
//! installs and this module wraps). Every author-code entry point on
//! this backend is either a wrapped callback (list above) or a hooked
//! scheduler/executor surface, so a CADisplayLink poll would only add
//! idle wakeups. Residual surfaces NOT covered (documented, not
//! silent): callbacks installed by `Element::External` third-party
//! iOS glue (the External registry predates the new core; its port
//! must call [`schedule_flush`] after author callbacks), and native
//! nav/system-back surfaces when those handlers are ported (P4/P5).
//!
//! Everything funnels through [`schedule_flush`]/`flush_now`, which
//! skips re-entrant flushes (`world.is_flushing()`) — belt and braces;
//! a main-queue microtask can't actually preempt a synchronous flush.
//!
//! # Module layout / gating
//!
//! Unlike `backend-macos` (whole module `cfg(target_os = "macos")`),
//! the flush-driver glue here is host-compilable and host-TESTED:
//! this crate's UIKit half only builds under `target_os = "ios"`, so
//! `cargo test -p backend-ios-mobile --features new-core` on a mac
//! host is the closest reachable unit-test gate (same posture as the
//! un-gated `splice_policy` / `portal_policy` modules). The Host/caps
//! impls and the boot path are inside the `cfg(target_os = "ios")`
//! section below; they are exercised by the `--target
//! aarch64-apple-ios-sim` build and the live `newcore-ios-smoke` app.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::primitives;
use runtime_world::World;

// Re-exported so the Swift-shell wrapper crates (and the smoke app)
// can name the boot-path types without a direct runtime-scene
// dependency — mirrors `backend_macos::newcore`.
pub use runtime_scene::Element as SceneElement;
pub use runtime_scene::Registry as SceneRegistry;

// ===========================================================================
// Flush driver (host-compilable glue — see module docs on gating)
// ===========================================================================

thread_local! {
    /// The world the flush driver commits. Kept out of [`NewCoreApp`]'s
    /// custody so [`schedule_flush`] never touches app state (a flush
    /// can run while the app value is mid-construction inside `start`).
    static FLUSH_WORLD: RefCell<Option<World>> = const { RefCell::new(None) };
    /// Dedup flag: one queued flush microtask at a time.
    static FLUSH_QUEUED: Cell<bool> = const { Cell::new(false) };
}

fn set_flush_world(world: Option<World>) {
    FLUSH_WORLD.with(|w| *w.borrow_mut() = world);
}

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// The dispatch-site wrappers and the apple-core post-dispatch hook
/// call this right after author-visible dispatch. During a
/// mount-buffering window the microtask buffers and commits inside the
/// boot's synchronous drain (module docs, step 3).
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
            world.flush();
        }
    }
}

/// True while a new-core app is mounted. Core-selection probe for
/// shared transports (robot relay), mirroring `backend_web::newcore`.
pub fn is_booted() -> bool {
    FLUSH_WORLD.with(|w| w.borrow().is_some())
}

/// Synchronously commit staged writes (skipped mid-flush; no-op before
/// boot). Interop seam for callers that stage writes and must return
/// with the tree updated (robot verbs, in-app self-tests) — they
/// cannot ride the async microtask the dispatch-site glue queues.
pub fn flush_sync() {
    flush_now();
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
/// through so the backend's suppress-default decision is unchanged).
fn flushing_key(f: primitives::key::KeyDownHandler) -> primitives::key::KeyDownHandler {
    Rc::new(move |ev| {
        let outcome = f(ev);
        schedule_flush();
        outcome
    })
}

// ===========================================================================
// iOS-only half: boot path + Host + capability-trait delegation
// ===========================================================================

#[cfg(target_os = "ios")]
pub use ios_impl::{run_in_view, start, NewCoreApp};

#[cfg(target_os = "ios")]
mod ios_impl {
    use std::any::{Any, TypeId};
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::rc::Rc;

    use objc2::rc::Retained;
    use objc2_foundation::MainThreadMarker;
    use objc2_ui_kit::UIView;
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
    use crate::imp::{IosBackend, IosNode};

    // =======================================================================
    // Boot path
    // =======================================================================

    /// Everything the boot path must keep alive. Field order is drop
    /// order: the realized tree unmounts before the world (its slots'
    /// owner) dies. The Swift-shell wrapper stashes this for the
    /// process lifetime (the new-core analogue of the old wrapper's
    /// `OWNER` thread-local); [`NewCoreApp::stop`] is the test/teardown
    /// path.
    pub struct NewCoreApp {
        realized: Realized<IosNode>,
        _backend: Rc<RefCell<IosBackend>>,
        _registry: Rc<Registry<IosBackend>>,
        world: World,
    }

    impl NewCoreApp {
        /// Borrow the live tree (tests, diagnostics).
        pub fn with_realized<R>(&self, f: impl FnOnce(&Realized<IosNode>) -> R) -> R {
            f(&self.realized)
        }

        /// The mounted world (tests can flush it explicitly).
        pub fn world(&self) -> &World {
            &self.world
        }

        /// Unmount: unhooks the flush driver (dispatch hook + world
        /// reference), then drops the `Realized` (cleanups fire, views
        /// detach from the live tree's point of view) and the world.
        /// The old wrapper's `ios_teardown` analogue.
        pub fn stop(self) {
            backend_apple_core::dispatch_hook::clear_dispatch_hook();
            set_flush_world(None);
            drop(self);
        }
    }

    /// The iOS counterpart of `host_appkit::newcore::run` — the
    /// new-core body of the CLI wrapper's `ios_main` (see module docs,
    /// "Boot path": iOS host duties live in the generated Swift shell,
    /// so this function IS the host boot). Performs the same idempotent
    /// installs the old-core wrapper performs (scheduler, logger,
    /// global self-handle), opens the mount-buffering window around
    /// [`start`], and hands the retained view to the backend as host
    /// root.
    ///
    /// # Safety
    /// - Must be invoked on the main thread.
    /// - `root_view` must be a non-null, valid `UIView *` (the Swift
    ///   host's view-controller root view), retained by the caller for
    ///   at least the duration of this call.
    pub unsafe fn run_in_view(
        root_view: *mut c_void,
        register: impl FnOnce(&mut Registry<IosBackend>),
        build: impl FnOnce() -> Element,
    ) -> NewCoreApp {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let view: Retained<UIView> = unsafe {
            Retained::retain(root_view as *mut UIView)
                .expect("newcore::run_in_view: root_view must be non-null")
        };

        // Same idempotent installs the CLI wrapper performs — the flush
        // driver rides schedule_microtask, so the scheduler must exist
        // before anything schedules. Logger keeps `log_*` (and the
        // smoke self-test's output) visible in the simulator's system
        // log.
        backend_apple_core::scheduler::install_scheduler();
        backend_apple_core::log::install_logger();

        let mut backend = IosBackend::new(mtm);
        backend.set_host_root(view);
        let backend = Rc::new(RefCell::new(backend));
        // Global weak self-handle: animation writes, deferred syncs,
        // and navigator closures reach the backend through this — same
        // as the old boot.
        crate::imp::install_global_self(Rc::downgrade(&backend));

        // Mount-buffering brackets (module docs, step 3): `start`
        // drains synchronously before `finish`, so deferred build
        // microtasks land in the first paint.
        backend_apple_core::scheduler::begin_mount_buffering();
        let app = start(backend, register, build);
        backend_apple_core::scheduler::end_mount_buffering();
        app
    }

    /// Mount a new-core element tree into an already-view-wired
    /// backend. The caller must have constructed the backend, set its
    /// host root, installed the scheduler + global self-handle, and
    /// opened a mount-buffering window — [`run_in_view`] is the
    /// canonical caller. Mirrors `backend_macos::newcore::start` step
    /// for step (see module docs for the sequence).
    ///
    /// `register` runs after [`runtime_vocabulary::register_builtins`],
    /// so apps/SDKs can register their own payload handlers on the same
    /// registry before the tree realizes. The build closure runs inside
    /// `world.enter`, so free `signal()`/`effect()` calls work;
    /// top-level creations are world-root-owned (they live until app
    /// teardown).
    pub fn start(
        backend: Rc<RefCell<IosBackend>>,
        register: impl FnOnce(&mut Registry<IosBackend>),
        build: impl FnOnce() -> Element,
    ) -> NewCoreApp {
        // Monotonic clock (step 1) — idempotent, first install wins.
        // The old `mount` preamble's other ambient installs are
        // runtime-core-private and skipped, same as web/macOS.
        let platform = backend.borrow().platform();
        runtime_core::time::install_default_time_source(platform);

        let mut registry: Registry<IosBackend> = Registry::new();
        runtime_vocabulary::register_builtins(&mut registry);
        register(&mut registry);
        let registry = Rc::new(registry);

        let world = World::new();
        let realized = world.enter(|| {
            let element = build();
            realize(&backend, &registry, element)
        });

        // Pre-`finish` buffered drain (step 3) — deferred chrome and
        // any build-time `schedule_flush` land before the first
        // layout. Must run with NO backend borrow held (drained tasks
        // re-borrow the backend).
        runtime_core::scheduling::drain_buffered_microtasks();

        // Single-root contract, matching the old-core mount: `finish`
        // parents the root view into the host root and runs the first
        // layout pass.
        let mut roots = realized.collect_nodes();
        let root = match roots.len() {
            1 => roots.pop().expect("len checked"),
            n => panic!(
                "backend_ios::newcore::start: the app root must contribute exactly one \
                 top-level node (got {n}) — wrap fragment/multi-root trees in a view"
            ),
        };
        Backend::finish(&mut *backend.borrow_mut(), root);

        // Commit anything staged during mount before the first paint.
        world.flush();

        // Install the flush driver: schedule_flush becomes reachable
        // from (a) the author-callback wrappers in the caps impls below
        // and (b) the apple-core scheduler/executor post-dispatch hook.
        backend_apple_core::dispatch_hook::install_dispatch_hook(schedule_flush);
        set_flush_world(Some(world.clone()));
        NewCoreApp {
            realized,
            _backend: backend,
            _registry: registry,
            world,
        }
    }

    // =======================================================================
    // Host + capability-trait delegation (generated from
    // backend-macos/src/newcore.rs + backend-web's dispatch-site
    // wrapping — keep mechanically in sync; the scene-parity goldens +
    // the AllCaps bound on register_builtins are the compile gates)
    // =======================================================================

    // -----------------------------------------------------------------------
    // Host — the P1 structural seam
    // -----------------------------------------------------------------------

    impl Host for IosBackend {
        type Node = IosNode;

        fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
            <IosBackend as Backend>::insert(self, parent, child)
        }

        fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
            <IosBackend as Backend>::insert_many(self, parent, children)
        }

        fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
            <IosBackend as Backend>::insert_at(self, parent, child, index)
        }

        fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
            <IosBackend as Backend>::remove_child(self, parent, child)
        }

        fn clear_children(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::clear_children(self, node)
        }

        fn create_anchor(&mut self) -> Self::Node {
            <IosBackend as Backend>::create_reactive_anchor(self)
        }

        fn supports_splice(&self) -> bool {
            <IosBackend as Backend>::supports_child_splice(self)
        }
    }

    // -----------------------------------------------------------------------
    // App environment + lifecycle
    // -----------------------------------------------------------------------

    impl caps::AppEnvOps for IosBackend {
        fn color_scheme(&self) -> ColorScheme {
            <IosBackend as Backend>::color_scheme(self)
        }

        fn platform(&self) -> Platform {
            <IosBackend as Backend>::platform(self)
        }

        fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
            <IosBackend as Backend>::url_opener(self)
        }

        fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
            <IosBackend as Backend>::fullscreen_setter(self)
        }

        fn set_page_metadata(&mut self, meta: &PageMetadata) {
            <IosBackend as Backend>::set_page_metadata(self, meta)
        }

        fn set_app_background(&mut self, color: &Tokenized<Color>) {
            <IosBackend as Backend>::set_app_background(self, color)
        }

        fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
            <IosBackend as Backend>::set_scrollbar_theme(self, thumb, track)
        }

        fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
            // Dispatch-site glue: app-level key handlers run author code
            // (hardware-keyboard events on iPad, simulator keyboards).
            let handler = handler.map(flushing_key);
            <IosBackend as Backend>::set_app_key_handler(self, handler)
        }
    }

    impl caps::LifecycleOps for IosBackend {
        fn finish(&mut self, root: Self::Node) {
            <IosBackend as Backend>::finish(self, root)
        }

        fn run_layout(&mut self) {
            <IosBackend as Backend>::run_layout(self)
        }

        fn schedule_layout_pass() {
            <IosBackend as Backend>::schedule_layout_pass()
        }

        fn is_hydrating(&self) -> bool {
            <IosBackend as Backend>::is_hydrating(self)
        }

        fn renders_lazy_chunks(&self) -> bool {
            <IosBackend as Backend>::renders_lazy_chunks(self)
        }
    }

    // -----------------------------------------------------------------------
    // View + input + pressable
    // -----------------------------------------------------------------------

    impl caps::ViewOps for IosBackend {
        fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
            <IosBackend as Backend>::create_view(self, a11y)
        }

        fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
            <IosBackend as Backend>::make_view_handle(self, node)
        }
    }

    impl caps::InputOps for IosBackend {
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
            <IosBackend as Backend>::install_touch_handler(self, node, handler)
        }

        fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
            <IosBackend as Backend>::claim_touch(self, node, touch_id)
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
            <IosBackend as Backend>::install_wheel_handler(self, node, handler)
        }

        fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
            <IosBackend as Backend>::install_hover_handler(self, node, flushing1(handler))
        }

        fn mark_preserves_focus(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::mark_preserves_focus(self, node)
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
            <IosBackend as Backend>::install_file_drop_handler(self, node, handler)
        }
    }

    impl caps::PressableOps for IosBackend {
        fn create_pressable(
            &mut self,
            on_click: Rc<dyn Fn()>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <IosBackend as Backend>::create_pressable(self, flushing0(on_click), a11y)
        }

        fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
            <IosBackend as Backend>::make_pressable_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Text + button
    // -----------------------------------------------------------------------

    impl caps::TextOps for IosBackend {
        fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
            <IosBackend as Backend>::create_text(self, content, a11y)
        }

        fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
            <IosBackend as Backend>::create_styled_text(self, runs, a11y)
        }

        fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
            <IosBackend as Backend>::update_styled_text(self, node, runs)
        }

        fn update_text(&mut self, node: &Self::Node, content: &str) {
            <IosBackend as Backend>::update_text(self, node, content)
        }

        fn create_text_with_id(
            &mut self,
            content: &str,
            a11y: &AccessibilityProps,
        ) -> Option<(Self::Node, u32)> {
            <IosBackend as Backend>::create_text_with_id(self, content, a11y)
        }

        fn update_text_by_id(&mut self, id: u32, content: String) {
            <IosBackend as Backend>::update_text_by_id(self, id, content)
        }

        fn release_text_id(&mut self, id: u32) {
            <IosBackend as Backend>::release_text_id(self, id)
        }

        fn supports_js_text_bindings(&self) -> bool {
            <IosBackend as Backend>::supports_js_text_bindings(self)
        }

        fn register_reactive_text_binding(
            &mut self,
            text_id: u32,
            signal_ids: &[u64],
            template_parts: &[&str],
            initial_values: &[&str],
            stringifiers: &[Rc<dyn Fn() -> String>],
        ) {
            <IosBackend as Backend>::register_reactive_text_binding(
                self,
                text_id,
                signal_ids,
                template_parts,
                initial_values,
                stringifiers,
            )
        }

        fn release_reactive_text_binding(&mut self, text_id: u32) {
            <IosBackend as Backend>::release_reactive_text_binding(self, text_id)
        }

        fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
            <IosBackend as Backend>::make_text_handle(self, node)
        }
    }

    impl caps::ButtonOps for IosBackend {
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
            <IosBackend as Backend>::create_button(
                self,
                label,
                &on_click,
                leading_icon,
                trailing_icon,
                a11y,
            )
        }

        fn update_button_label(&mut self, node: &Self::Node, label: &str) {
            <IosBackend as Backend>::update_button_label(self, node, label)
        }

        fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
            <IosBackend as Backend>::make_button_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Image + icon + link
    // -----------------------------------------------------------------------

    impl caps::ImageOps for IosBackend {
        fn create_image(
            &mut self,
            src: &str,
            alt: Option<&str>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <IosBackend as Backend>::create_image(self, src, alt, a11y)
        }

        fn update_image_src(&mut self, node: &Self::Node, src: &str) {
            <IosBackend as Backend>::update_image_src(self, node, src)
        }

        fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
            <IosBackend as Backend>::update_image_alt(self, node, alt)
        }

        fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
            // Dispatch-site glue: async image completion runs author
            // code (the IdealystImageView observer path).
            let handler: ImageLoadHandler = {
                let f = handler;
                Rc::new(move |ev| {
                    f(ev);
                    schedule_flush();
                })
            };
            <IosBackend as Backend>::install_image_load_handler(self, node, handler)
        }

        fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
            <IosBackend as Backend>::install_image_error_handler(self, node, flushing0(handler))
        }

        fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
            <IosBackend as Backend>::make_image_handle(self, node)
        }
    }

    impl caps::IconOps for IosBackend {
        fn create_icon(
            &mut self,
            data: &primitives::icon::IconData,
            color: Option<&Color>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <IosBackend as Backend>::create_icon(self, data, color, a11y)
        }

        fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
            <IosBackend as Backend>::update_icon_color(self, node, color)
        }

        fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
            <IosBackend as Backend>::update_icon_data(self, node, data)
        }

        fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
            <IosBackend as Backend>::update_icon_stroke(self, node, progress)
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
            <IosBackend as Backend>::animate_icon_stroke(
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
            <IosBackend as Backend>::make_icon_handle(self, node)
        }
    }

    impl caps::LinkOps for IosBackend {
        fn create_link(
            &mut self,
            config: primitives::link::LinkConfig,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            // Dispatch-site glue: link activation dispatches navigation
            // (stages nav-queue tick signals on the new core).
            let mut config = config;
            config.on_activate = flushing0(config.on_activate.clone());
            <IosBackend as Backend>::create_link(self, config, a11y)
        }

        fn update_link_url(&mut self, node: &Self::Node, url: &str) {
            <IosBackend as Backend>::update_link_url(self, node, url)
        }

        fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
            <IosBackend as Backend>::make_link_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Form widgets
    // -----------------------------------------------------------------------

    impl caps::TextInputOps for IosBackend {
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
            <IosBackend as Backend>::create_text_input(
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
            <IosBackend as Backend>::update_text_input_value(self, node, value)
        }

        fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
            <IosBackend as Backend>::update_text_input_secure(self, node, secure)
        }

        fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
            <IosBackend as Backend>::set_text_input_focus_handler(self, node, flushing1(handler))
        }

        fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
            <IosBackend as Backend>::update_text_input_placeholder(self, node, placeholder)
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
            <IosBackend as Backend>::create_text_area(
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
            <IosBackend as Backend>::update_text_area_value(self, node, value)
        }

        fn make_text_input_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::text_input::TextInputHandle {
            <IosBackend as Backend>::make_text_input_handle(self, node)
        }

        fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
            <IosBackend as Backend>::make_text_area_handle(self, node)
        }
    }

    impl caps::ToggleOps for IosBackend {
        fn create_toggle(
            &mut self,
            initial_value: bool,
            on_change: Rc<dyn Fn(bool)>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <IosBackend as Backend>::create_toggle(self, initial_value, flushing1(on_change), a11y)
        }

        fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
            <IosBackend as Backend>::update_toggle_value(self, node, value)
        }

        fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
            <IosBackend as Backend>::make_toggle_handle(self, node)
        }
    }

    impl caps::SliderOps for IosBackend {
        fn create_slider(
            &mut self,
            initial_value: f32,
            min: f32,
            max: f32,
            step: Option<f32>,
            on_change: Rc<dyn Fn(f32)>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <IosBackend as Backend>::create_slider(
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
            <IosBackend as Backend>::update_slider_value(self, node, value)
        }

        fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
            <IosBackend as Backend>::make_slider_handle(self, node)
        }
    }

    impl caps::ActivityIndicatorOps for IosBackend {
        fn create_activity_indicator(
            &mut self,
            size: primitives::activity_indicator::ActivityIndicatorSize,
            color: Option<&Color>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <IosBackend as Backend>::create_activity_indicator(self, size, color, a11y)
        }

        fn update_activity_indicator_size(
            &mut self,
            node: &Self::Node,
            size: primitives::activity_indicator::ActivityIndicatorSize,
        ) {
            <IosBackend as Backend>::update_activity_indicator_size(self, node, size)
        }

        fn make_activity_indicator_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::activity_indicator::ActivityIndicatorHandle {
            <IosBackend as Backend>::make_activity_indicator_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Scroll + safe area + virtualizer
    // -----------------------------------------------------------------------

    impl caps::ScrollOps for IosBackend {
        fn create_scroll_view(
            &mut self,
            horizontal: bool,
            on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            // Dispatch-site glue: on_scroll fires per scroll delegate
            // callback; the flush microtask is deduped so a burst costs
            // one commit. (The iOS on_scroll delivery is already
            // microtask-deferred — see the apple on_scroll async
            // note — so the wrapper adds a flush after that deferred
            // author call, same ordering as every other callback.)
            let on_scroll = on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
                Rc::new(move |x, y| {
                    f(x, y);
                    schedule_flush();
                })
            });
            <IosBackend as Backend>::create_scroll_view(self, horizontal, on_scroll, a11y)
        }

        fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
            <IosBackend as Backend>::node_scroll(self, node)
        }

        fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
            <IosBackend as Backend>::set_node_scroll(self, node, x, y)
        }

        fn make_scroll_view_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::scroll_view::ScrollViewHandle {
            <IosBackend as Backend>::make_scroll_view_handle(self, node)
        }
    }

    impl caps::SafeAreaOps for IosBackend {
        fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
            <IosBackend as Backend>::apply_safe_area_padding(self, node, sides)
        }

        fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
            <IosBackend as Backend>::apply_scroll_view_safe_area_inset(self, node, sides)
        }
    }

    impl caps::VirtualizerOps for IosBackend {
        fn create_virtualizer(
            &mut self,
            callbacks: VirtualizerCallbacks<Self::Node>,
            overscan: f32,
            layout: primitives::virtualizer::VirtualLayout,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            // Dispatch-site glue: mount/release run author render
            // closures and scope cleanups (which may stage writes) from
            // the backend's own scroll handling; measured-size reports
            // feed the handler's layout cache. item_count/item_key/
            // item_size are pure reads and stay unwrapped.
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
            <IosBackend as Backend>::create_virtualizer(self, callbacks, overscan, layout, a11y)
        }

        fn virtualizer_data_changed(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::virtualizer_data_changed(self, node)
        }

        fn release_virtualizer(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::release_virtualizer(self, node)
        }

        fn make_virtualizer_handle(
            &self,
            node: &Self::Node,
        ) -> primitives::virtualizer::VirtualizerHandle {
            <IosBackend as Backend>::make_virtualizer_handle(self, node)
        }
    }

    // -----------------------------------------------------------------------
    // Graphics + portal + presence + navigator
    // -----------------------------------------------------------------------

    impl caps::GraphicsOps for IosBackend {
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
            <IosBackend as Backend>::create_graphics(self, on_ready, on_resize, on_lost, a11y)
        }

        fn release_graphics(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::release_graphics(self, node)
        }

        fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
            <IosBackend as Backend>::make_graphics_handle(self, node)
        }
    }

    impl caps::PortalOps for IosBackend {
        fn create_portal(
            &mut self,
            target: primitives::portal::PortalTarget,
            on_dismiss: Option<Rc<dyn Fn()>>,
            trap_focus: bool,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            // Dispatch-site glue: backdrop tap dismissal runs the
            // author's on_dismiss.
            let on_dismiss = on_dismiss.map(flushing0);
            <IosBackend as Backend>::create_portal(self, target, on_dismiss, trap_focus, a11y)
        }

        fn release_portal(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::release_portal(self, node)
        }

        fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
            <IosBackend as Backend>::set_portal_hidden(self, node, hidden)
        }

        fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
            <IosBackend as Backend>::make_portal_handle(self, node)
        }
    }

    impl caps::PresenceOps for IosBackend {
        fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
            <IosBackend as Backend>::create_presence_placeholder(self, a11y)
        }

        fn apply_presence(
            &mut self,
            node: &Self::Node,
            state: primitives::presence::PresenceState,
            transition: Option<(u32, Easing)>,
        ) {
            <IosBackend as Backend>::apply_presence(self, node, state, transition)
        }

        fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
            <IosBackend as Backend>::make_presence_handle(self, node)
        }
    }

    impl caps::NavigatorOps for IosBackend {
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
            // own screens; author-initiated navigation stages via
            // handlers already wrapped above and commits inside the
            // flush). Native push surfaces / system back are a named
            // P4/P5 port — their glue must call `schedule_flush` when
            // they land.
            <IosBackend as Backend>::create_navigator(
                self,
                type_id,
                type_name,
                presentation,
                host,
                a11y,
            )
        }

        fn release_navigator(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::release_navigator(self, node)
        }

        fn apply_navigator_slot_style(
            &mut self,
            node: &Self::Node,
            slot: &'static str,
            style: &Rc<StyleRules>,
        ) {
            <IosBackend as Backend>::apply_navigator_slot_style(self, node, slot, style)
        }

        fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
            <IosBackend as Backend>::make_navigator_handle(self, node)
        }

        fn navigator_attach_initial(
            &mut self,
            navigator: &Self::Node,
            screen: Self::Node,
            scope_id: u64,
            options: Box<dyn Any>,
        ) {
            <IosBackend as Backend>::navigator_attach_initial(self, navigator, screen, scope_id, options)
        }
    }

    // -----------------------------------------------------------------------
    // External + document
    // -----------------------------------------------------------------------

    impl caps::ExternalOps for IosBackend {
        fn create_external(
            &mut self,
            type_id: TypeId,
            type_name: &'static str,
            payload: &Rc<dyn Any>,
            a11y: &AccessibilityProps,
        ) -> Self::Node {
            <IosBackend as Backend>::create_external(self, type_id, type_name, payload, a11y)
        }

        fn release_external(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::release_external(self, node)
        }

        fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
            <IosBackend as Backend>::missing_primitive_placeholder(self, label)
        }
    }

    impl caps::DocumentOps for IosBackend {
        fn create_element(&mut self, tag: &str) -> Self::Node {
            <IosBackend as Backend>::create_element(self, tag)
        }

        fn attach_html_id(&self, node: &Self::Node, id: &str) {
            <IosBackend as Backend>::attach_html_id(self, node, id)
        }

        fn attach_html_class(&self, node: &Self::Node, class: &str) {
            <IosBackend as Backend>::attach_html_class(self, node, class)
        }

        fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
            <IosBackend as Backend>::attach_html_style(self, node, prop, value)
        }

        fn register_raw_css(&mut self, css: &str) {
            <IosBackend as Backend>::register_raw_css(self, css)
        }
    }

    // -----------------------------------------------------------------------
    // Style + assets
    // -----------------------------------------------------------------------

    impl caps::StyleOps for IosBackend {
        fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
            <IosBackend as Backend>::apply_style(self, node, style)
        }

        fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
            <IosBackend as Backend>::mint_style_class(self, style)
        }

        fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
            <IosBackend as Backend>::mint_class_for_app(self, app)
        }

        fn apply_styled_states(
            &mut self,
            node: &Self::Node,
            base: &Rc<StyleRules>,
            overlays: &[(StateBits, Rc<StyleRules>)],
        ) {
            <IosBackend as Backend>::apply_styled_states(self, node, base, overlays)
        }

        fn apply_styled_variants(
            &mut self,
            node: &Self::Node,
            base: &Rc<StyleRules>,
            state_overlays: &[(StateBits, Rc<StyleRules>)],
            breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
            container_overlays: &[(f32, Rc<StyleRules>)],
        ) {
            <IosBackend as Backend>::apply_styled_variants(
                self,
                node,
                base,
                state_overlays,
                breakpoint_overlays,
                container_overlays,
            )
        }

        fn mark_container(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::mark_container(self, node)
        }

        fn handles_states_natively(&self) -> bool {
            <IosBackend as Backend>::handles_states_natively(self)
        }

        fn token_updates_propagate_via_cascade(&self) -> bool {
            <IosBackend as Backend>::token_updates_propagate_via_cascade(self)
        }

        fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
            <IosBackend as Backend>::register_stylesheet(self, rules)
        }

        fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
            <IosBackend as Backend>::unregister_stylesheet(self, rules)
        }

        fn install_tokens(&mut self, tokens: &[TokenEntry]) {
            <IosBackend as Backend>::install_tokens(self, tokens)
        }

        fn update_tokens(&mut self, tokens: &[TokenEntry]) {
            <IosBackend as Backend>::update_tokens(self, tokens)
        }

        fn on_node_unstyled(&mut self, node: &Self::Node) {
            <IosBackend as Backend>::on_node_unstyled(self, node)
        }

        fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
            // Dispatch-site glue: hover/press/focus state flips can
            // stage writes when the style path routes states through
            // signals — on native this IS the live state path (no CSS
            // pseudo-classes), so the wrapper matters here, unlike web.
            let setter: Rc<dyn Fn(StateBits, bool)> = {
                let f = setter;
                Rc::new(move |bits, on| {
                    f(bits, on);
                    schedule_flush();
                })
            };
            <IosBackend as Backend>::attach_states(self, node, setter)
        }

        fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
            <IosBackend as Backend>::set_disabled(self, node, disabled)
        }

        fn supports_preminted_styles(&self) -> bool {
            <IosBackend as Backend>::supports_preminted_styles(self)
        }

        fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {
            <IosBackend as Backend>::apply_default_text_font(self, font)
        }

        fn supports_js_class_bindings(&self) -> bool {
            <IosBackend as Backend>::supports_js_class_bindings(self)
        }

        fn register_reactive_class_binding(
            &mut self,
            node: &Self::Node,
            signal_id: u64,
            values: &[u32],
            classes: &[&str],
            value_reader: Rc<dyn Fn() -> u32>,
        ) -> u32 {
            <IosBackend as Backend>::register_reactive_class_binding(
                self,
                node,
                signal_id,
                values,
                classes,
                value_reader,
            )
        }

        fn release_reactive_class_binding(&mut self, binding_id: u32) {
            <IosBackend as Backend>::release_reactive_class_binding(self, binding_id)
        }
    }

    impl caps::AssetOps for IosBackend {
        fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
            <IosBackend as Backend>::register_asset(self, id, kind, source)
        }

        fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
            <IosBackend as Backend>::unregister_asset(self, id, kind)
        }

        fn register_typeface(
            &mut self,
            id: TypefaceId,
            family_name: &str,
            faces: &[TypefaceFace],
            fallback: SystemFallback,
        ) {
            <IosBackend as Backend>::register_typeface(self, id, family_name, faces, fallback)
        }

        fn unregister_typeface(&mut self, id: TypefaceId) {
            <IosBackend as Backend>::unregister_typeface(self, id)
        }
    }

    // -----------------------------------------------------------------------
    // A11y + animation + introspection
    // -----------------------------------------------------------------------

    impl caps::A11yOps for IosBackend {
        fn update_accessibility(
            &mut self,
            node: &Self::Node,
            a11y: &AccessibilityProps,
            inferred_role: Option<Role>,
        ) {
            <IosBackend as Backend>::update_accessibility(self, node, a11y, inferred_role)
        }

        fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
            <IosBackend as Backend>::announce_for_accessibility(self, msg, priority)
        }

        fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
            <IosBackend as Backend>::dump_accessibility_tree(self)
        }
    }

    impl caps::AnimationOps for IosBackend {
        fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
            <IosBackend as Backend>::set_animated_f32(self, node, prop, value)
        }

        fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
            <IosBackend as Backend>::set_animated_color(self, node, prop, value)
        }
    }

    impl caps::IntrospectionOps for IosBackend {
        fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
            <IosBackend as Backend>::frame(self, node)
        }

        fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
            <IosBackend as Backend>::absolute_frame(self, node)
        }

        fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
            <IosBackend as Backend>::device_frame(self, node)
        }

        fn supports_native_introspection(&self) -> bool {
            <IosBackend as Backend>::supports_native_introspection(self)
        }

        fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
            <IosBackend as Backend>::introspect_native(self, node)
        }

        fn note_introspection_root(&self, node: &Self::Node) {
            <IosBackend as Backend>::note_introspection_root(self, node)
        }

        fn supports_screenshot(&self) -> bool {
            <IosBackend as Backend>::supports_screenshot(self)
        }

        fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
            <IosBackend as Backend>::capture_screenshot(self, done)
        }
    }

    // -----------------------------------------------------------------------
    // Batch + wire bindings
    // -----------------------------------------------------------------------

    impl caps::BatchOps for IosBackend {
        fn supports_batched_repeat(&self) -> bool {
            <IosBackend as Backend>::supports_batched_repeat(self)
        }

        fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
            <IosBackend as Backend>::execute_batch(self, batch)
        }

        fn execute_batch_with_attach(
            &mut self,
            batch: BackendBatch,
            parent: &mut Self::Node,
            attach_locals: &[u32],
        ) -> Vec<Self::Node> {
            <IosBackend as Backend>::execute_batch_with_attach(self, batch, parent, attach_locals)
        }
    }

    impl caps::WireBindingOps for IosBackend {
        fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
            <IosBackend as Backend>::note_text_binding(self, node, signal_ids, method)
        }

        fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value) {
            <IosBackend as Backend>::note_signal_initial(self, signal_id, value)
        }

        fn note_when_binding(
            &mut self,
            anchor: &Self::Node,
            signal_ids: &[u64],
            cond_method: &'static str,
            then_node: &Self::Node,
            otherwise_node: &Self::Node,
        ) {
            <IosBackend as Backend>::note_when_binding(
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
            <IosBackend as Backend>::note_switch_binding(
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
            <IosBackend as Backend>::note_repeat_binding(
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
            <IosBackend as Backend>::note_virtualizer_binding(
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
            <IosBackend as Backend>::supports_lazy_slot_capture(self)
        }

        fn begin_slot_capture(&mut self) {
            <IosBackend as Backend>::begin_slot_capture(self)
        }

        fn end_slot_capture(&mut self, slot_root: &Self::Node) {
            <IosBackend as Backend>::end_slot_capture(self, slot_root)
        }
    }
}

// ===========================================================================
// Host-runnable tests — flush-driver glue only. The Host/caps
// delegation and the boot path compile only for `target_os = "ios"`
// (UIKit objects), so they are exercised by the `--target
// aarch64-apple-ios-sim` build and by launching `newcore-ios-smoke` on
// the simulator (see that crate). Mirrors the macOS newcore test
// module, which has the same seam for AppKit.
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
        // below keeps every microtask on THIS thread. (On a non-Apple
        // host the apple-core scheduler isn't compiled; this test runs
        // on macOS hosts, where it is — same posture as apple-core's
        // own scheduler tests.)
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

        runtime_core::scheduling::drain_buffered_microtasks();
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
        runtime_core::scheduling::drain_buffered_microtasks();
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
    /// THE driver for timer/frame/future-poll surfaces on iOS (no
    /// NSEvent monitor, no frame poll — module docs). Regression:
    /// installing the hook and firing it after a staged write must
    /// commit the write on the next microtask drain, and clearing the
    /// hook must sever the path (a torn-down app's timers can't flush
    /// a dead world).
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
        runtime_core::scheduling::drain_buffered_microtasks();
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
        runtime_core::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 7], "no commit after teardown");
        backend_apple_core::scheduler::end_mount_buffering();
    }

    /// The dispatch-site wrappers: author callback runs FIRST, then one
    /// deduped flush is queued — for all three shapes (zero-arg,
    /// one-value, key-outcome pass-through).
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
        runtime_core::scheduling::drain_buffered_microtasks();
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
        runtime_core::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 1, 2], "on_change write committed");

        // flushing_key — outcome passes through so the backend's
        // suppress-default decision is unchanged.
        let wrapped = flushing_key(Rc::new(move |_ev| {
            count.set(3);
            runtime_core::primitives::key::KeyOutcome::PreventDefault
        }));
        let ev = runtime_core::primitives::key::KeyEvent {
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
            runtime_core::primitives::key::KeyOutcome::PreventDefault
        ));
        runtime_core::scheduling::drain_buffered_microtasks();
        assert_eq!(*log.borrow(), vec![0, 1, 2, 3], "key write committed");

        set_flush_world(None);
        backend_apple_core::scheduler::end_mount_buffering();
    }
}

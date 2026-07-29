//! New-core adoption for the wgpu backend (idea-lite migration, P5).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on [`WgpuBackend`] — the
//! production shape of the migration: no `LegacyBridge` wrapper in the
//! render path. Every trait method delegates via UFCS
//! (`<WgpuBackend as Backend>::method(self, …)`) to the existing
//! `Backend` impl, so the GPU mechanism code (node/Taffy tree building,
//! glyphon text, the animator, style→render projection, the interaction
//! `Host`'s hit-testing) is REUSED verbatim — this module adds a second
//! *front* onto the same machinery, exactly like
//! `backend-web/src/newcore.rs` does for the DOM and
//! `backend-macos/src/newcore.rs` does for AppKit (this file mirrors
//! those modules mechanically; keep them in sync).
//!
//! # Delegation status — every capability accounted for
//!
//! | Trait | Status |
//! |---|---|
//! | `runtime_scene::Host` (7 ops) | direct (`create_anchor` → `create_reactive_anchor`, `supports_splice` → `supports_child_splice` — the P1 renames) |
//! | `AppEnvOps` | direct |
//! | `LifecycleOps` | direct (`is_hydrating` is always `false` on this backend — no hydration on native) |
//! | `ViewOps` | direct |
//! | `InputOps` | direct |
//! | `PressableOps` | direct |
//! | `TextOps` | direct (the js-binding methods resolve to the trait-default no-ops, same as the old walker: `supports_js_text_bindings` is `false` here) |
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
//! | `DocumentOps` | direct (web-flavored methods resolve to the same trait-default no-ops the old walker hit on this backend) |
//! | `StyleOps` | direct (class-minting methods return the trait-default `None` on native; the vocabulary's `attach_style` then takes the `apply_styled_variants` path — identical routing to the old walker; token updates re-fire through the theme-cohort driver, `token_updates_propagate_via_cascade` is `false`) |
//! | `AssetOps` | direct |
//! | `A11yOps` | direct |
//! | `AnimationOps` | direct |
//! | `IntrospectionOps` | direct |
//! | `BatchOps` | direct |
//! | `WireBindingOps` | direct (wire-recorder no-ops on this backend, same as today) |
//!
//! **30/30 direct, 0 adapted, 0 stubbed.** Nothing panics, nothing
//! silently no-ops beyond what the wrapped `Backend` impl already does.
//! Where the wgpu backend does not override a `Backend` method (the
//! DOM-only and wire-only families), the UFCS call resolves to the same
//! trait-default fallback the old walker would hit — behavior is
//! identical by construction. The impl bodies below are generated
//! mechanically from `backend-macos/src/newcore.rs` (itself generated
//! from `backend-web/src/newcore.rs` / `runtime_vocabulary::bridge`,
//! the compile-time proof of the signature freeze), with
//! `MacosBackend` → `WgpuBackend`, plus the web module's dispatch-site
//! author-callback wrapping (see *Flush driver*).
//!
//! # Boot path — [`start`]
//!
//! Client-render-only mount of a `runtime_scene::Element` tree against
//! the live GPU node tree through the registry. Host duties are split
//! exactly like macOS: the windowed shell (`host-winit`) owns the winit
//! event loop / window / wgpu surface and constructs the interaction
//! [`crate::Host`] (which builds the `WgpuBackend` and installs the
//! global self-handle); THIS function owns everything from "backend
//! exists" onward. `host_winit::newcore::run` is the all-in-one windowed
//! entry apps call; the `headless::Screenshotter` path can drive the
//! same `start` against its own backend for offscreen captures.
//!
//! Sequence (mirrors `runtime_core::mount`'s ordering where they
//! overlap):
//!
//! 1. Install the default monotonic time source (the analogue of web
//!    `start_in`'s `install_time_source` — without it the animation
//!    clock and `PhaseTimer` read 0). The old `mount` preamble's other
//!    ambient installs (current platform, color scheme, URL opener,
//!    announcer) are runtime-core-private and skipped here, exactly as
//!    on the web/macOS new-core boots — public seams for them are a
//!    later-phase migration item.
//! 2. `Registry` (`register_builtins` + the `register` seam) + `World` +
//!    `world.enter(realize)`.
//! 3. `runtime_core::scheduling::drain_buffered_microtasks()` — a no-op
//!    under the winit scheduler (which has no buffering window; deferred
//!    build microtasks arrive as 0 ms timers on the event loop, same as
//!    the old GPU boot), but load-bearing under the **headless buffering
//!    scheduler** (`headless.rs`), where every microtask queued during
//!    the build — including a buffered [`schedule_flush`] — buffers and
//!    must commit here, before `finish`, so a one-shot capture renders
//!    the settled tree. Must run with NO backend borrow held (drained
//!    tasks re-borrow the backend).
//! 4. `Backend::finish(root)` — records the single root in
//!    `backend.roots` (the renderer's frame walk starts there) and
//!    requests a redraw: the exact root-attachment mechanism of the old
//!    mount.
//! 5. `world.flush()` — commit anything staged during mount (ref-fill
//!    callbacks, handler setup) before the first paint.
//! 6. Install the flush driver ([`crate::dispatch_hook`] +
//!    `FLUSH_WORLD`), retain `{Realized, backend, registry, world}` in
//!    the returned [`NewCoreApp`]. The windowed host `std::mem::forget`s
//!    it (the process exits with the event loop, same retention as the
//!    old boot's `forget(owner)`).
//!
//! **Hydration is NOT in scope** (native never hydrates).
//!
//! # Flush driver (the SETTLED web design — precise dispatch-site glue)
//!
//! The new kernel stages writes; nothing is observable until the driver
//! calls [`World::flush`]. There is no event monitor and no per-frame
//! safety net — coverage is exact:
//!
//! 1. **Author-callback wrapping (this module).** Every callback-taking
//!    capability impl below wraps the author callback before delegating
//!    to the `Backend` machinery: press/click, input/change, toggle,
//!    slider, scroll, hover, wheel, touch, key, blur/focus, file-drop,
//!    image load/error, link activation, portal dismiss, graphics
//!    lifecycle, virtualizer row mount/release/measure, state setters,
//!    and the app-level key handler. The wrapper calls the author fn,
//!    then [`schedule_flush`] — one deduped
//!    `runtime_core::scheduling::schedule_microtask` → `world.flush()`.
//!    The interaction `Host` (`host.rs`) dispatches *these wrapped
//!    closures* from whatever surface invokes them — winit
//!    `WindowEvent`s, the on-screen keyboard router, momentum-scroll
//!    ticks inside `Host::tick` — so every author entry through the
//!    interaction layer is covered regardless of the triggering event,
//!    with zero edits to `host.rs`. Because the wrapping happens in
//!    these new-core-only impls, the shared old-core interaction code is
//!    reused verbatim and the old core never pays for it.
//! 2. **Post-dispatch hook ([`crate::dispatch_hook`]).** Author code
//!    also runs from the scheduler: `after_ms` timers,
//!    `after_animation_frame` one-shots, and `raf_loop` iterations. The
//!    `host-winit` scheduler fires the thread-local hook after each such
//!    callback ([`start`] installs [`schedule_flush`] into the slot;
//!    no-op default, so the old core is untouched). Scheduled
//!    *microtasks* deliberately do NOT fire the hook — on this host a
//!    microtask IS a 0 ms timer, and the flush itself rides one, so
//!    hooking microtasks would re-arm the flush forever (see
//!    `dispatch_hook`'s module docs and the Android precedent).
//!
//! **No frame-tick safety net** — the winit host has no AppKit-style
//! control tracking loop that bypasses installed callbacks (every input
//! reaches author code through the wrapped closures above), so there is
//! nothing left for a per-frame poll to catch. Mirrors iOS/Android.
//!
//! Residual surfaces NOT covered (documented, not silent):
//! - `Element::External` GPU glue that installs its own callbacks (the
//!   External registry predates the new core; its port must call
//!   [`schedule_flush`] after author callbacks).
//! - The old-core navigator header chrome (skin-painted back button →
//!   `NavigatorControl::dispatch`): the vocabulary navigator handlers
//!   never call `create_navigator` on this backend, so that chrome does
//!   not exist under a new-core mount — native nav-surface parity is
//!   the same P5/P6 seam documented for iOS/Android.
//! - Futures: no async executor is installed on this host today
//!   (`runtime_core::driver::spawn` has no wgpu executor); when one is
//!   added it must fire the dispatch hook per poll like web's.
//!
//! Everything funnels through [`schedule_flush`]/`flush_now`, which
//! skips re-entrant flushes (`world.is_flushing()`) — belt and braces;
//! a queued microtask can't actually preempt a synchronous flush.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

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

use crate::node::WgpuNode;
use crate::WgpuBackend;

// Re-exported so the host shell (`host-winit`) and app wrappers can
// name the boot-path types without a direct runtime-scene dependency —
// mirrors `backend_macos::newcore`.
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
    /// Weak handle to the mounted backend, for diagnostics
    /// ([`with_backend`]) — the GPU analogue of the macOS smoke's
    /// NSApplication window walk (this backend has no global window to
    /// count views under, so the boot path exposes its own seam).
    static BACKEND: RefCell<Option<Weak<RefCell<WgpuBackend>>>> = const { RefCell::new(None) };
}

/// Everything the boot path must keep alive. Field order is drop order:
/// the realized tree unmounts before the collected top-level state
/// (embedded boots) and the world (its slots' owner) die.
/// The windowed host typically `std::mem::forget`s this before entering
/// the event loop (same retention as the old boot's `forget(owner)` —
/// the process exits with the event loop).
pub struct NewCoreApp {
    realized: Realized<WgpuNode>,
    /// Embedded boots only ([`start_in_world`]): the `collect_owned`
    /// harvest of the build — top-level effects (AV bind keepalives,
    /// scoped-scheduling anchors) that would otherwise be owned by the
    /// EMBEDDING world's root and leak past this app's unmount. `None`
    /// on the windowed [`start`] path, where the world is app-lifetime
    /// and root ownership is exactly right.
    owned: Option<runtime_world::Owned>,
    _backend: Rc<RefCell<WgpuBackend>>,
    _registry: Rc<Registry<WgpuBackend>>,
    world: World,
    /// True for [`start_in_world`] boots — [`stop`] then leaves the
    /// host-lifetime driver state alone (see `stop`'s comments).
    embedded: bool,
}

impl NewCoreApp {
    /// Borrow the live tree (tests, diagnostics).
    pub fn with_realized<R>(&self, f: impl FnOnce(&Realized<WgpuNode>) -> R) -> R {
        f(&self.realized)
    }

    /// The mounted world (tests can flush it explicitly).
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Unmount: drops the `Realized` (cleanups fire, nodes detach from
    /// the live tree's point of view), uninstalls the flush driver, and
    /// drops the world. Primarily for tests; a windowed app forgets the
    /// value instead.
    ///
    /// Embedded boots ([`start_in_world`]) deliberately do NOT clear
    /// `FLUSH_WORLD` or the dispatch hook: the world belongs to the
    /// embedding host and outlives this app, and a REPLACEMENT embedded
    /// app may already have mounted before this one drops (the
    /// swap-remount ordering isn't guaranteed) — clearing here would
    /// sever the replacement's author-callback flush path. A stray
    /// `schedule_flush` against the host world is a benign no-op-shaped
    /// flush. The `BACKEND` diagnostic weak IS cleared when it still
    /// points at this app's backend (pointer-compared, so a
    /// replacement's install survives).
    pub fn stop(self) {
        if !self.embedded {
            crate::dispatch_hook::clear_dispatch_hook();
            set_viewport_sink(None);
            set_flush_world(None);
            BACKEND.with(|b| *b.borrow_mut() = None);
        } else {
            BACKEND.with(|b| {
                let mut slot = b.borrow_mut();
                let is_this = slot
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .is_some_and(|rc| Rc::ptr_eq(&rc, &self._backend));
                if is_this {
                    *slot = None;
                }
            });
        }
        drop(self);
    }
}

/// Mount a new-core element tree into an already-constructed backend.
///
/// The host must have: constructed the backend (typically via
/// [`crate::Host::new`], which also installs the global self-handle the
/// animation writers use) and installed a scheduler (the flush driver
/// rides `schedule_microtask`; the winit host installs its scheduler
/// before the event loop runs, the headless path installs the buffering
/// scheduler). See the module docs for the exact sequence and
/// `host_winit::newcore::run` for the canonical windowed caller.
///
/// `register` runs after [`runtime_vocabulary::register_builtins`], so
/// apps/SDKs can register their own payload handlers on the same
/// registry before the tree realizes. The build closure runs inside
/// `world.enter`, so free `signal()`/`effect()` calls work; top-level
/// creations are world-root-owned (they live until app teardown).
pub fn start(
    backend: Rc<RefCell<WgpuBackend>>,
    register: impl FnOnce(&mut Registry<WgpuBackend>),
    build: impl FnOnce() -> Element,
) -> NewCoreApp {
    // Monotonic clock (step 1 in the module docs). Idempotent, first
    // install wins. The platform identity comes from the active skin
    // (`NativeSkin` reports the real host OS; sim skins report their
    // pretend platform) — same value the old boot would install.
    let platform = backend.borrow().platform();
    runtime_core::time::install_default_time_source(platform);

    let mut registry: Registry<WgpuBackend> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    let world = World::new();
    let (vp_sig, realized) = world.enter(|| {
        let element = build();
        let realized = realize(&backend, &registry, element);
        // Capture the per-world viewport ctx AFTER the build, never
        // before: the ctx's bucket memo pins the breakpoint TABLE at
        // creation and apps `install_breakpoints` inside their root
        // component (see the viewport-source section and backend-web's
        // identical ordering comment).
        let vp_sig = runtime_vocabulary::viewport::viewport_ctx().size_signal();
        (vp_sig, realized)
    });

    // Buffered-microtask drain (step 3) — a no-op on the winit
    // scheduler, load-bearing on the headless buffering scheduler.
    // Must run with NO backend borrow held (drained tasks re-borrow).
    // ENTERED (the web hydrate boot's pattern): a task buffered during
    // mount may do creation-side work — the deferred virtualizer fill
    // realizes rows — and `FLUSH_WORLD` isn't stored yet at this point,
    // so `enter_mounted_world`'s stored-world route can't cover it; the
    // ambient enter does. On the winit scheduler the same fill runs
    // post-`start` (0 ms timer), where the stored world covers it.
    world.enter(runtime_core::scheduling::drain_buffered_microtasks);

    // Single-root contract, matching the old-core mount: `finish`
    // records the root for the renderer's frame walk and requests a
    // redraw.
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "render_wgpu::newcore::start: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    Backend::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount before the first paint.
    world.flush();

    // Install the flush driver: schedule_flush becomes reachable from
    // (a) the author-callback wrappers in the caps impls below and
    // (b) the host scheduler's post-dispatch hook.
    crate::dispatch_hook::install_dispatch_hook(schedule_flush);
    set_flush_world(Some(world.clone()));
    // Live viewport source: `Host::set_viewport` now reaches the
    // world's ctx through `forward_viewport` (windowed boots only —
    // see the viewport-source section for the embed rationale).
    set_viewport_sink(Some(vp_sig));
    BACKEND.with(|b| *b.borrow_mut() = Some(Rc::downgrade(&backend)));
    NewCoreApp {
        realized,
        owned: None,
        _backend: backend,
        _registry: registry,
        world,
        embedded: false,
    }
}

/// Mount a new-core element tree into an already-constructed backend,
/// realizing it into an EXISTING world owned by an embedding host —
/// the embedded-preview seam (the marketing website's wgpu Simulator
/// running the `welcome` app inside a `backend-web` page is the
/// consumer, via `host_web::mount_newcore`).
///
/// Differences from [`start`], each load-bearing:
///
/// - **No `World::new()`** — the tree realizes into `world` (for the
///   web embed: the page's own mounted world). One thread, one world,
///   one logical update stream: the embedding host's existing flush
///   driver (backend-web's dispatch-site glue + scheduler/executor
///   post-dispatch hook) commits writes staged by the embedded app's
///   timers, raf loops, and future polls with no second driver. This
///   backend's own author-callback wrappers still call
///   [`schedule_flush`], which now flushes the same shared world
///   (`FLUSH_WORLD` is set below), covering input dispatched through
///   the wgpu interaction `Host` (canvas pointer/wheel listeners) that
///   never crosses the embedding backend's glue.
/// - **`collect_owned` around the build** — top-level creations
///   (`AnimatedValue::bind` keepalives, scoped-scheduling anchors,
///   free effects in the app's root fn) must die when THIS app
///   unmounts, not when the page-lifetime world does. The harvest
///   rides in [`NewCoreApp::owned`]; drop order (realized first)
///   preserves the unmount-before-owner contract.
/// - **No dispatch-hook install** — the hook slot belongs to whichever
///   host scheduler drives the thread. On web that's backend-web's
///   (already installed by its boot); this crate's own slot is only
///   fired by `host-winit`, which is not present in an embed.
/// - **No viewport-sink install** — the shared world's `ViewportCtx`
///   belongs to the embedding page, whose own resize listener
///   (backend-web's viewport source) is its live source; forwarding
///   `Host::set_viewport` here would stamp the sim profile's fixed
///   logical size over the page's real viewport (see the
///   viewport-source section).
///
/// The host must have constructed the backend (via [`crate::Host::new`],
/// which installs the global self-handle) and a scheduler + time source
/// must be installed (the embedding page host's boot did both).
pub fn start_in_world(
    backend: Rc<RefCell<WgpuBackend>>,
    register: impl FnOnce(&mut Registry<WgpuBackend>),
    build: impl FnOnce() -> Element,
    world: World,
) -> NewCoreApp {
    let mut registry: Registry<WgpuBackend> = Registry::new();
    runtime_vocabulary::register_builtins(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    let (realized, owned) = world.enter(|| {
        runtime_world::collect_owned(|| {
            let element = build();
            realize(&backend, &registry, element)
        })
    });

    // Buffered-microtask drain — same contract as `start` (a no-op on
    // real-microtask schedulers like the web's; load-bearing under a
    // buffering test/headless scheduler). Entered for the same
    // creation-side reasons.
    world.enter(runtime_core::scheduling::drain_buffered_microtasks);

    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "render_wgpu::newcore::start_in_world: the app root must contribute exactly \
             one top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    Backend::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount before the first paint. The
    // shared world can't be mid-flush here: embeds mount from async
    // host init (executor callback), never from inside an effect.
    world.flush();

    set_flush_world(Some(world.clone()));
    BACKEND.with(|b| *b.borrow_mut() = Some(Rc::downgrade(&backend)));
    NewCoreApp {
        realized,
        owned: Some(owned),
        _backend: backend,
        _registry: registry,
        world,
        embedded: true,
    }
}

fn set_flush_world(world: Option<World>) {
    FLUSH_WORLD.with(|w| *w.borrow_mut() = world);
}

/// True while a new-core app is mounted (`start` ran, `stop` hasn't).
pub fn is_booted() -> bool {
    FLUSH_WORLD.with(|w| w.borrow().is_some())
}

/// Run `f` with the mounted backend (diagnostics: the smoke app's
/// self-test counts live nodes through this). `None` before [`start`]
/// or after the backend dropped.
pub fn with_backend<R>(f: impl FnOnce(&Rc<RefCell<WgpuBackend>>) -> R) -> Option<R> {
    let rc = BACKEND.with(|b| b.borrow().as_ref().and_then(Weak::upgrade));
    rc.map(|rc| f(&rc))
}

// ===========================================================================
// Flush driver
// ===========================================================================

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// The author-callback wrappers and the scheduler's dispatch hook call
/// this right after author-visible dispatch. Under the headless
/// buffering scheduler the microtask buffers and commits inside
/// [`start`]'s synchronous drain (module docs, step 3).
pub fn schedule_flush() {
    if FLUSH_QUEUED.with(|q| q.replace(true)) {
        return;
    }
    runtime_core::scheduling::schedule_microtask(|| {
        FLUSH_QUEUED.with(|q| q.set(false));
        flush_now();
    });
}

/// Synchronously commit staged writes (skipped mid-flush; no-op before
/// [`start`]). Harness seam: a driver that staged writes and must
/// observe the committed tree before returning (tests, a future robot
/// transport) cannot ride the async microtask — it flushes before
/// returning instead.
pub fn flush_sync() {
    flush_now();
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
// Viewport source (the new-core GPU viewport seam)
// ===========================================================================
//
// The vocabulary's per-world viewport/breakpoint ctx
// (`runtime_vocabulary::viewport`) is SEED-ONLY unless the platform
// pushes live sizes (its module docs). On this backend the LOGICAL
// viewport is what author layout/breakpoints run against, and its one
// source of truth is [`crate::Host::set_viewport`] — the shell calls it
// with the profile's logical size at window creation (and would on any
// future logical change; a WINDOW resize deliberately does NOT change
// the logical size — the winit host letterboxes, see `DeviceProfile::
// logical_size`). That seam forwards here so the mounted world's ctx
// tracks the logical viewport exactly like the other backends track
// their windows: same values, same bucket transitions, no
// backend-specific thresholds — the mechanism differs (fixed logical
// size vs live window), the observable contract converges.
//
// Old-TLS seeding split: `Host::set_viewport` must NOT write the
// old-core TLS value itself — an EMBEDDED simulator (`host_web::
// mount_newcore`, the website's in-page wgpu preview) shares its
// thread's TLS with the embedding page, and stamping the sim profile
// over the page's real viewport would collapse the page to phone
// breakpoints. The WINDOWED host (`host_winit::newcore::run_with`)
// seeds the TLS pre-mount instead, where the window owns the thread.
// The sink is equally embed-safe: only the windowed [`start`] installs
// it ([`start_in_world`] leaves the page backend's own resize listener
// as the ctx's source).
//
// Discipline (mirrors `backend_web::newcore`'s resize listener): the
// seam runs OUTSIDE `World::enter`, so the boot CAPTURES the world's
// signal handle — capture, don't inject — and the push stages through
// the handle (routes to its own world, equality-guarded, dead-world
// writes are silent no-ops) then rides one deduped [`schedule_flush`].

thread_local! {
    /// The mounted world's viewport signal (`Copy` handle). `None`
    /// outside a windowed new-core boot, so `Host::set_viewport` costs
    /// one TLS read and nothing else on old-core and embedded paths.
    static VIEWPORT_SINK: Cell<Option<runtime_world::Signal<runtime_core::ViewportSize>>> =
        const { Cell::new(None) };
}

fn set_viewport_sink(sig: Option<runtime_world::Signal<runtime_core::ViewportSize>>) {
    VIEWPORT_SINK.with(|s| s.set(sig));
}

/// Forward one logical-viewport report into the mounted world's
/// viewport ctx (no-op before [`start`] / after teardown / on embedded
/// boots). Called by [`crate::Host::set_viewport`].
pub(crate) fn forward_viewport(size: runtime_core::ViewportSize) {
    let Some(sig) = VIEWPORT_SINK.with(|s| s.get()) else {
        return;
    };
    // Staged write outside `enter` + one deduped flush — commits on
    // the next scheduler turn, like every wrapped callback.
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
// (instead of inside the shared interaction `Host` / `Backend` code)
// keeps the old-core render path byte-identical: the old core applies
// writes synchronously and must not pay a flush per event.

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
/// through so the backend's consume/propagate decision is unchanged).
fn flushing_key(f: primitives::key::KeyDownHandler) -> primitives::key::KeyDownHandler {
    Rc::new(move |ev| {
        let outcome = f(ev);
        schedule_flush();
        outcome
    })
}

// ===========================================================================
// Host + capability-trait delegation (generated from
// backend-macos/src/newcore.rs / runtime_vocabulary::bridge — keep
// mechanically in sync; the scene-parity goldens + the AllCaps bound on
// register_builtins are the compile gates)
// ===========================================================================

// ---------------------------------------------------------------------------
// Host — the P1 structural seam
// ---------------------------------------------------------------------------

impl Host for WgpuBackend {
    type Node = WgpuNode;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        <WgpuBackend as Backend>::insert(self, parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        <WgpuBackend as Backend>::insert_many(self, parent, children)
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        <WgpuBackend as Backend>::insert_at(self, parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        <WgpuBackend as Backend>::remove_child(self, parent, child)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::clear_children(self, node)
    }

    fn create_anchor(&mut self) -> Self::Node {
        <WgpuBackend as Backend>::create_reactive_anchor(self)
    }

    fn supports_splice(&self) -> bool {
        <WgpuBackend as Backend>::supports_child_splice(self)
    }
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

impl caps::AppEnvOps for WgpuBackend {
    fn color_scheme(&self) -> ColorScheme {
        <WgpuBackend as Backend>::color_scheme(self)
    }

    fn platform(&self) -> Platform {
        <WgpuBackend as Backend>::platform(self)
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        <WgpuBackend as Backend>::url_opener(self)
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        <WgpuBackend as Backend>::fullscreen_setter(self)
    }

    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        <WgpuBackend as Backend>::set_page_metadata(self, meta)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        <WgpuBackend as Backend>::set_app_background(self, color)
    }

    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
        <WgpuBackend as Backend>::set_scrollbar_theme(self, thumb, track)
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        // Dispatch-site glue: app-level key handlers run author code
        // (Host::key routes here before the focused-input path).
        let handler = handler.map(flushing_key);
        <WgpuBackend as Backend>::set_app_key_handler(self, handler)
    }
}

impl caps::LifecycleOps for WgpuBackend {
    fn finish(&mut self, root: Self::Node) {
        <WgpuBackend as Backend>::finish(self, root)
    }

    fn run_layout(&mut self) {
        <WgpuBackend as Backend>::run_layout(self)
    }

    fn schedule_layout_pass() {
        <WgpuBackend as Backend>::schedule_layout_pass()
    }

    fn is_hydrating(&self) -> bool {
        <WgpuBackend as Backend>::is_hydrating(self)
    }

    fn renders_lazy_chunks(&self) -> bool {
        <WgpuBackend as Backend>::renders_lazy_chunks(self)
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for WgpuBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        <WgpuBackend as Backend>::create_view(self, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        <WgpuBackend as Backend>::make_view_handle(self, node)
    }
}

impl caps::InputOps for WgpuBackend {
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
        <WgpuBackend as Backend>::install_touch_handler(self, node, handler)
    }

    fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
        <WgpuBackend as Backend>::claim_touch(self, node, touch_id)
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
        <WgpuBackend as Backend>::install_wheel_handler(self, node, handler)
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        <WgpuBackend as Backend>::install_hover_handler(self, node, flushing1(handler))
    }

    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::mark_preserves_focus(self, node)
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
        <WgpuBackend as Backend>::install_file_drop_handler(self, node, handler)
    }
}

impl caps::PressableOps for WgpuBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        <WgpuBackend as Backend>::create_pressable(self, flushing0(on_click), a11y)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
        <WgpuBackend as Backend>::make_pressable_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for WgpuBackend {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
        <WgpuBackend as Backend>::create_text(self, content, a11y)
    }

    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
        <WgpuBackend as Backend>::create_styled_text(self, runs, a11y)
    }

    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
        <WgpuBackend as Backend>::update_styled_text(self, node, runs)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        <WgpuBackend as Backend>::update_text(self, node, content)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
        <WgpuBackend as Backend>::create_text_with_id(self, content, a11y)
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        <WgpuBackend as Backend>::update_text_by_id(self, id, content)
    }

    fn release_text_id(&mut self, id: u32) {
        <WgpuBackend as Backend>::release_text_id(self, id)
    }

    fn supports_js_text_bindings(&self) -> bool {
        <WgpuBackend as Backend>::supports_js_text_bindings(self)
    }

    fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[Rc<dyn Fn() -> String>],
    ) {
        <WgpuBackend as Backend>::register_reactive_text_binding(
            self,
            text_id,
            signal_ids,
            template_parts,
            initial_values,
            stringifiers,
        )
    }

    fn release_reactive_text_binding(&mut self, text_id: u32) {
        <WgpuBackend as Backend>::release_reactive_text_binding(self, text_id)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        <WgpuBackend as Backend>::make_text_handle(self, node)
    }
}

impl caps::ButtonOps for WgpuBackend {
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
        <WgpuBackend as Backend>::create_button(self, label, &on_click, leading_icon, trailing_icon, a11y)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        <WgpuBackend as Backend>::update_button_label(self, node, label)
    }

    fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
        <WgpuBackend as Backend>::make_button_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for WgpuBackend {
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node {
        <WgpuBackend as Backend>::create_image(self, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        <WgpuBackend as Backend>::update_image_src(self, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        <WgpuBackend as Backend>::update_image_alt(self, node, alt)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        let handler: ImageLoadHandler = {
            let f = handler;
            Rc::new(move |ev| {
                f(ev);
                schedule_flush();
            })
        };
        <WgpuBackend as Backend>::install_image_load_handler(self, node, handler)
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        <WgpuBackend as Backend>::install_image_error_handler(self, node, flushing0(handler))
    }

    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        <WgpuBackend as Backend>::make_image_handle(self, node)
    }
}

impl caps::IconOps for WgpuBackend {
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WgpuBackend as Backend>::create_icon(self, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        <WgpuBackend as Backend>::update_icon_color(self, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
        <WgpuBackend as Backend>::update_icon_data(self, node, data)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        <WgpuBackend as Backend>::update_icon_stroke(self, node, progress)
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
        <WgpuBackend as Backend>::animate_icon_stroke(
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
        <WgpuBackend as Backend>::make_icon_handle(self, node)
    }
}

impl caps::LinkOps for WgpuBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: link activation dispatches navigation
        // (stages nav-queue tick signals on the new core).
        let mut config = config;
        config.on_activate = flushing0(config.on_activate.clone());
        <WgpuBackend as Backend>::create_link(self, config, a11y)
    }

    fn update_link_url(&mut self, node: &Self::Node, url: &str) {
        <WgpuBackend as Backend>::update_link_url(self, node, url)
    }

    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        <WgpuBackend as Backend>::make_link_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for WgpuBackend {
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
        <WgpuBackend as Backend>::create_text_input(
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
        <WgpuBackend as Backend>::update_text_input_value(self, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        <WgpuBackend as Backend>::update_text_input_secure(self, node, secure)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
        <WgpuBackend as Backend>::set_text_input_focus_handler(self, node, flushing1(handler))
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        <WgpuBackend as Backend>::update_text_input_placeholder(self, node, placeholder)
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
        <WgpuBackend as Backend>::create_text_area(
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
        <WgpuBackend as Backend>::update_text_area_value(self, node, value)
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        <WgpuBackend as Backend>::make_text_input_handle(self, node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        <WgpuBackend as Backend>::make_text_area_handle(self, node)
    }
}

impl caps::ToggleOps for WgpuBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WgpuBackend as Backend>::create_toggle(self, initial_value, flushing1(on_change), a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        <WgpuBackend as Backend>::update_toggle_value(self, node, value)
    }

    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        <WgpuBackend as Backend>::make_toggle_handle(self, node)
    }
}

impl caps::SliderOps for WgpuBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WgpuBackend as Backend>::create_slider(
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
        <WgpuBackend as Backend>::update_slider_value(self, node, value)
    }

    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        <WgpuBackend as Backend>::make_slider_handle(self, node)
    }
}

impl caps::ActivityIndicatorOps for WgpuBackend {
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WgpuBackend as Backend>::create_activity_indicator(self, size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        <WgpuBackend as Backend>::update_activity_indicator_size(self, node, size)
    }

    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        <WgpuBackend as Backend>::make_activity_indicator_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for WgpuBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: on_scroll fires per scroll event AND per
        // momentum tick inside Host::tick; the flush microtask is
        // deduped so a burst costs one commit.
        let on_scroll = on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
            Rc::new(move |x, y| {
                f(x, y);
                schedule_flush();
            })
        });
        <WgpuBackend as Backend>::create_scroll_view(self, horizontal, on_scroll, a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        <WgpuBackend as Backend>::node_scroll(self, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        <WgpuBackend as Backend>::set_node_scroll(self, node, x, y)
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        <WgpuBackend as Backend>::make_scroll_view_handle(self, node)
    }
}

impl caps::SafeAreaOps for WgpuBackend {
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <WgpuBackend as Backend>::apply_safe_area_padding(self, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <WgpuBackend as Backend>::apply_scroll_view_safe_area_inset(self, node, sides)
    }
}

impl caps::VirtualizerOps for WgpuBackend {
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
        <WgpuBackend as Backend>::create_virtualizer(self, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::virtualizer_data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::release_virtualizer(self, node)
    }

    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle {
        <WgpuBackend as Backend>::make_virtualizer_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for WgpuBackend {
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
        <WgpuBackend as Backend>::create_graphics(self, on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::release_graphics(self, node)
    }

    fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
        <WgpuBackend as Backend>::make_graphics_handle(self, node)
    }
}

impl caps::PortalOps for WgpuBackend {
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
        <WgpuBackend as Backend>::create_portal(self, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::release_portal(self, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        <WgpuBackend as Backend>::set_portal_hidden(self, node, hidden)
    }

    fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
        <WgpuBackend as Backend>::make_portal_handle(self, node)
    }
}

impl caps::PresenceOps for WgpuBackend {
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        <WgpuBackend as Backend>::create_presence_placeholder(self, a11y)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        <WgpuBackend as Backend>::apply_presence(self, node, state, transition)
    }

    fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
        <WgpuBackend as Backend>::make_presence_handle(self, node)
    }
}

impl caps::NavigatorOps for WgpuBackend {
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
        // screens and dispatch is handler-safe: commands stage into a
        // queue + tick signal committed by a driver effect on flush).
        // Author-initiated navigation stages via handlers already
        // wrapped above and commits inside the flush.
        <WgpuBackend as Backend>::create_navigator(self, type_id, type_name, presentation, host, a11y)
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::release_navigator(self, node)
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        <WgpuBackend as Backend>::apply_navigator_slot_style(self, node, slot, style)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
        <WgpuBackend as Backend>::make_navigator_handle(self, node)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        <WgpuBackend as Backend>::navigator_attach_initial(self, navigator, screen, scope_id, options)
    }
}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for WgpuBackend {
    fn create_external(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <WgpuBackend as Backend>::create_external(self, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::release_external(self, node)
    }

    fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
        <WgpuBackend as Backend>::missing_primitive_placeholder(self, label)
    }
}

impl caps::DocumentOps for WgpuBackend {
    fn create_element(&mut self, tag: &str) -> Self::Node {
        <WgpuBackend as Backend>::create_element(self, tag)
    }

    fn attach_html_id(&self, node: &Self::Node, id: &str) {
        <WgpuBackend as Backend>::attach_html_id(self, node, id)
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        <WgpuBackend as Backend>::attach_html_class(self, node, class)
    }

    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
        <WgpuBackend as Backend>::attach_html_style(self, node, prop, value)
    }

    fn register_raw_css(&mut self, css: &str) {
        <WgpuBackend as Backend>::register_raw_css(self, css)
    }
}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for WgpuBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        <WgpuBackend as Backend>::apply_style(self, node, style)
    }

    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        <WgpuBackend as Backend>::mint_style_class(self, style)
    }

    fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
        <WgpuBackend as Backend>::mint_class_for_app(self, app)
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        <WgpuBackend as Backend>::apply_styled_states(self, node, base, overlays)
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        <WgpuBackend as Backend>::apply_styled_variants(
            self,
            node,
            base,
            state_overlays,
            breakpoint_overlays,
            container_overlays,
        )
    }

    fn mark_container(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::mark_container(self, node)
    }

    fn handles_states_natively(&self) -> bool {
        <WgpuBackend as Backend>::handles_states_natively(self)
    }

    fn token_updates_propagate_via_cascade(&self) -> bool {
        <WgpuBackend as Backend>::token_updates_propagate_via_cascade(self)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <WgpuBackend as Backend>::register_stylesheet(self, rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <WgpuBackend as Backend>::unregister_stylesheet(self, rules)
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        <WgpuBackend as Backend>::install_tokens(self, tokens)
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        <WgpuBackend as Backend>::update_tokens(self, tokens)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        <WgpuBackend as Backend>::on_node_unstyled(self, node)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        // Dispatch-site glue: hover/press/focus state flips can stage
        // writes when the style path routes states through signals.
        // The interaction Host fires the setter from its press/hover
        // tracking.
        let setter: Rc<dyn Fn(StateBits, bool)> = {
            let f = setter;
            Rc::new(move |bits, on| {
                f(bits, on);
                schedule_flush();
            })
        };
        <WgpuBackend as Backend>::attach_states(self, node, setter)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        <WgpuBackend as Backend>::set_disabled(self, node, disabled)
    }

    fn supports_preminted_styles(&self) -> bool {
        <WgpuBackend as Backend>::supports_preminted_styles(self)
    }

    fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {
        <WgpuBackend as Backend>::apply_default_text_font(self, font)
    }

    fn supports_js_class_bindings(&self) -> bool {
        <WgpuBackend as Backend>::supports_js_class_bindings(self)
    }

    fn register_reactive_class_binding(
        &mut self,
        node: &Self::Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: Rc<dyn Fn() -> u32>,
    ) -> u32 {
        <WgpuBackend as Backend>::register_reactive_class_binding(
            self,
            node,
            signal_id,
            values,
            classes,
            value_reader,
        )
    }

    fn release_reactive_class_binding(&mut self, binding_id: u32) {
        <WgpuBackend as Backend>::release_reactive_class_binding(self, binding_id)
    }
}

impl caps::AssetOps for WgpuBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        <WgpuBackend as Backend>::register_asset(self, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        <WgpuBackend as Backend>::unregister_asset(self, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        <WgpuBackend as Backend>::register_typeface(self, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        <WgpuBackend as Backend>::unregister_typeface(self, id)
    }
}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for WgpuBackend {
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        <WgpuBackend as Backend>::update_accessibility(self, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        <WgpuBackend as Backend>::announce_for_accessibility(self, msg, priority)
    }

    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
        <WgpuBackend as Backend>::dump_accessibility_tree(self)
    }
}

impl caps::AnimationOps for WgpuBackend {
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        <WgpuBackend as Backend>::set_animated_f32(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        <WgpuBackend as Backend>::set_animated_color(self, node, prop, value)
    }
}

impl caps::IntrospectionOps for WgpuBackend {
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <WgpuBackend as Backend>::frame(self, node)
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <WgpuBackend as Backend>::absolute_frame(self, node)
    }

    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <WgpuBackend as Backend>::device_frame(self, node)
    }

    fn supports_native_introspection(&self) -> bool {
        <WgpuBackend as Backend>::supports_native_introspection(self)
    }

    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        <WgpuBackend as Backend>::introspect_native(self, node)
    }

    fn note_introspection_root(&self, node: &Self::Node) {
        <WgpuBackend as Backend>::note_introspection_root(self, node)
    }

    fn supports_screenshot(&self) -> bool {
        <WgpuBackend as Backend>::supports_screenshot(self)
    }

    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        <WgpuBackend as Backend>::capture_screenshot(self, done)
    }
}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for WgpuBackend {
    fn supports_batched_repeat(&self) -> bool {
        <WgpuBackend as Backend>::supports_batched_repeat(self)
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        <WgpuBackend as Backend>::execute_batch(self, batch)
    }

    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        <WgpuBackend as Backend>::execute_batch_with_attach(self, batch, parent, attach_locals)
    }
}

impl caps::WireBindingOps for WgpuBackend {
    fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
        <WgpuBackend as Backend>::note_text_binding(self, node, signal_ids, method)
    }

    fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value) {
        <WgpuBackend as Backend>::note_signal_initial(self, signal_id, value)
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        <WgpuBackend as Backend>::note_when_binding(
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
        <WgpuBackend as Backend>::note_switch_binding(
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
        <WgpuBackend as Backend>::note_repeat_binding(
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
        <WgpuBackend as Backend>::note_virtualizer_binding(
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
        <WgpuBackend as Backend>::supports_lazy_slot_capture(self)
    }

    fn begin_slot_capture(&mut self) {
        <WgpuBackend as Backend>::begin_slot_capture(self)
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        <WgpuBackend as Backend>::end_slot_capture(self, slot_root)
    }
}

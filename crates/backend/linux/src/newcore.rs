//! Rendering: the `runtime_scene::Host` + capability-trait surface, the
//! boot entry, and the flush driver.
//!
//! [`LinuxBackend`] implements [`runtime_scene::Host`] plus **all 30**
//! capability traits (`runtime_vocabulary::caps`) — the production shape
//! of the migration. Every mechanism body in this file was moved here
//! verbatim from the crate's old `impl runtime_core::Backend for LinuxBackend`
//! when the 159-method mega-trait was deleted, so the GTK widget mechanism code
//! is unchanged: the same scene builds the same widget tree.
//! Capabilities this backend does not implement are simply absent — the
//! caps-trait DEFAULT bodies serve them, and those defaults were audited
//! byte-for-byte against the `Backend` defaults they replace
//! (`docs/runtime-v2-deletion-baseline.md` S2.1; 128 of this backend's
//! 152 caps methods resolve to a default).
//!
//! **30/30 traits implemented, 0 adapted, 0 stubbed.**
//!
//! # Two layers in one file: mechanism + flush policy
//!
//! Capability methods that take an author callback wrap it before running
//! the mechanism (`flushing0`/`flushing1`/`flushing_key` + the inline
//! wrappers below) so a staged write commits after the callback returns.
//! That dispatch-site policy is why the mechanism lives here rather than
//! in an inherent impl: the wrap and the body are one method.
//!
//! # Boot sequence ([`start`])
//!
//! The host shell — `host-gtk` in this repo — realizes + presents its
//! `gtk::Window`, wraps it in [`LinuxBackend::new`], and installs its
//! `runtime_shared::scheduling::Scheduler` BEFORE calling [`start`]:
//!
//! 1. Monotonic time source (idempotent, first install wins).
//! 2. Registry: [`runtime_vocabulary::register_builtins`] + the app's
//!    `register` seam.
//! 3. Fresh [`World`]; build + [`realize`] inside `world.enter`.
//! 4. Entered buffered-microtask drain (no-op under a real scheduler;
//!    load-bearing under a buffering test scheduler).
//! 5. Single root → `caps::LifecycleOps::finish`; `world.flush()` commits
//!    anything staged during mount before the first paint.
//! 6. Install the flush driver and retain
//!    `{Realized, backend, registry, world}` in [`NewCoreApp`].
//!
//! # Flush driver — dispatch-model analysis
//!
//! The new core stages signal writes; nothing observes them until the
//! driver calls [`World::flush`]. GTK dispatches author callbacks
//! exclusively through signal closures the mechanism bodies connect, and
//! every one captures the closure handed in at creation. Under new-core
//! the closures handed in ARE the caps-layer wrapped ones (wrapped
//! BEFORE the UFCS delegation), so every GTK signal dispatch schedules a
//! flush with zero host changes.
//!
//! **Every author callback this backend accepts must be wrapped.** The
//! full list, because an unwrapped one is invisible — it compiles, the
//! widget still reacts to input, the reactive graph really does update,
//! and only the commit is missing, so the sole symptom is a UI that
//! quietly ignores the user:
//!
//! - `connect_clicked` (Button `Action::fire`), `GestureClick::
//!   connect_released` (Pressable **and** Link) — `flushing0`.
//! - `connect_state_notify` (Toggle's `gtk::Switch`),
//!   `connect_value_changed` (Slider's `gtk::Scale`), text input /
//!   text area `on_change` — `flushing1`; `on_key_down` —
//!   `flushing_key`.
//! - ScrolledWindow h/v `Adjustment` `value-changed` (`on_scroll`) —
//!   hand-written (two args). This is the scroll-spy path: a table of
//!   contents writes its active-section signal from here.
//! - `install_touch_handler` / `install_file_drop_handler` —
//!   hand-written, because they RETURN a `TouchResponse` the backend
//!   acts on, so the wrapper must forward the value rather than swallow
//!   it.
//! - `attach_states` (hover / press / focus → style variants) and
//!   `install_hover_handler` — hand-written. See `states.rs`.
//!
//! Historical note, since it cost real debugging time: `create_link` was
//! the one cap that delegated its callback RAW. The framework wraps
//! navigator push/replace dispatch inside `on_activate`, so every in-app
//! link fired, changed the route signal, and never re-rendered — the
//! whole app read as "buttons don't do anything".
//!
//! **Scheduler contract.** `host-gtk` installs a
//! `runtime_shared::scheduling::Scheduler` (and a `raf_loop` driver) on
//! the GLib main loop. Two regimes:
//!
//! - *No scheduler installed* (a bare embedder, and most tests):
//!   `schedule_microtask` falls back to SYNCHRONOUS execution off-Web
//!   (the `runtime-shared::scheduling` contract), so the deduped flush
//!   runs inline before the wrapped author callback returns. That is
//!   borrow-safe here: GTK signal closures run from the GTK main loop
//!   with NO backend borrow held (they capture only the handler `Rc`,
//!   never the backend), so the flush may re-enter the backend
//!   mutably through its `Rc<RefCell<…>>`.
//! - *Scheduler installed* (a host bridging `glib::timeout_add` /
//!   `idle_add` / the frame clock): the host MUST fire
//!   [`crate::dispatch_hook::fire_dispatch_hook`] after every
//!   scheduler-driven author callback (`after_ms` timers,
//!   `after_animation_frame` one-shots, `raf_loop` iterations) and
//!   drain microtasks once per main-loop iteration so the flush
//!   commits before the next paint — `host_terminal`'s scheduler tick
//!   is the model. Microtasks themselves never fire the hook (the
//!   flush rides one; see [`crate::dispatch_hook`]).
//!
//! # Viewport
//!
//! `run_layout` publishes the window size to BOTH sinks whenever it
//! changes: `runtime_shared::set_viewport_size` (the legacy TLS) and
//! `forward_viewport`, which feeds the per-world
//! `runtime_vocabulary::viewport` ctx that v2 breakpoint resolution
//! actually reads. The two must never diverge — the terminal backend's
//! `set_viewport` + `forward_viewport` pairing is the model.
//! `host-gtk` additionally seeds the viewport from its `RunOptions`
//! before the first realize, because a component body may resolve a
//! `__bp_*` overlay while constructing, and the window is not allocated
//! yet at that point.
//!
//! # Residual seams (named, none silent)
//!
//! - Third-party payloads mounted through the scene `Registry`: author
//!   callbacks such a leaf wires to its own GTK signals must call
//!   [`schedule_flush`] themselves. Nothing in the caps layer can wrap
//!   them, because the caps layer never sees them.
//! - `trap_focus` on a portal is best-effort (focusable + grab on map).
//!   Real modal focus confinement needs a `gtk::Window`-hosted popup.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use runtime_shared::animation::AnimProp;
use runtime_shared::assets::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives;
#[cfg(feature = "robot")]
use runtime_shared::introspect::NativeNode;
#[cfg(feature = "robot")]
use runtime_shared::primitives::portal::ViewportRect;
#[cfg(feature = "robot")]
use runtime_shared::Screenshot;
use runtime_shared::{
    Action, Color, ColorScheme, Platform, StyleRules,
    VirtualizerCallbacks,
};
use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;
// The GTK widget mechanism (moved here from the crate's old `impl
// Backend`) leans on gtk4's trait-based API surface.
use gtk4::prelude::*;

use crate::{LinuxBackend, LinuxNode};

// Re-exported so a host shell and app wrappers can name the boot-path
// types without a direct runtime-scene dependency — mirrors
// `backend_terminal::newcore`.
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
    /// ([`with_backend`]).
    static BACKEND: RefCell<Option<Weak<RefCell<LinuxBackend>>>> = const { RefCell::new(None) };
}

/// Everything the boot path must keep alive. Field order is drop order:
/// the realized tree unmounts before the world (its slots' owner) dies.
/// The host shell holds this value for the whole session and calls
/// [`NewCoreApp::stop`] on teardown.
pub struct NewCoreApp {
    realized: Realized<LinuxNode>,
    _backend: Rc<RefCell<LinuxBackend>>,
    _registry: Rc<Registry<LinuxBackend>>,
    world: World,
}

impl NewCoreApp {
    /// Borrow the live tree (tests, diagnostics).
    pub fn with_realized<R>(&self, f: impl FnOnce(&Realized<LinuxNode>) -> R) -> R {
        f(&self.realized)
    }

    /// The mounted world (tests can flush it explicitly).
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Unmount: drops the `Realized` (cleanups fire), uninstalls the
    /// flush driver, and drops the world. Must run on the GTK main
    /// thread while its TLS is intact — the reactive teardown runs
    /// scope cleanups (same rationale as the old host's explicit
    /// `Owner` drop on the terminal).
    pub fn stop(self) {
        crate::dispatch_hook::clear_dispatch_hook();
        set_flush_world(None);
        set_viewport_sink(None);
        BACKEND.with(|b| *b.borrow_mut() = None);
        drop(self);
    }
}

/// Mount a new-core element tree into an already-constructed backend.
///
/// The host must have constructed the backend around its realized
/// window ([`LinuxBackend::new`]) and, if it uses one, installed its
/// scheduler first — see the module docs' scheduler contract.
///
/// `register` runs after [`runtime_vocabulary::register_builtins`], so
/// apps/SDKs can register their own payload handlers on the same
/// registry before the tree realizes. The build closure runs inside
/// `world.enter`, so free `signal()`/`effect()` calls work; top-level
/// creations are world-root-owned (they live until [`NewCoreApp::stop`]).
#[inline]
pub fn start(
    backend: Rc<RefCell<LinuxBackend>>,
    register: impl FnOnce(&mut Registry<LinuxBackend>),
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
    backend: Rc<RefCell<LinuxBackend>>,
    register: R,
    build: B,
) -> NewCoreApp where
    S: runtime_vocabulary::BuiltinSet,
    R: FnOnce(&mut Registry<LinuxBackend>),
    B: FnOnce() -> Element,
{
    // Monotonic clock (idempotent, first install wins) — animation and
    // presence timing read it; the old boot relied on the host's lazy
    // default, the new boot installs it explicitly like macOS/wgpu.
    let platform = caps::AppEnvOps::platform(&*backend.borrow());
    runtime_shared::time::install_default_time_source(platform);

    // Ambient environment services (platform identity, color scheme, URL
    // opener, full-screen setter, AX announcer) -> the thread-locals
    // `platform()` / `open_url()` / `announce()` etc. read. MUST precede
    // the build: a component body may read `platform()` while
    // constructing. See `runtime_vocabulary::backend`.
    runtime_vocabulary::backend::install_env_services(&backend);
    let mut registry: Registry<LinuxBackend> = Registry::new();
    runtime_vocabulary::register_builtins_with::<_, S>(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    let world = World::new();
    let (vp_sig, realized) = world.enter(|| {
        let element = build();
        let realized = realize(&backend, &registry, element);
        // Captured POST-BUILD on purpose: the ctx's derived-bucket memo
        // snapshots the breakpoint threshold table at creation, and an app
        // may install a custom table inside its root component (the docs
        // app does). Creating the ctx eagerly would pin the DEFAULT table
        // and misclassify every width the two disagree on — same ordering
        // backend-web documents.
        let vp_sig = runtime_vocabulary::viewport::viewport_ctx().size_signal();
        (vp_sig, realized)
    });

    // Buffered-microtask drain — a no-op under a real host scheduler
    // (and under the no-scheduler synchronous fallback), load-bearing
    // under a buffering test scheduler. Must run with NO backend
    // borrow held (drained tasks re-borrow); ENTERED because a
    // buffered task may do creation-side work.
    world.enter(runtime_shared::scheduling::drain_buffered_microtasks);

    // Single-root contract, matching the old-core mount (`find_root`
    // wants exactly one application root — id 1).
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_linux::newcore::start: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    caps::LifecycleOps::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount before the first paint.
    world.flush();

    // Install the flush driver: schedule_flush becomes reachable from
    // (a) the author-callback wrappers in the caps impls below and
    // (b) the host shell's post-dispatch hook fires.
    crate::dispatch_hook::install_dispatch_hook(schedule_flush);
    set_flush_world(Some(world.clone()));
    // Live viewport source: `LinuxBackend::run_layout` now reaches the
    // world's ctx through `forward_viewport`, so breakpoint-dependent
    // author reactivity re-fires on resize instead of freezing at its seed.
    set_viewport_sink(Some(vp_sig));
    BACKEND.with(|b| *b.borrow_mut() = Some(Rc::downgrade(&backend)));
    // Point the Robot bridge at the registry this boot actually fills.
    #[cfg(feature = "robot")]
    install_robot_env();
    NewCoreApp {
        realized,
        _backend: backend,
        _registry: registry,
        world,
    }
}

/// Point the Robot bridge at the vocabulary registry and give it a way to enter
/// the mounted world.
///
/// # Why this is needed, and what its absence looks like
///
/// There are TWO robot registries: the shared substrate's (legacy) and
/// `runtime_vocabulary`'s, which is the one the v2 walker registers every
/// mounted primitive into. `runtime_shared`'s bridge dispatches to its own by
/// default, so a native app that merely starts the bridge answers every verb
/// from an EMPTY registry: `find_element` → `null`, `get_snapshot` → `[]`,
/// `list_navigators` → `[]`, on a perfectly healthy app. Well-formed answers,
/// all of them wrong — a silently blinded driver, which is worse than an error.
/// Measured exactly that way on the GTK docs app before this existed.
///
/// `install_verb_router` redirects to the vocabulary dispatch, falling back to
/// the shared one on the precise `unknown command:` marker so verbs the
/// vocabulary does not own (`get_logs`, host-registered commands like
/// `screenshot`) still resolve, and a REAL verb error is never masked.
///
/// `install_driver_env` supplies the two things vocabulary verbs need from the
/// host: queries run INSIDE the mounted world (a reactive `label_fn` reads world
/// signals, and reading outside a world panics), and actions settle through a
/// flush so an author callback's staged writes are committed before the verb
/// returns — otherwise `click` reports success and the caller reads the state
/// from before the click.
///
/// The sidecar does this for runtime-server sessions and `backend-web` does it
/// for the browser; local-mount native had no equivalent, which is why
/// `idealyst dev --linux` produced an app the Robot tools could reach and not
/// see.
#[cfg(feature = "robot")]
fn install_robot_env() {
    runtime_vocabulary::robot::install_driver_env(
        |f| match mounted_world() {
            Some(world) => world.enter(|| f()),
            // Pre-boot / post-stop there is no world; run plainly so a query
            // still resolves static labels instead of panicking.
            None => f(),
        },
        || {
            if let Some(world) = mounted_world() {
                if !world.is_flushing() {
                    world.flush();
                }
            }
        },
    );
    runtime_shared::robot::bridge::install_verb_router(|cmd, args| {
        match runtime_vocabulary::robot::bridge::invoke_command(cmd, args) {
            Err(e) if e.starts_with("unknown command:") => None,
            other => Some(other),
        }
    });
}

fn set_flush_world(world: Option<World>) {
    FLUSH_WORLD.with(|w| *w.borrow_mut() = world);
}

thread_local! {
    /// The mounted world's viewport signal (`Copy` handle). `None`
    /// outside a boot, so `forward_viewport` costs one TLS read and
    /// nothing else when no app is mounted.
    static VIEWPORT_SINK: std::cell::Cell<
        Option<runtime_world::Signal<runtime_shared::ViewportSize>>,
    > = const { std::cell::Cell::new(None) };
}

fn set_viewport_sink(sig: Option<runtime_world::Signal<runtime_shared::ViewportSize>>) {
    VIEWPORT_SINK.with(|s| s.set(sig));
}

/// Forward one viewport report into the mounted world's viewport ctx.
/// No-op before [`start`] / after teardown.
///
/// `runtime_shared::set_viewport_size` alone is NOT enough: that writes the
/// legacy thread-local signal, which a `runtime_world` effect cannot
/// subscribe to. Author reactivity (`current_breakpoint()`, and everything
/// idea-ui-nav's `AppShell` derives from it) reads the PER-WORLD ctx, so a
/// backend that only writes the shared TLS value leaves every breakpoint
/// reader frozen at the ctx's seed — the GTK docs app's sidebar never
/// re-pinned on resize.
///
/// Capture, don't inject: the caller runs outside `World::enter` (a GLib
/// idle), so the handle is captured at boot and the write stages through
/// it (equality-guarded, routed to its own world) then rides one deduped
/// [`schedule_flush`] — the backend-web resize-listener discipline.
pub(crate) fn forward_viewport(size: runtime_shared::ViewportSize) {
    let Some(sig) = VIEWPORT_SINK.with(|s| s.get()) else {
        return;
    };
    sig.set(size);
    schedule_flush();
}

/// True while a new-core app is mounted (`start` ran, `stop` hasn't).
/// Host-integration seam: an embedded renderer mounted INSIDE this app's
/// tree — the wgpu GPU preview (`host_linux_desktop::mount`) — realizes its
/// scene into this SAME world, so the app's existing flush driver commits
/// the embedded app's staged writes with no second driver: one thread, one
/// world, one logical update stream. Mirrors
/// `backend_macos::newcore::mounted_world`.
pub fn mounted_world() -> Option<World> {
    FLUSH_WORLD.with(|w| w.borrow().clone())
}

pub fn is_booted() -> bool {
    FLUSH_WORLD.with(|w| w.borrow().is_some())
}

/// Run `f` with the mounted backend (diagnostics). `None` before
/// [`start`] or after the backend dropped.
pub fn with_backend<R>(f: impl FnOnce(&Rc<RefCell<LinuxBackend>>) -> R) -> Option<R> {
    let rc = BACKEND.with(|b| b.borrow().as_ref().and_then(Weak::upgrade));
    rc.map(|rc| f(&rc))
}

// ===========================================================================
// Flush driver
// ===========================================================================

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// With no scheduler installed this runs synchronously (see the module
/// docs' scheduler contract); under a host scheduler it defers to the
/// next microtask drain.
pub fn schedule_flush() {
    if FLUSH_QUEUED.with(|q| q.replace(true)) {
        return;
    }
    runtime_shared::scheduling::schedule_microtask(|| {
        FLUSH_QUEUED.with(|q| q.set(false));
        flush_now();
    });
}

/// Synchronously commit staged writes (skipped mid-flush; no-op before
/// [`start`]). Harness seam: a driver that staged writes and must
/// observe the committed tree before returning cannot ride the async
/// microtask — it flushes before returning instead.
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
/// ambient (`World::enter`). Same rationale as the wgpu/web glue:
/// virtualizer `mount_item`/`release_item` REALIZE/tear down a row —
/// creation-side work that needs the ambient world. Pre-boot the boot's
/// own `enter` is still ambient, so the bare-call fallback never
/// double-books.
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
// returns, one deduped flush microtask is queued. Wrapping here (instead
// of inside the shared Backend code) keeps the old-core path
// byte-identical: the old core applies writes synchronously and must not
// pay a flush per event.

/// Wrap a zero-arg author callback (`on_press`, link `on_activate`, …).
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
// backend-terminal/src/newcore.rs / runtime_vocabulary::bridge — keep
// mechanically in sync; the AllCaps bound on register_builtins is the
// compile gate)
// ===========================================================================

// ---------------------------------------------------------------------------
// Host — the P1 structural seam
// ---------------------------------------------------------------------------

impl Host for LinuxBackend {
    type Node = LinuxNode;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        // Our node model tracks children/parent_of for z-order and sticky;
        // master's body only knew layout_for_id, so delegate to the real one.
        LinuxBackend::insert(self, parent, child)
    }

    // `insert_many` is deliberately NOT implemented: `Host`'s default is
    // the same N-x-`insert` loop the old `Backend` default ran, so the
    // resulting child order is unchanged (deletion-baseline S2.2 —
    // "byte-identical on `Host`, safe").

    /// Explicit port of the old `Backend::insert_at` DEFAULT body: append,
    /// ignoring the index. `Host` makes the method REQUIRED, so the default
    /// that used to supply this body is gone — reproduced verbatim rather
    /// than inherited (deletion-baseline S2.2). Never reached in practice:
    /// [`supports_splice`](Self::supports_splice) is `false`, so reactive
    /// regions rebuild wholesale under their own anchor and no positional
    /// splice is ever emitted.
    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, _index: usize) {
        self.insert(parent, child)
    }

    /// Explicit port of the old `Backend::remove_child` DEFAULT body (a
    /// no-op). `Host` makes it REQUIRED, so it is stated here rather than
    /// inherited (deletion-baseline S2.2). Only meaningful for
    /// splice-capable hosts; this one is anchored, so the framework never
    /// calls it.
    fn remove_child(&mut self, _parent: &Self::Node, _child: &Self::Node) {
        // default: no-op
    }

    fn clear_children(&mut self, node: &Self::Node) {
        // Must also drop our `children` / `parent_of` / sticky entries, not
        // just detach GTK widgets — the inherent body does both.
        LinuxBackend::clear_children(self, node)
    }

    /// Explicit port of the old `Backend::create_reactive_anchor` DEFAULT
    /// body (`create_view` with default a11y). `Host` makes it REQUIRED, so
    /// it is stated here rather than inherited (deletion-baseline S2.2). A
    /// plain container view is the right anchor for this backend: an
    /// unstyled view draws nothing, so the anchor is invisible and
    /// layout-neutral.
    fn create_anchor(&mut self) -> Self::Node {
        self.create_view(&AccessibilityProps::default())
    }

    /// Explicit `false` — the port of the old
    /// `Backend::supports_child_splice` DEFAULT this backend relied on.
    /// `Host` makes it REQUIRED, so the value is stated here instead of
    /// inherited (deletion-baseline S2.2). ANCHORED mode is what the frozen
    /// artifacts in `tests/goldens/` recorded from the old core: flipping it
    /// to `true` would move every reactive region out from under its anchor
    /// and change the output wholesale. Pinned by a literal assertion in the
    /// crate's parity suite.
    fn supports_splice(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

impl caps::AppEnvOps for LinuxBackend {
    fn color_scheme(&self) -> ColorScheme {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        LinuxBackend::color_scheme(self)
    }

    fn platform(&self) -> Platform {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        LinuxBackend::platform(self)
    }

    fn set_app_background(&mut self, color: &runtime_shared::Tokenized<runtime_shared::Color>) {
        LinuxBackend::set_app_background_impl(self, color)
    }
}

impl caps::LifecycleOps for LinuxBackend {
    /// Synchronous layout pass (runtime-server shells). The trait method
    /// takes no size; our inherent pass needs one, so read the host
    /// window's current allocation — the same source `finish` uses. A
    /// degenerate (pre-realize) size is skipped there, so this is safe to
    /// call before the window is mapped.
    fn run_layout(&mut self) {
        let (w, h) = {
            let win = self.host_window();
            (win.width() as f32, win.height() as f32)
        };
        LinuxBackend::run_layout(self, w, h)
    }

    fn finish(&mut self, root: Self::Node) {
        // The inherent pass runs Taffy over our node map, then applies
        // sticky + transforms and publishes the viewport size.
        LinuxBackend::finish(self, root)
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for LinuxBackend {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        // Delegates to the inherent GTK body (formerly `impl Backend`),
        // which builds the richer widget this backend actually ships.
        LinuxBackend::create_view(self, _a11y)
    }

    /// Real node handle, NOT the trait's defaulted no-op. `AnimatedValue::
    /// bind` drives every per-frame write through this handle, so leaving
    /// the default in place means the animation clock ticks and nothing
    /// ever paints — the value lands in a handle wired to nowhere.
    fn make_view_handle(&self, node: &Self::Node) -> runtime_shared::ViewHandle {
        LinuxBackend::make_view_handle(self, node)
    }
}

impl caps::InputOps for LinuxBackend {
    fn install_touch_handler(&mut self, node: &Self::Node, handler: runtime_shared::TouchHandler) {
        // Dispatch-site glue, same as every other author callback here.
        // These two can't use `flushing0`/`flushing1`: they RETURN a
        // `TouchResponse` the backend acts on, so the wrapper has to
        // forward the value rather than swallow it. Mirrors
        // `MacosBackend::install_touch_handler`.
        let handler: runtime_shared::TouchHandler = {
            let f = handler;
            Rc::new(move |ev| {
                let response = f(ev);
                schedule_flush();
                response
            })
        };
        LinuxBackend::install_touch_handler(self, node, handler)
    }

    fn install_file_drop_handler(
        &mut self,
        node: &Self::Node,
        handler: runtime_shared::FileDropHandler,
    ) {
        let handler: runtime_shared::FileDropHandler = {
            let f = handler;
            Rc::new(move |ev| {
                let response = f(ev);
                schedule_flush();
                response
            })
        };
        LinuxBackend::install_file_drop_handler(self, node, handler)
    }

    /// Author-facing `.on_hover(…)`. The trait default is a NO-OP, so
    /// leaving it unimplemented is invisible: the node renders fine and
    /// the callback simply never fires. See `states.rs`.
    fn install_hover_handler(&mut self, node: &Self::Node, handler: runtime_shared::HoverHandler) {
        let handler: runtime_shared::HoverHandler = {
            let f = handler;
            Rc::new(move |entered| {
                f(entered);
                schedule_flush();
            })
        };
        crate::states::install_hover(&node.widget(), handler)
    }
}

impl caps::PressableOps for LinuxBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> Self::Node {
        // Wrap the author callback so ONE deduped flush microtask is queued
        // after it returns. Without this the handler mutates its signal and
        // nothing re-renders — the v2 flush is not implicit.
        LinuxBackend::create_pressable(self, flushing0(on_click), _a11y)
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for LinuxBackend {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        // Delegates to the inherent GTK body (formerly `impl Backend`),
        // which builds the richer widget this backend actually ships.
        LinuxBackend::create_text(self, content, _a11y)
    }

    /// Real text handle — same rationale as `make_view_handle`.
    fn make_text_handle(&self, node: &Self::Node) -> runtime_shared::TextHandle {
        LinuxBackend::make_text_handle(self, node)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        LinuxBackend::update_text(self, node, content)
    }
}

impl caps::ButtonOps for LinuxBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&primitives::icon::IconData>,
        _trailing_icon: Option<&primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body (formerly `impl Backend`),
        // which builds the richer widget this backend actually ships.
        // Wrap the author callback so ONE deduped flush microtask is queued
        // after it returns. Without this the handler mutates its signal and
        // nothing re-renders — the v2 flush is not implicit.
        let on_click = Action { fire: flushing0(on_click.fire.clone()), ..on_click.clone() };
        LinuxBackend::create_button(self, label, &on_click, _leading_icon, _trailing_icon, _a11y)
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        LinuxBackend::update_button_label(self, node, label)
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for LinuxBackend {
    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        // Real GTK body; the caps default here is a silent no-op.
        LinuxBackend::update_image_src(self, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        // Real GTK body; the caps default here is a silent no-op.
        LinuxBackend::update_image_alt(self, node, alt)
    }
    fn create_image(
        &mut self,
        _src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        LinuxBackend::create_image(self, _src, _alt, _a11y)
    }

    // Both of these are DEFAULTED no-ops on the trait, and inheriting them
    // meant an author's `Image(on_load = …, on_error = …)` never fired on
    // Linux while it did on web and Apple — so an app's "couldn't load"
    // fallback silently never rendered. See `crate::image`.
    fn install_image_load_handler(
        &mut self,
        node: &Self::Node,
        handler: runtime_shared::primitives::image::ImageLoadHandler,
    ) {
        // Dispatch-site glue: the completion runs author code, which must
        // be followed by one deduped flush.
        let handler: runtime_shared::primitives::image::ImageLoadHandler = {
            let f = handler;
            Rc::new(move |ev| {
                f(ev);
                schedule_flush();
            })
        };
        LinuxBackend::install_image_load_handler(self, node, handler)
    }

    fn install_image_error_handler(
        &mut self,
        node: &Self::Node,
        handler: runtime_shared::primitives::image::ImageErrorHandler,
    ) {
        LinuxBackend::install_image_error_handler(self, node, flushing0(handler))
    }
}

impl caps::IconOps for LinuxBackend {
    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        // Real GTK body; the caps default here is a silent no-op.
        LinuxBackend::update_icon_color(self, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
        // Real GTK body; the caps default here is a silent no-op.
        LinuxBackend::update_icon_data(self, node, data)
    }
    fn create_icon(
        &mut self,
        _data: &runtime_shared::primitives::icon::IconData,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        LinuxBackend::create_icon(self, _data, _color, _a11y)
    }
}

impl caps::LinkOps for LinuxBackend {
    fn create_link(
        &mut self,
        config: runtime_shared::primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Wrap `on_activate` so ONE deduped flush microtask is queued after
        // it returns — the same treatment every other interactive cap here
        // gives its callback (pressable, button, text_input, toggle, slider,
        // portal on_dismiss).
        //
        // This was the ONLY interactive cap that delegated RAW, and it is
        // the whole "links do nothing" bug: for an in-app link the framework
        // wraps navigator push/replace dispatch in `on_activate`, so clicking
        // one STAGED the route write and nothing ever committed it. The click
        // landed, the gesture fired, the author callback ran, the route
        // signal changed — and the screen never re-rendered. Nothing warns
        // about this: a raw delegation compiles and the widget still reacts
        // to input, so the only symptom is a UI that quietly ignores you.
        let config = runtime_shared::primitives::link::LinkConfig {
            on_activate: flushing0(config.on_activate.clone()),
            ..config
        };
        LinuxBackend::create_link(self, config, a11y)
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for LinuxBackend {
    fn create_text_input(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body (formerly `impl Backend`),
        // which builds the richer widget this backend actually ships.
        // Flush after each author callback (see `flushing0`). `on_blur` is a
        // BlurHandler, not an Rc<dyn Fn()>, so it is left unwrapped — matching
        // master, which wrapped only on_change + on_key_down here.
        LinuxBackend::create_text_input(self, initial_value, _placeholder, flushing1(on_change), on_key_down.map(flushing_key), on_blur, secure, _a11y)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        LinuxBackend::update_text_input_secure(self, node, secure)
    }

    fn create_text_area(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        _wrap: bool,
        _min_rows: Option<u32>,
        _max_rows: Option<u32>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body (formerly `impl Backend`),
        // which builds the richer widget this backend actually ships.
        // Flush after each author callback (see `flushing0`). `on_blur` is a
        // BlurHandler, not an Rc<dyn Fn()>, so it is left unwrapped — matching
        // master, which wrapped only on_change + on_key_down here.
        LinuxBackend::create_text_area(self, initial_value, _placeholder, _wrap, _min_rows, _max_rows, flushing1(on_change), on_key_down.map(flushing_key), _a11y)
    }
}

impl caps::ToggleOps for LinuxBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body (formerly `impl Backend`),
        // which builds the richer widget this backend actually ships.
        // Wrap the author callback so ONE deduped flush microtask is queued
        // after it returns. Without this the handler mutates its signal and
        // nothing re-renders — the v2 flush is not implicit.
        LinuxBackend::create_toggle(self, initial_value, flushing1(on_change), _a11y)
    }
}

impl caps::SliderOps for LinuxBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body (formerly `impl Backend`),
        // which builds the richer widget this backend actually ships.
        // Wrap the author callback so ONE deduped flush microtask is queued
        // after it returns. Without this the handler mutates its signal and
        // nothing re-renders — the v2 flush is not implicit.
        LinuxBackend::create_slider(self, initial_value, min, max, _step, flushing1(on_change), _a11y)
    }
}

impl caps::ActivityIndicatorOps for LinuxBackend {
    fn create_activity_indicator(
        &mut self,
        _size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body (formerly `impl Backend`),
        // which builds the richer widget this backend actually ships.
        LinuxBackend::create_activity_indicator(self, _size, _color, _a11y)
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for LinuxBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body (formerly `impl Backend`),
        // which builds the richer widget this backend actually ships.
        //
        // `on_scroll` needs the flush wrapper like every other author
        // callback: a scroll-spy (the docs/website table of contents is
        // exactly this) writes the active-section signal from here, and
        // unwrapped that write stages and never commits — the TOC would
        // track internally and never repaint. Two args, so neither
        // `flushing0` nor `flushing1` fits; the flush microtask is
        // deduped, so a scroll burst still costs one commit. Mirrors
        // `MacosBackend::create_scroll_view`.
        let on_scroll = on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
            Rc::new(move |x, y| {
                f(x, y);
                schedule_flush();
            })
        });
        LinuxBackend::create_scroll_view(self, horizontal, on_scroll, _a11y)
    }

    /// Real scroll handle — same trap as `make_view_handle`: the trait
    /// default hands back a no-op, so imperative scrolling and
    /// scroll-position reads would silently do nothing.
    fn make_scroll_view_handle(
        &self,
        node: &Self::Node,
    ) -> runtime_shared::primitives::scroll_view::ScrollViewHandle {
        LinuxBackend::make_scroll_view_handle(self, node)
    }
}

impl caps::SafeAreaOps for LinuxBackend {}

// No two-axis grid engine on this backend yet; every `GridOps`
// method defaults, so `virtual_grid` reports itself as an
// unsupported primitive instead of silently rendering nothing.
impl caps::GridOps for LinuxBackend {}

impl caps::VirtualizerOps for LinuxBackend {
    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        // Real GTK body; the caps default here is a silent no-op.
        LinuxBackend::virtualizer_data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        // Real GTK body; the caps default here is a silent no-op.
        LinuxBackend::release_virtualizer(self, node)
    }
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _layout: primitives::virtualizer::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Rows are REALIZED and torn down here, which is creation-side work:
        // it needs the mounted world ambient (`World::enter`), and it must
        // queue a flush afterwards like any other author callback. master
        // built this same wrapper and then dropped it on the floor — its
        // Linux virtualizer was a placeholder. Ours is real, so the wrapped
        // callbacks are passed through to the GTK body.
        let VirtualizerCallbacks {
            item_count,
            item_key,
            item_size,
            measure_sizes,
            mount_item,
            release_item,
            set_measured_size,
            on_scroll,
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
            // Author scroll observer — same dispatch-site glue as
            // `create_scroll_view`'s `on_scroll` (stage writes, then
            // flush). Stays `None` when unset so the impl can skip
            // installing scroll observation entirely.
            on_scroll: on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
                Rc::new(move |x, y| {
                    f(x, y);
                    schedule_flush();
                })
            }),
        };
        LinuxBackend::create_virtualizer(self, callbacks, _overscan, _layout, _a11y)
    }
}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for LinuxBackend {
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        LinuxBackend::create_graphics(self, on_ready, on_resize, on_lost, _a11y)
    }
}

impl caps::PortalOps for LinuxBackend {
    fn release_portal(&mut self, node: &Self::Node) {
        // Real GTK body; the caps default here is a silent no-op.
        LinuxBackend::release_portal(self, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        // Real GTK body; the caps default here is a silent no-op.
        LinuxBackend::set_portal_hidden(self, node, hidden)
    }
    fn create_portal(
        &mut self,
        _target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        // Wrap the author callback so ONE deduped flush microtask is queued
        // after it returns. Without this the handler mutates its signal and
        // nothing re-renders — the v2 flush is not implicit.
        LinuxBackend::create_portal(self, _target, on_dismiss.map(flushing0), _trap_focus, _a11y)
    }
}

impl caps::PresenceOps for LinuxBackend {}

impl caps::NavigatorOps for LinuxBackend {}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for LinuxBackend {
    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Third-party primitives register a scene-`Registry` handler and
        // never reach this method — `create_external` only serves
        // `missing_primitive_placeholder`, so a labeled placeholder is the
        // whole contract. (The old core routed here through a backend-side
        // `ExternalRegistry`, which died with `Element::External`; the
        // placeholder shape is unchanged.)
        self.placeholder(&format!(
            "External \"{type_name}\" not registered on Linux backend"
        ))
    }
}

impl caps::DocumentOps for LinuxBackend {}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for LinuxBackend {
    fn apply_style(&mut self, _node: &Self::Node, _style: &Rc<StyleRules>) {
        // Delegates to the inherent GTK body — master's placeholder
        // here did not render at full fidelity.
        LinuxBackend::apply_style(self, _node, _style)
    }

    /// Drive `hover` / `pressed` / `focused` style variants from real GTK
    /// input.
    ///
    /// `handles_states_natively()` is `false` here (the trait default,
    /// and the right answer for GTK — there are no CSS pseudo-classes to
    /// hand states off to), which selects the EVENT-DRIVEN path: the
    /// framework hands us a setter and this backend is responsible for
    /// calling it. The default body is a no-op, so leaving it
    /// unimplemented compiles, renders every base style correctly, and
    /// silently lights no variant at all — every hover highlight, press
    /// feedback and focus ring in idea-ui was dead on Linux. See
    /// `states.rs` for the event mapping.
    fn attach_states(
        &mut self,
        node: &Self::Node,
        setter: Rc<dyn Fn(runtime_shared::StateBits, bool)>,
    ) {
        // Dispatch-site glue: flipping a state bit stages the re-resolve;
        // on native this IS the live state path, so it needs the flush
        // like any other author-visible write. Mirrors macOS.
        let setter: Rc<dyn Fn(runtime_shared::StateBits, bool)> = {
            let f = setter;
            Rc::new(move |bits, on| {
                f(bits, on);
                schedule_flush();
            })
        };
        let installed = crate::states::attach(&node.widget(), setter);
        // Replacing an existing entry detaches the old controllers first,
        // so a re-attach on the same node can't stack duplicates.
        if let Some(prev) = self.state_controllers.insert(node.id(), installed) {
            prev.detach(&node.widget());
        }
    }

    /// Release the state controllers when the node's style scope tears
    /// down. NOT optional: the setter writes a signal owned by that
    /// scope, and GTK emits `focus-leave` while the framework unparents a
    /// focused widget — i.e. AFTER the scope can already be gone. Writing
    /// through the freed slot panics inside a GObject signal trampoline,
    /// which cannot unwind, so it aborts the process rather than
    /// surfacing as a panic. See `states.rs`.
    fn on_node_unstyled(&mut self, node: &Self::Node) {
        if let Some(installed) = self.state_controllers.remove(&node.id()) {
            installed.detach(&node.widget());
        }
    }
}

impl caps::AssetOps for LinuxBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        LinuxBackend::register_asset(self, id, kind, source)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        LinuxBackend::register_typeface(self, id, family_name, faces, fallback)
    }
}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for LinuxBackend {}

impl caps::AnimationOps for LinuxBackend {
    // The trait defaults are no-ops. Leaving this impl empty meant every
    // per-frame animation write was silently discarded — the welcome
    // scene's three acts animate opacity/transform in, so the content
    // stayed at its initial (invisible) state on a correct layout tree.
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        LinuxBackend::set_animated_f32(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        LinuxBackend::set_animated_color(self, node, prop, value)
    }
}

// The trait's bodies all default to `None` / "unsupported", and leaving
// this impl empty meant the Robot surface was dark on Linux even though
// the backend already had every number it needed: `get_frame` /
// `get_absolute_frame` returned nothing (while `ViewHandle` reported the
// same values correctly to author code), and `screenshot` reported
// "not supported on this backend" — so an agent could drive a GTK app but
// never see or measure it. Same silently-degrading-default trap this
// backend hit before with `create_link` and `make_scroll_view_handle`.
// EVERY method here is behind `robot`. This whole trait is the DIAGNOSTIC
// surface — the robot bridge's `get_frame` / `get_absolute_frame` /
// `introspect_native` / `screenshot` verbs — and a shipped app must not carry
// it. With the feature off the trait's own defaults apply (`None`,
// `supports_* -> false`), which is the honest answer for a build that cannot
// serve them.
//
// The geometry helpers themselves (`node_frame` / `node_absolute_frame`) are
// NOT gated, and must not be: `ViewHandle::absolute_frame()` is a production
// author API reached through `handles.rs`, and real apps do real layout maths
// with it — the docs app's and the website's "On this page" scroll-spy both
// compare a section's absolute frame against the scroll viewport on every
// scroll event. Gating the helpers would break a shipping feature; gating this
// trait only removes the bridge's access to them.
impl caps::IntrospectionOps for LinuxBackend {
    #[cfg(feature = "robot")]
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        self.node_frame(node.id())
            .map(|(x, y, width, height)| ViewportRect { x, y, width, height })
    }

    #[cfg(feature = "robot")]
    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        self.node_absolute_frame(node.id())
            .map(|(x, y, width, height)| ViewportRect { x, y, width, height })
    }

    // `device_frame` (physical screen pixels, for OS-level input
    // injection) stays at the trait default `None` DELIBERATELY. A Wayland
    // client is not told where the compositor placed its surface — there
    // is no `wl_surface` → screen-origin query — so any number here would
    // be a guess that silently misaims injected clicks. Reporting `None`
    // makes callers fall back to widget-relative injection, which this
    // backend does support.

    #[cfg(feature = "robot")]
    fn supports_native_introspection(&self) -> bool {
        LinuxBackend::supports_native_introspection_impl(self)
    }

    #[cfg(feature = "robot")]
    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        LinuxBackend::introspect_native_impl(self, node.id())
    }

    #[cfg(feature = "robot")]
    fn note_introspection_root(&self, node: &Self::Node) {
        LinuxBackend::note_introspection_root_impl(self, node.widget())
    }

    #[cfg(feature = "robot")]
    fn supports_screenshot(&self) -> bool {
        LinuxBackend::supports_screenshot_impl(self)
    }

    #[cfg(feature = "robot")]
    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        LinuxBackend::capture_screenshot_impl(self, done)
    }
}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for LinuxBackend {}

impl caps::WireBindingOps for LinuxBackend {}

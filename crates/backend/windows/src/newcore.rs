//! Rendering: the `runtime_scene::Host` + capability-trait surface, the
//! boot entry, and the flush driver.
//!
//! [`WindowsBackend`] implements [`runtime_scene::Host`] plus every
//! capability trait (`runtime_vocabulary::caps`). Each method delegates to
//! an inherent painted-scene body in `lib.rs`; methods this backend does
//! not override are served by the caps-trait DEFAULT bodies — most of
//! which are silent no-ops, so a forgotten override compiles and quietly
//! does nothing.
//!
//! # Flush policy at the caps layer
//!
//! Capability methods that take an author callback wrap it before
//! delegating (`flushing0`/`flushing1`/`flushing_key` + the inline
//! wrappers below) so a staged write commits after the callback returns.
//!
//! # Boot sequence ([`start`])
//!
//! The host shell (`host-win32`) creates its top-level window, wraps it
//! in [`WindowsBackend::new`], and installs its
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
//! The kernel stages signal writes; nothing observes them until the
//! driver calls [`World::flush`]. Author callbacks reach this backend
//! two ways:
//!
//! 1. **Through the caps layer** — button / pressable / link / text
//!    input / toggle / slider / scroll / image / graphics / portal /
//!    virtualizer callbacks, and painted-view input handlers (the host
//!    routes mouse messages along the hit chain, `crate::view_input`).
//!    Each is wrapped here BEFORE the inherent body stores it, so
//!    whatever host message arm fires it (`WM_COMMAND`, `WM_HSCROLL`,
//!    hit-tested `WM_LBUTTONUP`, …) schedules a flush with zero host
//!    changes.
//! 2. **SDK seams** (`register_command_handler` /
//!    `install_command_handler_with_id`): take RAW closures from SDK
//!    leaf crates, bypassing the caps layer — see the residual seams.
//!
//! **Scheduler contract.** Two regimes:
//!
//! - *No scheduler installed* (a bare embedder, most tests):
//!   `schedule_microtask` falls back to SYNCHRONOUS execution off-Web
//!   (the `runtime-shared::scheduling` contract), so the deduped flush
//!   runs inline before the wrapped author callback returns. That is
//!   borrow-safe in `scroll_wnd_proc` (no backend borrow held across
//!   the callback) but a re-borrow hazard inside `dispatch_command`,
//!   which takes `&self`: a host holding `backend.borrow()` across the
//!   call will abort when the inline flush re-enters the backend
//!   mutably. Host contract: drop any `RefCell` borrow of the backend
//!   before author dispatch, or install a scheduler so the flush
//!   defers.
//! - *Scheduler installed*: the host MUST fire
//!   [`crate::dispatch_hook::fire_dispatch_hook`] after every
//!   scheduler-driven author callback (`after_ms` timers,
//!   `after_animation_frame` one-shots, `raf_loop` iterations) and
//!   after each `dispatch_command` return (which also covers the SDK
//!   command-handler seam), and drain microtasks once per message-pump
//!   iteration so the flush commits before the next paint
//!   (`host-win32`'s scheduler and `WM_COMMAND` arm do all three).
//!   Microtasks themselves never fire the hook (the flush rides one; see
//!   [`crate::dispatch_hook`]).
//!
//! # Viewport
//!
//! The host shell (`host-win32`) is the viewport source: it publishes
//! [`WindowsBackend::logical_client_size`] through
//! `runtime_shared::set_viewport_size` at boot, on `WM_SIZE` and on
//! `WM_DPICHANGED`. The size is in LOGICAL pixels — the same units the
//! layout pass runs in (`layout_pass` converts the device client rect the
//! same way; see `crate::dpi`), so breakpoints don't shift with the
//! monitor's scale.
//!
//! # Residual seams (named, none silent)
//!
//! - SDK command handlers (`register_command_handler` /
//!   `install_command_handler_with_id`) bypass the caps wraps; they
//!   are covered iff the host fires `dispatch_hook` after
//!   `dispatch_command` (above). SDK glue should call [`schedule_flush`]
//!   itself to be host-independent.
//! - `register_external_view` child HWNDs: author callbacks such a leaf
//!   wires natively must call [`schedule_flush`] themselves.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives;
use runtime_shared::primitives::image::{ImageErrorHandler, ImageLoadHandler};
#[cfg(feature = "robot")]
use runtime_shared::introspect::NativeNode;
#[cfg(feature = "robot")]
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::{
    Action, Color, ColorScheme, FileDropHandler, HoverHandler, Platform, StyleRules, TouchHandler, WheelHandler,
    VirtualizerCallbacks,
};
use runtime_shared::animation::AnimProp;
use runtime_shared::assets::{AssetId, AssetSource, AssetTag, SystemFallback, TypefaceFace, TypefaceId};
use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;

use crate::{WindowsBackend, WindowsNode};

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
    static BACKEND: RefCell<Option<Weak<RefCell<WindowsBackend>>>> = const { RefCell::new(None) };
}

/// Everything the boot path must keep alive. Field order is drop order:
/// the realized tree unmounts before the world (its slots' owner) dies.
/// The host shell holds this value for the whole session and calls
/// [`NewCoreApp::stop`] on teardown.
pub struct NewCoreApp {
    realized: Realized<WindowsNode>,
    _backend: Rc<RefCell<WindowsBackend>>,
    _registry: Rc<Registry<WindowsBackend>>,
    world: World,
}

impl NewCoreApp {
    /// Borrow the live tree (tests, diagnostics).
    pub fn with_realized<R>(&self, f: impl FnOnce(&Realized<WindowsNode>) -> R) -> R {
        f(&self.realized)
    }

    /// The mounted world (tests can flush it explicitly).
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Unmount: drops the `Realized` (cleanups fire), uninstalls the
    /// flush driver, and drops the world. Must run on the UI thread
    /// while its TLS is intact — the reactive teardown runs scope
    /// cleanups.
    pub fn stop(self) {
        // Release a held pointer lock (the clip rectangle is desktop-wide)
        // and show the cursor before the tree goes away.
        if let Ok(mut b) = self._backend.try_borrow_mut() {
            b.set_pointer_sink_impl(None);
        }
        crate::dispatch_hook::clear_dispatch_hook();
        set_flush_world(None);
        BACKEND.with(|b| *b.borrow_mut() = None);
        drop(self);
    }
}

/// Mount a scene element tree into an already-constructed backend.
///
/// The host must have constructed the backend around its parent HWND
/// ([`WindowsBackend::new`]) and, if it uses one, installed its
/// scheduler first — see the module docs' scheduler contract.
///
/// `register` runs after [`runtime_vocabulary::register_builtins`], so
/// apps/SDKs can register their own payload handlers on the same
/// registry before the tree realizes. The build closure runs inside
/// `world.enter`, so free `signal()`/`effect()` calls work; top-level
/// creations are world-root-owned (they live until [`NewCoreApp::stop`]).
#[inline]
pub fn start(
    backend: Rc<RefCell<WindowsBackend>>,
    register: impl FnOnce(&mut Registry<WindowsBackend>),
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
    backend: Rc<RefCell<WindowsBackend>>,
    register: R,
    build: B,
) -> NewCoreApp where
    S: runtime_vocabulary::BuiltinSet,
    R: FnOnce(&mut Registry<WindowsBackend>),
    B: FnOnce() -> Element,
{
    // Monotonic clock (idempotent, first install wins) — animation and
    // presence timing read it. Installed explicitly like macOS/wgpu.
    let platform = caps::AppEnvOps::platform(&*backend.borrow());
    runtime_shared::time::install_default_time_source(platform);

    // Ambient environment services (platform identity, color scheme, URL
    // opener, full-screen setter, AX announcer) -> the thread-locals
    // `platform()` / `open_url()` / `announce()` etc. read. MUST precede
    // the build: a component body may read `platform()` while
    // constructing. See `runtime_vocabulary::backend`.
    runtime_vocabulary::backend::install_env_services(&backend);
    let mut registry: Registry<WindowsBackend> = Registry::new();
    runtime_vocabulary::register_builtins_with::<_, S>(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    let world = World::new();
    let realized = world.enter(|| {
        let element = build();
        realize(&backend, &registry, element)
    });

    // Buffered-microtask drain — a no-op under a real host scheduler
    // (and under the no-scheduler synchronous fallback), load-bearing
    // under a buffering test scheduler. Must run with NO backend
    // borrow held (drained tasks re-borrow); ENTERED because a
    // buffered task may do creation-side work.
    world.enter(runtime_shared::scheduling::drain_buffered_microtasks);

    // Single-root contract (`find_root` wants exactly one application
    // root — id 1).
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_windows::newcore::start: the app root must contribute exactly one \
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
    BACKEND.with(|b| *b.borrow_mut() = Some(Rc::downgrade(&backend)));
    // App situation at boot (focus / minimized / reduce motion / pointer
    // kind); later changes arrive through the host WndProc
    // (`app_situation::window_message`). Reported with NO backend borrow
    // held — a report runs world observers.
    let host = backend.borrow().host_hwnd();
    for fact in crate::app_situation::boot_facts(host) {
        runtime_shared::app_state::report(fact);
    }
    schedule_flush();
    // Point the Robot bridge at the registry this boot actually fills.
    #[cfg(feature = "robot")]
    install_robot_env();
    // The live `screenshot` verb, answered by this backend's real capture.
    // Without it local-mount native apps answered `unknown command:
    // screenshot` — only the dev-server sidecar registered one, so an
    // `idealyst dev --local` app could be driven but never seen.
    #[cfg(feature = "robot")]
    {
        let weak = std::rc::Rc::downgrade(&backend);
        runtime_shared::robot::screenshot::register_native_screenshot(move |done| match weak.upgrade() {
            Some(b) => caps::IntrospectionOps::capture_screenshot(&*b.borrow(), done),
            None => done(Err("the app has been torn down".into())),
        });
    }
    NewCoreApp {
        realized,
        _backend: backend,
        _registry: registry,
        world,
    }
}

/// Point the Robot bridge at the vocabulary registry and give it a way to
/// enter the mounted world — the same wiring, for the same reason, as
/// `backend_linux::newcore::install_robot_env` (read its doc for details).
///
/// In short: the shared bridge dispatches to the substrate's own (EMPTY)
/// registry by default, so an app that merely starts the bridge answers
/// `get_snapshot` with `[]` and `find_element` with `null` on a perfectly
/// healthy tree — a silently blinded driver. `install_verb_router` routes to
/// the vocabulary registry realize fills, falling back on the exact
/// `unknown command:` marker so substrate-owned verbs (`get_logs`, host
/// commands) still resolve. `install_driver_env` runs queries inside the
/// mounted world (a reactive `label_fn` reads world signals) and settles
/// actions through a flush so a `click` returns after its writes commit.
#[cfg(feature = "robot")]
fn install_robot_env() {
    runtime_vocabulary::robot::install_driver_env(
        |f| match FLUSH_WORLD.with(|w| w.borrow().clone()) {
            Some(world) => world.enter(|| f()),
            // Pre-boot / post-stop: no world; static labels still resolve.
            None => f(),
        },
        || {
            if let Some(world) = FLUSH_WORLD.with(|w| w.borrow().clone()) {
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

/// True while an app is mounted (`start` ran, `stop` hasn't).
pub fn is_booted() -> bool {
    FLUSH_WORLD.with(|w| w.borrow().is_some())
}

/// Run `f` with the mounted backend (diagnostics). `None` before
/// [`start`] or after the backend dropped.
pub fn with_backend<R>(f: impl FnOnce(&Rc<RefCell<WindowsBackend>>) -> R) -> Option<R> {
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
// returns, one deduped flush microtask is queued.

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

/// Wrap a by-reference one-value author callback (`on_load(&ImageLoadEvent)`).
fn flushing1_ref<A: ?Sized + 'static>(f: Rc<dyn Fn(&A)>) -> Rc<dyn Fn(&A)> {
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

/// Wrap the app-level pointer sink: flush after every event, lock change
/// and focus-loss release burst (all run author listeners). The outcome
/// passes through untouched.
fn flushing_pointer_sink(
    sink: runtime_shared::pointer_input::PointerSink,
) -> runtime_shared::pointer_input::PointerSink {
    let on_lock = sink.clone();
    let on_focus_lost = sink.clone();
    runtime_shared::pointer_input::PointerSink::new(
        move |ev| {
            let outcome = sink.event(ev);
            schedule_flush();
            outcome
        },
        move |locked| {
            on_lock.lock_changed(locked);
            schedule_flush();
        },
        move || {
            on_focus_lost.focus_lost();
            schedule_flush();
        },
    )
}

/// Wrap the app-level keyboard sink: flush after every key delivery and
/// after a focus-loss release burst (both run author listeners). The key
/// outcome passes through untouched.
fn flushing_sink(sink: primitives::key::KeyboardSink) -> primitives::key::KeyboardSink {
    let on_focus_lost = sink.clone();
    primitives::key::KeyboardSink::new(
        move |ev| {
            let outcome = sink.key(ev);
            schedule_flush();
            outcome
        },
        move || {
            on_focus_lost.focus_lost();
            schedule_flush();
        },
    )
}

// ===========================================================================
// Host + capability-trait delegation (the AllCaps bound on
// register_builtins is the compile gate)
// ===========================================================================

// ---------------------------------------------------------------------------
// Host — the structural seam
// ---------------------------------------------------------------------------

impl Host for WindowsBackend {
    type Node = WindowsNode;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        // Delegates to the inherent painted-scene body, which links the
        // Taffy child AND records the parent→children edge the painter and
        // hit tester walk (painted nodes have no HWND, so a bare
        // `SetParent` would leave the paint tree empty).
        WindowsBackend::insert(self, parent, child)
    }

    // `insert_many` is deliberately NOT implemented: `Host`'s default
    // N-x-`insert` loop gives the right child order.

    /// Append, ignoring the index. Never reached in practice:
    /// [`supports_splice`](Self::supports_splice) is `false`, so reactive
    /// regions rebuild wholesale under their own anchor and no positional
    /// splice is ever emitted.
    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, _index: usize) {
        self.insert(parent, child)
    }

    /// A no-op. Only meaningful for splice-capable hosts; this one is
    /// anchored, so the framework never calls it.
    fn remove_child(&mut self, _parent: &Self::Node, _child: &Self::Node) {
        // default: no-op
    }

    fn clear_children(&mut self, node: &Self::Node) {
        // Detach only: the navigator re-inserts a cleared screen. See the
        // inherent body.
        WindowsBackend::clear_children(self, node)
    }

    /// Free a discarded subtree (node meta, Taffy node, control HWNDs).
    /// Reactive regions call this for every branch they throw away, so a
    /// detach-only `clear_children` leaks nothing.
    fn release_subtree(&mut self, node: &Self::Node) {
        WindowsBackend::release_subtree(self, node)
    }

    /// A layout- and hit-transparent node: this backend's
    /// `display: contents`, as on web, iOS, macOS and Android. This backend
    /// anchors EVERY reactive region (`supports_splice` is false), and a
    /// plain view here would be a flex item the author never wrote: a
    /// `height: 100%` / `flex_grow` child under an `if` lays out at 0, a
    /// row's children under a `for` stack into a column, and `gap` stops
    /// separating them.
    fn create_anchor(&mut self) -> Self::Node {
        WindowsBackend::create_anchor(self)
    }

    /// `false` — ANCHORED mode: reactive regions rebuild wholesale under
    /// their own anchor. Flipping it to `true` needs a real positional
    /// `insert_at` / `remove_child` (both are stubs above).
    fn supports_splice(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

impl caps::AppEnvOps for WindowsBackend {
    fn color_scheme(&self) -> ColorScheme {
        // Win32 doesn't expose a single "dark mode" toggle until very
        // recent builds (UISettings::Background via WinRT). Return Auto
        // and let the framework's theme APIs own the decision. Reading
        // `AppsUseLightTheme` from the registry under
        // `Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`
        // would answer it.
        ColorScheme::Auto
    }

    fn platform(&self) -> Platform {
        Platform::Custom("windows")
    }

    fn set_app_background(&mut self, color: &runtime_shared::Tokenized<Color>) {
        WindowsBackend::set_app_background(self, color)
    }

    fn set_keyboard_sink(&mut self, sink: Option<primitives::key::KeyboardSink>) {
        // Dispatch-site glue: the sink runs author listeners from the host's
        // message pump (outside any wrapped callback), so flush after each
        // delivery. Native source: `app_keys.rs`.
        WindowsBackend::set_keyboard_sink_impl(self, sink.map(flushing_sink))
    }

    fn set_pointer_sink(&mut self, sink: Option<runtime_shared::pointer_input::PointerSink>) {
        // Same dispatch-site glue as the keyboard: the host pump runs the
        // sink outside any wrapped callback. Native source: `app_pointer.rs`.
        WindowsBackend::set_pointer_sink_impl(self, sink.map(flushing_pointer_sink))
    }

    fn set_pointer_lock(&mut self, locked: bool) {
        // ClipCursor + raw input; the grant is reported from a microtask
        // (this runs inside the dispatcher's backend borrow).
        WindowsBackend::set_pointer_lock_impl(self, locked)
    }

    /// `open_url`: the shell's default handler for the URL (the default
    /// browser for `http(s)`, the mail client for `mailto:`, …).
    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        Some(Rc::new(|url: &str| crate::open_url_with_shell(url)))
    }

    fn set_cursor_hidden(&mut self, hidden: bool) {
        // WM_SETCURSOR over the client area — `app_pointer.rs`.
        WindowsBackend::set_cursor_hidden_impl(self, hidden)
    }
}

impl caps::LifecycleOps for WindowsBackend {
    fn finish(&mut self, root: Self::Node) {
        // Delegates to the inherent painted-scene body: record the root,
        // run the Taffy layout pass, invalidate for repaint. Most nodes
        // have no window — `layout_pass` computes frames for painting and
        // `position_native_children` moves the few real controls.
        WindowsBackend::finish(self, root)
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for WindowsBackend {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        // Delegates to the inherent painted-scene body: a painted view
        // carries background/border/radius and clips its children (a
        // STATIC child HWND can do none of that).
        WindowsBackend::create_view(self, _a11y)
    }

    /// Real node handle, NOT the trait's defaulted no-op: `AnimatedValue::bind`
    /// drives every per-frame write through this handle, so the default means
    /// the animation clock ticks and nothing ever paints.
    fn make_view_handle(&self, node: &Self::Node) -> runtime_shared::ViewHandle {
        WindowsBackend::make_view_handle(self, node)
    }
}

/// Painted views have no HWNDs: the handlers go into the backend's
/// `view_input` tables, and the host routes mouse messages to them along
/// the hit chain (`crate::view_input`). Wrapped to flush after author
/// code, like every other dispatch site here.
impl caps::InputOps for WindowsBackend {
    fn install_touch_handler(&mut self, node: &Self::Node, handler: TouchHandler) {
        let f = handler;
        let handler: TouchHandler = Rc::new(move |ev| {
            let response = f(ev);
            schedule_flush();
            response
        });
        self.view_input.touch.insert(node.id, handler);
    }

    fn install_wheel_handler(&mut self, node: &Self::Node, handler: WheelHandler) {
        let f = handler;
        let handler: WheelHandler = Rc::new(move |ev| {
            let response = f(ev);
            schedule_flush();
            response
        });
        self.view_input.wheel.insert(node.id, handler);
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        self.view_input.hover.insert(node.id, flushing1(handler));
    }

    fn install_file_drop_handler(&mut self, node: &Self::Node, handler: FileDropHandler) {
        let f = handler;
        let handler: FileDropHandler = Rc::new(move |ev| {
            let response = f(ev);
            schedule_flush();
            response
        });
        self.install_file_drop_handler_impl(node.id, handler);
    }
}

impl caps::PressableOps for WindowsBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> Self::Node {
        // Wrap the author callback so ONE deduped flush microtask is queued
        // after it returns — the flush is not implicit.
        //
        // Delegates to the inherent painted body: a Pressable is a styleable
        // painted container carrying an on_click, fired by the host's
        // hit-tested WM_LBUTTONUP (not a STATIC control + WM_COMMAND, which
        // STATIC controls do not emit without subclassing).
        WindowsBackend::create_pressable(self, flushing0(on_click), _a11y)
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for WindowsBackend {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        // Delegates to the inherent painted body — text is a painted leaf
        // with real typography/measurement, not a STATIC control.
        WindowsBackend::create_text(self, content, _a11y)
    }

    /// Real text handle, NOT the trait's defaulted no-op: `AnimatedValue::bind`
    /// drives per-frame writes through it, so the default silently animates
    /// nothing.
    fn make_text_handle(&self, node: &Self::Node) -> runtime_shared::TextHandle {
        WindowsBackend::make_text_handle(self, node)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        WindowsBackend::update_text(self, node, content)
    }
}

impl caps::ButtonOps for WindowsBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&primitives::icon::IconData>,
        _trailing_icon: Option<&primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Wrap the Action's runtime evaluator so the press queues a flush;
        // the serialization metadata passes through untouched.
        let on_click = Action { fire: flushing0(on_click.fire.clone()), ..on_click.clone() };
        // Delegates to the inherent body, which creates the real BUTTON
        // control and registers the WM_COMMAND handler itself.
        WindowsBackend::create_button(
            self,
            label,
            &on_click,
            _leading_icon,
            _trailing_icon,
            _a11y,
        )
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        WindowsBackend::update_button_label(self, node, label)
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for WindowsBackend {
    fn create_image(
        &mut self,
        _src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        WindowsBackend::create_image(self, _src, _alt, _a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        WindowsBackend::update_image_src(self, node, src)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        // Dispatch-site glue: image events fire from the image pump
        // (`image_fetch.rs`), outside any flush, so queue one after.
        WindowsBackend::install_image_load_handler(self, node, flushing1_ref(handler))
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        WindowsBackend::install_image_error_handler(self, node, flushing0(handler))
    }
}

impl caps::IconOps for WindowsBackend {
    fn create_icon(
        &mut self,
        _data: &runtime_shared::primitives::icon::IconData,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        WindowsBackend::create_icon(self, _data, _color, _a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        WindowsBackend::update_icon_color(self, node, color)
    }

    fn update_icon_data(
        &mut self,
        node: &Self::Node,
        data: &runtime_shared::primitives::icon::IconData,
    ) {
        WindowsBackend::update_icon_data(self, node, data)
    }
}

impl caps::LinkOps for WindowsBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // The trait DEFAULT collapses to `create_view` and drops
        // `on_activate` — every link would render as inert text and nothing
        // could be navigated to.
        //
        // Wrap the activation so the navigation's signal writes are flushed.
        let config = primitives::link::LinkConfig {
            on_activate: flushing0(config.on_activate.clone()),
            ..config
        };
        WindowsBackend::create_link(self, config, _a11y)
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for WindowsBackend {
    fn create_text_input(
        &mut self,
        initial_value: &str,
        _placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        _secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let on_change = flushing1(on_change);
        let on_key_down = on_key_down.map(flushing_key);
        let on_blur = on_blur.map(|f| -> primitives::text_input::BlurHandler {
                Rc::new(move || {
                    let outcome = f();
                    schedule_flush();
                    outcome
                })
            });
        // Delegates to the inherent body, which creates a real EDIT
        // control and wires its EN_CHANGE notification.
        WindowsBackend::create_text_input(
            self,
            initial_value,
            _placeholder,
            on_change,
            on_key_down,
            on_blur,
            _secure,
            _a11y,
        )
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
        let on_change = flushing1(on_change);
        let on_key_down = on_key_down.map(flushing_key);
        WindowsBackend::create_text_area(
            self,
            initial_value,
            _placeholder,
            _wrap,
            _min_rows,
            _max_rows,
            on_change,
            on_key_down,
            _a11y,
        )
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        WindowsBackend::update_edit_value(self, node, value, false)
    }

    fn update_text_area_value(&mut self, node: &Self::Node, value: &str) {
        WindowsBackend::update_edit_value(self, node, value, true)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        WindowsBackend::update_text_input_secure(self, node, secure)
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        WindowsBackend::update_text_input_placeholder(self, node, placeholder)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
        WindowsBackend::set_text_input_focus_handler(self, node, flushing1(handler))
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        crate::handles::make_text_input_handle(node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        crate::handles::make_text_area_handle(node)
    }
}

impl caps::ToggleOps for WindowsBackend {
    fn create_toggle(
        &mut self,
        _initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let on_change = flushing1(on_change);
        WindowsBackend::create_toggle(self, _initial_value, on_change, _a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        WindowsBackend::update_toggle_value(self, node, value)
    }
}

impl caps::SliderOps for WindowsBackend {
    fn create_slider(
        &mut self,
        _initial_value: f32,
        _min: f32,
        _max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let on_change = flushing1(on_change);
        WindowsBackend::create_slider(
            self,
            _initial_value,
            _min,
            _max,
            _step,
            on_change,
            _a11y,
        )
    }
}

impl caps::ActivityIndicatorOps for WindowsBackend {
    fn create_activity_indicator(
        &mut self,
        _size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        WindowsBackend::create_activity_indicator(self, _size, _color, _a11y)
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for WindowsBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: an author `on_scroll` mutates signals, so it
        // needs the deduped flush like every other callback here.
        let on_scroll: Option<Rc<dyn Fn(f32, f32)>> = on_scroll.map(|f| {
            Rc::new(move |x, y| {
                f(x, y);
                schedule_flush();
            }) as Rc<dyn Fn(f32, f32)>
        });
        // Delegates to the inherent painted body: a scroll view is a painted
        // View carrying `ScrollInfo`, and the body also marks the Taffy node
        // `overflow: scroll` on the axis — without that the viewport sizes to
        // its content and there is nothing to scroll. The painted model
        // scrolls by offsetting the painter and `position_native_children`,
        // so no per-scroller window class is needed.
        WindowsBackend::create_scroll_view(self, horizontal, on_scroll, _a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        WindowsBackend::node_scroll(self, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        WindowsBackend::set_node_scroll(self, node, x, y)
    }

    fn observe_scroll_end(&mut self, node: &Self::Node, _horizontal: bool, threshold: f32, on_end: Rc<dyn Fn()>) {
        // The axis is the scroll view's own; `on_end` is a dispatch site.
        WindowsBackend::observe_scroll_end(self, node, threshold, flushing0(on_end))
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        crate::handles::make_scroll_view_handle(self, node)
    }
}

impl caps::SafeAreaOps for WindowsBackend {}

// No two-axis grid engine on this backend yet; every `GridOps`
// method defaults, so `virtual_grid` reports itself as an
// unsupported primitive instead of silently rendering nothing.
impl caps::GridOps for WindowsBackend {}

impl caps::VirtualizerOps for WindowsBackend {
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        _overscan: f32,
        _layout: primitives::virtualizer::VirtualLayout,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue + world entry: mount/release run author
        // render closures and scope cleanups; mount_item REALIZES the
        // row (creation-side work that needs the ambient world — without
        // it flat_list renders zero rows).
        // item_count/item_key/item_size are pure reads, unwrapped.
        let VirtualizerCallbacks {
            item_count,
            item_key,
            item_size,
            measure_sizes,
            mount_item,
            release_item,
            set_measured_size,
            on_scroll,
            item_changed,
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
            // Pure read (like `item_key`): no world entry, no flush.
            item_changed,
        };
        WindowsBackend::create_virtualizer(self, callbacks, _overscan, _layout, _a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        WindowsBackend::virtualizer_data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        WindowsBackend::release_virtualizer(self, node)
    }

    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle {
        crate::handles::make_virtualizer_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for WindowsBackend {
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: surface lifecycle callbacks run author code,
        // so each queues the deduped flush before handing off.
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
        WindowsBackend::create_graphics(self, on_ready, on_resize, on_lost, _a11y)
    }
}

impl caps::PortalOps for WindowsBackend {
    fn create_portal(
        &mut self,
        _target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let on_dismiss = on_dismiss.map(flushing0);
        WindowsBackend::create_portal(self, _target, on_dismiss, _trap_focus, _a11y)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        WindowsBackend::set_portal_hidden(self, node, hidden)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        WindowsBackend::release_portal(self, node)
    }
}

impl caps::PresenceOps for WindowsBackend {}

impl caps::NavigatorOps for WindowsBackend {}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for WindowsBackend {
    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Third-party primitives register a scene-`Registry` handler and
        // never reach this method — `create_external` only serves
        // `missing_primitive_placeholder`, so the labeled placeholder is
        // the whole contract.
        self.placeholder(&format!(
            "External \"{type_name}\" not registered on Windows backend"
        ))
    }
}

impl caps::DocumentOps for WindowsBackend {}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for WindowsBackend {
    /// Native inert state for the `disabled` prop — see the inherent
    /// `WindowsBackend::set_disabled` (`EnableWindow`).
    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        WindowsBackend::set_disabled(self, node, disabled)
    }

    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        // Delegates to the inherent body, which translates the rules into
        // the Taffy style AND the painted visual (background, border,
        // radius, gradient, opacity, transform, overflow).
        WindowsBackend::apply_style(self, node, style)
    }

    /// Hover / press → the node's state signal. The trait default is a
    /// no-op that lights no `state hovered` / `state pressed` variant ever;
    /// `view_input` flips the bits from the host's mouse messages (hit
    /// chain), since painted views have no HWND to observe. Native child
    /// controls draw their own hover / press. FOCUSED is not driven.
    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(runtime_shared::StateBits, bool)>) {
        // A bit flip stages the node's re-resolve; on native this IS the
        // live state path, so it needs the flush like any author write.
        let setter: Rc<dyn Fn(runtime_shared::StateBits, bool)> = Rc::new(move |bit, on| {
            setter(bit, on);
            schedule_flush();
        });
        self.view_input.states.insert(node.id, setter);
    }

    /// Stop delivering state flips to a node whose style scope is gone.
    fn on_node_unstyled(&mut self, node: &Self::Node) {
        self.view_input.states.remove(&node.id);
    }
}

impl caps::AssetOps for WindowsBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        WindowsBackend::register_asset(self, id, kind, source)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        // Installs each face into the process font table so later text
        // measurement and painting can name the family.
        WindowsBackend::register_typeface(self, id, family_name, faces, fallback)
    }
}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for WindowsBackend {}

impl caps::AnimationOps for WindowsBackend {
    // Per-frame animation writes. The trait defaults are no-ops, so without
    // these an animation's clock runs and the scene never changes.
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        WindowsBackend::set_animated_f32(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        WindowsBackend::set_animated_color(self, node, prop, value)
    }
}

// EVERY method here is behind `robot`: this trait is the DIAGNOSTIC surface
// (the bridge's `get_frame` / `get_absolute_frame` / `introspect_native`
// verbs) and a shipped app must not carry it. With the feature off the
// trait's defaults apply (`None`, `supports_* -> false`), the honest answer
// for a build that cannot serve them. The geometry helpers themselves
// (`node_frame` / `node_abs_frame`) stay ungated — `ViewHandle::absolute_frame`
// is a production author API.
impl caps::IntrospectionOps for WindowsBackend {
    #[cfg(feature = "robot")]
    fn is_shown(&self, node: &Self::Node) -> Option<bool> {
        Some(self.node_is_shown(node.id))
    }

    #[cfg(feature = "robot")]
    fn hit_contains(&self, node: &Self::Node, x: f32, y: f32) -> Option<bool> {
        self.hit_contains_impl(node.id, x, y)
    }

    #[cfg(feature = "robot")]
    fn native_pointer(&self, _node: &Self::Node, steps: &[caps::PointerStep]) -> Option<caps::NativeInjection> {
        self.native_pointer_impl(steps)
    }

    #[cfg(feature = "robot")]
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        self.node_frame(node.id)
            .map(|(x, y, width, height)| ViewportRect { x, y, width, height })
    }

    #[cfg(feature = "robot")]
    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        self.node_abs_frame(node.id)
            .map(|(x, y, width, height)| ViewportRect { x, y, width, height })
    }

    #[cfg(feature = "robot")]
    fn supports_native_introspection(&self) -> bool {
        true
    }

    #[cfg(feature = "robot")]
    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        self.introspect_native_impl(node.id)
    }

    #[cfg(feature = "robot")]
    fn supports_screenshot(&self) -> bool {
        self.supports_screenshot_impl()
    }

    #[cfg(feature = "robot")]
    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<runtime_shared::Screenshot, String>)>) {
        self.capture_screenshot_impl(done)
    }
}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for WindowsBackend {}

impl caps::WireBindingOps for WindowsBackend {}

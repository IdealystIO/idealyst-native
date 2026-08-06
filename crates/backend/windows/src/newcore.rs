//! Rendering: the `runtime_scene::Host` + capability-trait surface, the
//! boot entry, and the flush driver.
//!
//! [`WindowsBackend`] implements [`runtime_scene::Host`] plus **all 30**
//! capability traits (`runtime_vocabulary::caps`) — the production shape
//! of the migration. Every mechanism body in this file was moved here
//! verbatim from the crate's old `impl runtime_core::Backend for WindowsBackend`
//! when the 159-method mega-trait was deleted, so the Win32 control mechanism code
//! is unchanged: the same scene builds the same HWND tree.
//! Capabilities this backend does not implement are simply absent — the
//! caps-trait DEFAULT bodies serve them, and those defaults were audited
//! byte-for-byte against the `Backend` defaults they replace
//! (`docs/runtime-v2-deletion-baseline.md` S2.1; 129 of this backend's
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
//! The host shell (out-of-repo; see the crate docs' test plan) creates
//! its top-level window, wraps it in [`WindowsBackend::new`], and — if
//! it has one — installs its `runtime_shared::scheduling::Scheduler`
//! BEFORE calling [`start`]:
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
//! driver calls [`World::flush`]. This scaffold fires author callbacks
//! from exactly three places, and all three are covered:
//!
//! 1. **`WindowsBackend::dispatch_command`** (called by the host
//!    WndProc's `WM_COMMAND` arm): looks up
//!    `command_handlers[control_id]` and invokes it. Under new-core
//!    that stored closure IS the wrapped one —
//!    `caps::ButtonOps::create_button` / `create_pressable` wrap
//!    `on_click` BEFORE delegating, and the old `Backend` bodies clone
//!    the (already wrapped) closure into `command_handlers`. Every
//!    `WM_COMMAND` dispatch therefore schedules a flush with zero
//!    host changes.
//! 2. **`scroll_wnd_proc`'s `WM_MOUSEWHEEL` arm**: fires
//!    `ScrollState::on_scroll` — wrapped at
//!    `caps::ScrollOps::create_scroll_view` before the old body boxes
//!    it into `GWLP_USERDATA`.
//! 3. **SDK seams** (`register_command_handler` /
//!    `install_command_handler_with_id`): take RAW closures from SDK
//!    leaf crates, bypassing the caps layer — residual, see below.
//!
//! Toggle / slider / text-input / text-area / graphics / portal /
//! virtualizer are placeholders that never fire their callbacks today;
//! they are wrapped uniformly anyway so the flush arrives for free the
//! day a placeholder becomes a real control.
//!
//! **Scheduler contract (honest statement).** The scaffold installs no
//! `runtime_shared::scheduling::Scheduler` and this repo has no Windows
//! host crate. Two regimes:
//!
//! - *No scheduler installed* (the scaffold's world today):
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
//!   iteration so the flush commits before the next paint —
//!   `host_terminal`'s scheduler tick is the model. Microtasks
//!   themselves never fire the hook (the flush rides one; see
//!   [`crate::dispatch_hook`]).
//!
//! # Viewport
//!
//! **No viewport source on this scaffold.** `finish()` reads the host
//! client rect via `GetClientRect` directly for the Taffy pass, and
//! nothing writes `runtime_shared::set_viewport_size` on EITHER core, so
//! the world's viewport ctx keeps its default seed and old/new
//! behavior matches by construction. When a resize seam lands
//! (`WM_SIZE` forwarding) it must write the old TLS value and a
//! new-core sink side by side — the terminal's `set_viewport` +
//! `forward_viewport` pattern; the two sinks must never diverge.
//!
//! # Residual seams (named, none silent)
//!
//! - SDK command handlers (`register_command_handler` /
//!   `install_command_handler_with_id`) bypass the caps wraps; they
//!   are covered iff the host fires `dispatch_hook` after
//!   `dispatch_command` (above). SDK glue ported to new-core should
//!   call [`schedule_flush`] itself to be host-independent.
//! - `register_external_view` externals: author callbacks an External
//!   leaf wires natively must call [`schedule_flush`] when those SDKs
//!   are ported.
//! - The old-core `NavigatorRegistry`/inventory registrars keep
//!   serving the old path only; new-core navigators are vocabulary
//!   built-ins (swap/stack), so `Element::Navigator` routes through
//!   `Backend::create_navigator` exactly as before.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives;
use runtime_shared::{
    Action, Color, ColorScheme, Platform, StyleRules,
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
    /// cleanups (same rationale as the old host's explicit `Owner`
    /// drop on the terminal).
    pub fn stop(self) {
        crate::dispatch_hook::clear_dispatch_hook();
        set_flush_world(None);
        BACKEND.with(|b| *b.borrow_mut() = None);
        drop(self);
    }
}

/// Mount a new-core element tree into an already-constructed backend.
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

    // Single-root contract, matching the old-core mount (`find_root`
    // wants exactly one application root — id 1).
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

/// True while a new-core app is mounted (`start` ran, `stop` hasn't).
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

impl Host for WindowsBackend {
    type Node = WindowsNode;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        // Delegates to the inherent painted-scene body, which links the
        // Taffy child AND records the parent→children edge the painter and
        // hit tester walk. The scaffold this replaced only did the Taffy
        // link plus a `SetParent`, which is meaningless for painted nodes
        // (they have no HWND) and left the paint tree empty.
        WindowsBackend::insert(self, parent, child)
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
        // Delegates to the inherent body, which drops each child subtree
        // (`remove_subtree`: node meta, Taffy node, and any real control
        // HWND). The scaffold this replaced was a no-op, so every reactive
        // rebuild leaked the old subtree and painted it under the new one.
        WindowsBackend::clear_children(self, node)
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

impl caps::AppEnvOps for WindowsBackend {
    fn color_scheme(&self) -> ColorScheme {
        // Win32 doesn't expose a single "dark mode" toggle until very
        // recent builds (UISettings::Background via WinRT). For the
        // scaffold we return Auto and let the framework's theme APIs
        // own the decision. A future revision can read
        // `AppsUseLightTheme` from the registry under
        // `Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`.
        ColorScheme::Auto
    }

    fn platform(&self) -> Platform {
        Platform::Custom("windows")
    }

    fn set_app_background(&mut self, color: &runtime_shared::Tokenized<Color>) {
        WindowsBackend::set_app_background(self, color)
    }
}

impl caps::LifecycleOps for WindowsBackend {
    fn finish(&mut self, root: Self::Node) {
        // Delegates to the inherent painted-scene body: record the root,
        // run the Taffy layout pass, invalidate for repaint.
        //
        // The scaffold this replaced walked `self.nodes` and issued a
        // `SetWindowPos` per node, which only works when every node is a
        // child HWND. In the painted model most nodes have no window at
        // all — `layout_pass` computes frames for painting and
        // `position_native_children` moves the few real controls.
        WindowsBackend::finish(self, root)
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for WindowsBackend {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        // Delegates to the inherent painted-scene body (formerly
        // `impl Backend`). The scaffold this replaced made every view a
        // STATIC child HWND, which cannot carry background/border/radius
        // or clip its children — the painter does all of that.
        WindowsBackend::create_view(self, _a11y)
    }

    /// Real node handle, NOT the trait's defaulted no-op: `AnimatedValue::bind`
    /// drives every per-frame write through this handle, so the default means
    /// the animation clock ticks and nothing ever paints.
    fn make_view_handle(&self, node: &Self::Node) -> runtime_shared::ViewHandle {
        WindowsBackend::make_view_handle(self, node)
    }
}

impl caps::InputOps for WindowsBackend {}

impl caps::PressableOps for WindowsBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> Self::Node {
        // Wrap the author callback so ONE deduped flush microtask is queued
        // after it returns — the v2 flush is not implicit.
        //
        // Delegates to the inherent painted body: a Pressable is a styleable
        // painted container carrying an on_click, fired by the host's
        // hit-tested WM_LBUTTONUP. The scaffold this replaced tried to drive
        // it through a STATIC control id + WM_COMMAND, which STATIC controls
        // do not emit without subclassing — so presses never fired at all.
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
        // Delegates to the inherent painted-scene body — the branch's
        // Win32 implementation, which master's seam stubbed out.
        WindowsBackend::create_image(self, _src, _alt, _a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        WindowsBackend::update_image_src(self, node, src)
    }
}

impl caps::IconOps for WindowsBackend {
    fn create_icon(
        &mut self,
        _data: &runtime_shared::primitives::icon::IconData,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Delegates to the inherent painted-scene body — the branch's
        // Win32 implementation, which master's seam stubbed out.
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
        // `on_activate` — every link renders as inert text and nothing can
        // be navigated to. Same bug the Linux and terminal backends fixed.
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
        // control and wires its EN_CHANGE notification. The scaffold this
        // replaced made a read-only STATIC, so typing did nothing.
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
}

impl caps::ToggleOps for WindowsBackend {
    fn create_toggle(
        &mut self,
        _initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let on_change = flushing1(on_change);
        // Delegates to the inherent painted-scene body — the branch's
        // Win32 implementation, which master's seam stubbed out.
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
        // Delegates to the inherent painted-scene body — the branch's
        // Win32 implementation, which master's seam stubbed out.
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
        // Delegates to the inherent painted-scene body — the branch's
        // Win32 implementation, which master's seam stubbed out.
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
        // its content and there is nothing to scroll.
        //
        // The scaffold this replaced registered an `IdealystScroll` WNDCLASS
        // and stashed state in `GWLP_USERDATA`; the painted model scrolls by
        // offsetting the painter and `position_native_children`, so no
        // per-scroller window class is needed.
        WindowsBackend::create_scroll_view(self, horizontal, on_scroll, _a11y)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        WindowsBackend::set_node_scroll(self, node, x, y)
    }
}

impl caps::SafeAreaOps for WindowsBackend {}

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
        // row (creation-side work that needs the ambient world — the
        // flat_list-renders-zero-rows bug every backend shared).
        // item_count/item_key/item_size are pure reads, unwrapped.
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
        // Delegates to the inherent painted-scene body — the branch's
        // Win32 implementation, which master's seam stubbed out.
        WindowsBackend::create_virtualizer(self, callbacks, _overscan, _layout, _a11y)
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
        // Delegates to the inherent painted-scene body — the branch's
        // Win32 implementation, which master's seam stubbed out.
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
        // Delegates to the inherent painted-scene body — the branch's
        // Win32 implementation, which master's seam stubbed out.
        WindowsBackend::create_portal(self, _target, on_dismiss, _trap_focus, _a11y)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        WindowsBackend::set_portal_hidden(self, node, hidden)
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
        // the whole contract. (The old core routed here through a
        // backend-side `ExternalRegistry`, which died with
        // `Element::External`; the placeholder shape is unchanged.)
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
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        // Delegates to the inherent body, which translates the rules into
        // the Taffy style AND the painted visual (background, border,
        // radius, gradient, opacity, transform, overflow). The scaffold
        // this replaced dropped every style silently — the whole app
        // rendered unstyled.
        WindowsBackend::apply_style(self, node, style)
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

impl caps::IntrospectionOps for WindowsBackend {}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for WindowsBackend {}

impl caps::WireBindingOps for WindowsBackend {}

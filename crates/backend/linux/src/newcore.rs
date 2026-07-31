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
//! The host shell (out-of-repo; see the crate docs' test plan)
//! realizes + presents its `gtk::Window`, wraps it in
//! [`LinuxBackend::new`], and — if it has one — installs its
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
//! exclusively through signal closures the old `Backend` bodies
//! connect, and every one captures the closure handed in at
//! creation: `connect_clicked` (Button `Action::fire`),
//! `GestureClick::connect_released` (Pressable),
//! `connect_state_notify` (Toggle's `gtk::Switch`),
//! `connect_value_changed` (Slider's `gtk::Scale`), and the
//! ScrolledWindow h/v `Adjustment` `value-changed` signals
//! (`on_scroll`). Under new-core the closures handed in ARE the
//! caps-layer wrapped ones (wrapped BEFORE the UFCS delegation), so
//! every GTK signal dispatch schedules a flush with zero host changes.
//!
//! Text input / text area `on_change`/`on_key_down`/`on_blur` are
//! DROPPED by the scaffold today (never connected to the
//! `Entry`/`TextView` change signals); graphics / portal / virtualizer
//! are placeholders that fire nothing. All are wrapped uniformly
//! anyway so the flush arrives for free the day a placeholder becomes
//! a real widget.
//!
//! **Scheduler contract (honest statement).** The scaffold installs no
//! `runtime_shared::scheduling::Scheduler` and this repo has no Linux
//! host crate. Two regimes:
//!
//! - *No scheduler installed* (the scaffold's world today):
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
//! **No viewport source on this scaffold.** `finish()` reads
//! `host_window.width()/height()` directly for the Taffy pass, and
//! nothing writes `runtime_shared::set_viewport_size` on EITHER core, so
//! the world's viewport ctx keeps its default seed and old/new
//! behavior matches by construction. When a resize seam lands (window
//! `default-width`/`default-height` notify or surface layout signal)
//! it must write the old TLS value and a new-core sink side by side —
//! the terminal's `set_viewport` + `forward_viewport` pattern; the two
//! sinks must never diverge.
//!
//! # Residual seams (named, none silent)
//!
//! - `register_external` / `register_external_view` externals: author
//!   callbacks an External leaf wires natively (its own GTK signals)
//!   must call [`schedule_flush`] when those SDKs are ported.
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
use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;
use runtime_vocabulary::caps::ViewOps as _;
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

    let mut registry: Registry<LinuxBackend> = Registry::new();
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
        let Some(parent_layout) = self.layout_for_id.get(&parent.id).copied() else {
            return;
        };
        let Some(child_layout) = self.layout_for_id.get(&child.id).copied() else {
            return;
        };
        self.layout.add_child(parent_layout, child_layout);

        // GTK4 attach pattern: `gtk::Fixed::put(child, x, y)` adds
        // a child at absolute (x, y) within the container.
        // Initial coordinates are (0, 0); `finish()` walks every
        // registered widget and calls `fixed.move_()` once Taffy
        // has computed the real frame.
        //
        // For ScrolledWindow parents we route to the inner Fixed
        // installed by `create_scroll_view` — the outer
        // ScrolledWindow takes exactly one child (the scrollable
        // document), and that child is always our Fixed. Author-
        // supplied children mount inside the Fixed, NOT as a
        // sibling document replacing it.
        //
        // Leaf widgets (Button, Label, etc.) aren't containers —
        // author code shouldn't try to mount children inside
        // them; if it does, this call is a no-op rather than a
        // panic.
        if let Some(fixed) = parent.widget.downcast_ref::<gtk4::Fixed>() {
            fixed.put(&child.widget, 0.0, 0.0);
        } else if let Some(scrolled) = parent.widget.downcast_ref::<gtk4::ScrolledWindow>() {
            if let Some(inner) = scrolled
                .child()
                .and_then(|c| c.downcast::<gtk4::Fixed>().ok())
            {
                inner.put(&child.widget, 0.0, 0.0);
            }
        }
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
        // Walk + remove via the GTK4 `first_child`/`next_sibling`
        // iteration. Works for any `gtk::Widget` that has children;
        // the per-container removal API depends on the concrete
        // type (Fixed::remove, Box::remove, ScrolledWindow::
        // set_child(None)).
        if let Some(fixed) = node.widget.downcast_ref::<gtk4::Fixed>() {
            let mut child = fixed.first_child();
            while let Some(c) = child {
                let next = c.next_sibling();
                fixed.remove(&c);
                child = next;
            }
        } else if let Some(scrolled) = node.widget.downcast_ref::<gtk4::ScrolledWindow>() {
            // The inner document is our own `gtk::Fixed`. Clear
            // its children but keep the Fixed itself — author code
            // can still mount fresh children after a clear, and a
            // ScrolledWindow with no document widget would lose
            // its scrollbar slot machinery.
            if let Some(inner) = scrolled
                .child()
                .and_then(|c| c.downcast::<gtk4::Fixed>().ok())
            {
                let mut child = inner.first_child();
                while let Some(c) = child {
                    let next = c.next_sibling();
                    inner.remove(&c);
                    child = next;
                }
            }
        }
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
        // GTK4 exposes the system dark-mode preference via
        // `gtk::Settings::default().gtk_application_prefer_dark_theme`,
        // but the canonical signal is `gtk::StyleContext::settings`'s
        // `prefer_dark_theme` property combined with the system
        // freedesktop color-scheme setting. For the scaffold we
        // return Auto and let the framework's theme APIs decide.
        ColorScheme::Auto
    }

    fn platform(&self) -> Platform {
        Platform::Custom("linux")
    }
}

impl caps::LifecycleOps for LinuxBackend {
    fn finish(&mut self, root: Self::Node) {
        // First mount: attach the framework's root container to our
        // root `gtk::Fixed`. Only the first time — subsequent
        // `finish()` calls (re-render after data changes) keep the
        // same root attached, just re-position.
        if root.widget.parent().is_none() {
            self.root_fixed.put(&root.widget, 0.0, 0.0);
        }

        // Compute against the host window's allocated size. Before
        // the window is realized + presented, `width()`/`height()`
        // return 0; bail in that case so Taffy doesn't compute
        // against a degenerate viewport. The framework will call
        // `finish()` again after the first GTK allocate pass once
        // the window has real bounds.
        let width = self.host_window.width() as f32;
        let height = self.host_window.height() as f32;
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        let Some(root_layout) = self.layout_for_id.get(&root.id).copied() else {
            return;
        };
        self.layout.compute(root_layout, width, height);

        // Walk every registered widget and project its Taffy frame
        // into GTK's positioning surface. `set_size_request`
        // pins the widget's min size so GTK's own allocate pass
        // honors the Taffy width × height. `fixed.move_()` repositions
        // a child that's already attached to a Fixed parent.
        //
        // We split the walk into a collect-then-apply pass so the
        // GTK calls don't alias the borrow on `self.widgets` /
        // `self.layout_for_id`.
        let mut updates: Vec<(gtk4::Widget, f32, f32, i32, i32)> =
            Vec::with_capacity(self.widgets.len());
        for (id, widget) in &self.widgets {
            let Some(layout) = self.layout_for_id.get(id).copied() else {
                continue;
            };
            let frame = self.layout.frame_of(layout);
            updates.push((
                widget.clone(),
                frame.x,
                frame.y,
                frame.width.round() as i32,
                frame.height.round() as i32,
            ));
        }
        for (widget, x, y, w, h) in updates {
            widget.set_size_request(w, h);
            if let Some(parent) = widget.parent() {
                if let Some(fixed) = parent.downcast_ref::<gtk4::Fixed>() {
                    fixed.move_(&widget, x as f64, y as f64);
                }
                // Non-Fixed parents (Buttons accepting a Label
                // child, ScrolledWindow holding our inner Fixed)
                // don't have a coordinate concept — leave their
                // positioning to GTK's own allocate pass.
            }
        }
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for LinuxBackend {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        // gtk::Fixed — absolute-positioning container that takes
        // its children's (x, y) from our own `finish()` layout
        // pass. We deliberately don't use `gtk::Box` here because
        // Box's auto-stacking fights Taffy's frame assignments;
        // every container in the framework's flex tree needs to
        // be Fixed so finish() can write the computed position
        // directly via `fixed.move_()`.
        let widget = gtk4::Fixed::new();
        self.wrap(widget.upcast::<gtk4::Widget>())
    }
}

impl caps::InputOps for LinuxBackend {}

impl caps::PressableOps for LinuxBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> Self::Node {
        // Dispatch-site glue: the wrapped closure is what the old
        // `create_pressable` body moves into the `GestureClick`
        // `connect_released` signal closure — every GTK press
        // dispatch gets the flush for free.
        let on_click = flushing0(on_click);
        // Same container shape as `create_view` — a `gtk::Fixed`
        // so children land at Taffy-computed coordinates — with a
        // `GestureClick` controller mounted on top so the whole
        // surface reports a press. The framework's Pressable is
        // semantically a "transparent View that fires a callback"
        // and that's exactly what this gives us.
        let widget = gtk4::Fixed::new();
        let gesture = gtk4::GestureClick::new();
        let fire = on_click.clone();
        gesture.connect_released(move |_, _, _, _| (fire)());
        widget.add_controller(gesture);
        self.wrap(widget.upcast::<gtk4::Widget>())
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for LinuxBackend {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let label = gtk4::Label::new(Some(content));
        label.set_wrap(true);
        label.set_xalign(0.0);
        self.wrap(label.upcast::<gtk4::Widget>())
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        if let Some(label) = node.widget.downcast_ref::<gtk4::Label>() {
            label.set_text(content);
        }
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
        // Dispatch-site glue: wrap the Action's runtime evaluator
        // (the closure the old body hands to `connect_clicked`);
        // the serialization metadata passes through untouched.
        let on_click = Action {
            method: on_click.method,
            inputs: on_click.inputs.clone(),
            initial: on_click.initial.clone(),
            output: on_click.output,
            fire: flushing0(on_click.fire.clone()),
        };
        let on_click = &on_click;
        let button = gtk4::Button::with_label(label);
        let fire = on_click.fire.clone();
        button.connect_clicked(move |_| (fire)());
        self.wrap(button.upcast::<gtk4::Widget>())
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        if let Some(btn) = node.widget.downcast_ref::<gtk4::Button>() {
            btn.set_label(label);
        }
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for LinuxBackend {
    fn create_image(
        &mut self,
        _src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Image not yet implemented on Linux backend")
    }
}

impl caps::IconOps for LinuxBackend {
    fn create_icon(
        &mut self,
        _data: &runtime_shared::primitives::icon::IconData,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.placeholder("Icon not yet implemented on Linux backend")
    }
}

impl caps::LinkOps for LinuxBackend {}

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
        // Wrapped uniformly, though the scaffold does not yet wire
        // on_change/on_key_down/on_blur to the native editor (see
        // lib.rs) — when it does, the flush comes with it for free.
        let _on_change = flushing1(on_change);
        let _on_key_down = on_key_down.map(flushing_key);
        let _on_blur = on_blur.map(|f| -> primitives::text_input::BlurHandler {
                Rc::new(move || {
                    let outcome = f();
                    schedule_flush();
                    outcome
                })
            });
        // gtk::Entry is the canonical single-line text editor. Wire
        // initial value here; on_change wiring lands in the follow-up
        // PR alongside placeholder string + key handler routing.
        let entry = gtk4::Entry::new();
        entry.set_text(initial_value);
        // Password masking: GTK's Entry hides typed characters (shows
        // the invisible-char bullet) when visibility is off.
        if secure {
            entry.set_visibility(false);
        }
        self.wrap(entry.upcast::<gtk4::Widget>())
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        // GTK masks by hiding the entry's characters; `visibility = !secure`
        // toggles it in place on the same Entry.
        if let Some(entry) = node.widget.downcast_ref::<gtk4::Entry>() {
            entry.set_visibility(!secure);
        }
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
        let _on_change = flushing1(on_change);
        let _on_key_down = on_key_down.map(flushing_key);
        // gtk::TextView is the multi-line editor. Wrap in a
        // gtk::ScrolledWindow so long content scrolls naturally — a
        // bare TextView has no scrollbar.
        let view = gtk4::TextView::new();
        view.buffer().set_text(initial_value);
        let scrolled = gtk4::ScrolledWindow::new();
        scrolled.set_child(Some(&view));
        self.wrap(scrolled.upcast::<gtk4::Widget>())
    }
}

impl caps::ToggleOps for LinuxBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // The old body hands on_change to the Switch's
        // `connect_state_notify` closure — the wrap covers every
        // GTK state flip.
        let on_change = flushing1(on_change);
        let switch = gtk4::Switch::new();
        switch.set_active(initial_value);
        let fire = on_change.clone();
        switch.connect_state_notify(move |s| (fire)(s.is_active()));
        self.wrap(switch.upcast::<gtk4::Widget>())
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
        let on_change = flushing1(on_change);
        let scale = gtk4::Scale::with_range(
            gtk4::Orientation::Horizontal,
            min as f64,
            max as f64,
            // step_increment — GTK's keyboard step. Drag returns
            // continuous values regardless.
            1.0,
        );
        scale.set_value(initial_value as f64);
        let fire = on_change.clone();
        scale.connect_value_changed(move |s| (fire)(s.value() as f32));
        self.wrap(scale.upcast::<gtk4::Widget>())
    }
}

impl caps::ActivityIndicatorOps for LinuxBackend {
    fn create_activity_indicator(
        &mut self,
        _size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // gtk::Spinner is GTK's spinning loading indicator.
        let spinner = gtk4::Spinner::new();
        spinner.start();
        self.wrap(spinner.upcast::<gtk4::Widget>())
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
        // Dispatch-site glue: the old body attaches on_scroll to
        // both GTK Adjustments' `value-changed` signals; the flush
        // is deduped so a scroll burst costs one commit.
        let on_scroll = on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
            Rc::new(move |x, y| {
                f(x, y);
                schedule_flush();
            })
        });
        let scrolled = gtk4::ScrolledWindow::new();
        // Disable the axis the author didn't ask for. GTK's default
        // is "show scrollbars on both axes when needed".
        if horizontal {
            scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
        } else {
            scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        }
        // Inner document = `gtk::Fixed` so children mount via the
        // standard Fixed `put()`/`move_()` path. ScrolledWindow's
        // `set_child` takes exactly one widget; that one widget is
        // our document container. Author-supplied children attach
        // to the framework's logical ScrollView, which the host's
        // `insert` redirects to this inner Fixed via the
        // downcast-to-ScrolledWindow branch.
        let inner = gtk4::Fixed::new();
        scrolled.set_child(Some(&inner));

        // Wire `on_scroll` via the ScrolledWindow's adjustments.
        // GTK4 exposes one `gtk::Adjustment` per axis (`hadjustment` /
        // `vadjustment`); the `value-changed` signal fires whenever
        // the adjustment's `value` (the scroll offset, in widget
        // coordinates) changes \u{2014} touchpad scroll, scroll bar
        // drag, programmatic `set_value`, all of them.
        //
        // We connect to BOTH axes regardless of `horizontal` so the
        // callback observes the disabled axis too (it stays at 0
        // there, matching every other backend). The closure is
        // cloned per signal since GTK's connect API takes `Fn` by
        // ownership and we attach twice.
        if let Some(cb) = on_scroll {
            use gtk4::prelude::*;
            let cb_for_h = cb.clone();
            let scrolled_for_h = scrolled.clone();
            scrolled
                .hadjustment()
                .connect_value_changed(move |adj| {
                    let x = adj.value() as f32;
                    let y = scrolled_for_h.vadjustment().value() as f32;
                    cb_for_h(x, y);
                });
            let scrolled_for_v = scrolled.clone();
            scrolled
                .vadjustment()
                .connect_value_changed(move |adj| {
                    let x = scrolled_for_v.hadjustment().value() as f32;
                    let y = adj.value() as f32;
                    cb(x, y);
                });
        }

        self.wrap(scrolled.upcast::<gtk4::Widget>())
    }
}

impl caps::SafeAreaOps for LinuxBackend {}

impl caps::VirtualizerOps for LinuxBackend {
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
        let _callbacks = callbacks;
        self.placeholder("Virtualizer not yet implemented on Linux backend")
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
        // Dispatch-site glue: surface lifecycle callbacks run author
        // code (this scaffold never fires them — graphics is a
        // placeholder — but the wrap keeps the delegation
        // mechanically uniform).
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
        let _on_ready = on_ready;
        let _on_resize = on_resize;
        let _on_lost = on_lost;
        self.placeholder("Graphics not yet implemented on Linux backend")
    }
}

impl caps::PortalOps for LinuxBackend {
    fn create_portal(
        &mut self,
        _target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let on_dismiss = on_dismiss.map(flushing0);
        let _on_dismiss = on_dismiss;
        self.placeholder("Portal not yet implemented on Linux backend")
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
        // No-op until we wire Taffy-driven size_allocate in finish().
        // Author code calling apply_style today shouldn't crash; the
        // style is silently dropped.
    }
}

impl caps::AssetOps for LinuxBackend {}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for LinuxBackend {}

impl caps::AnimationOps for LinuxBackend {}

impl caps::IntrospectionOps for LinuxBackend {}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for LinuxBackend {}

impl caps::WireBindingOps for LinuxBackend {}

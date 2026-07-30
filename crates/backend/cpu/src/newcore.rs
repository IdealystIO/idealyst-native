//! Rendering: the `runtime_scene::Host` + capability-trait surface, the
//! boot entry, and the flush driver.
//!
//! [`CpuBackend`] implements [`runtime_scene::Host`] plus **all 30**
//! capability traits (`runtime_vocabulary::caps`) — the production shape
//! of the migration. Every mechanism body in this file was moved here
//! verbatim from the crate's old `impl runtime_core::Backend for CpuBackend`
//! when the 159-method mega-trait was deleted, so the rasterizer mechanism code (node allocation,
//! Taffy layout, hit-testing, `render`)
//! is unchanged: the same scene paints the same pixels
//! (pinned by `tests/newcore_parity.rs` against the frozen old-core
//! framebuffers).
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
//! The host constructs the backend, installs a scheduler if it has one
//! (the flush driver rides `schedule_microtask`), sets the viewport,
//! then calls [`start`]:
//!
//! 1. Monotonic time source (idempotent, first install wins).
//! 2. Registry: [`runtime_vocabulary::register_builtins`] + the app's
//!    `register` seam.
//! 3. Fresh [`World`]; build + [`realize`] inside `world.enter`;
//!    capture the per-world viewport ctx's size signal AFTER the build
//!    (the ctx's bucket memo pins the breakpoint table at creation —
//!    same ordering comment as backend-web/terminal).
//! 4. Entered buffered-microtask drain (no-op under a real scheduler;
//!    load-bearing under a buffering test scheduler).
//! 5. Single root → `caps::LifecycleOps::finish`; `world.flush()` commits
//!    anything staged during mount before the first paint.
//! 6. Install the flush driver and retain
//!    `{Realized, backend, registry, world}` in [`NewCoreApp`].
//!
//! # Flush driver — the dispatch-hook discipline
//!
//! The new core stages signal writes; nothing observes them until the
//! driver calls [`World::flush`]. Two delivery mechanisms, identical to
//! the terminal shape:
//!
//! 1. **Dispatch-site glue** (this module's caps impls). The
//!    **completeness argument** for this backend: `CpuBackend`'s ONLY
//!    author-callback dispatch surface is the closure
//!    [`CpuBackend::dispatch_click`] returns
//!    ([`crate::ClickOutcome::HandlerFired`] — the host fires it after
//!    releasing the backend borrow). That closure comes from exactly
//!    two places — `PressableOps::create_pressable`'s `on_click` and
//!    `ButtonOps::create_button`'s `Action.fire` — and both are wrapped
//!    here, so the closure the host fires IS the wrapped one and every
//!    interactive dispatch schedules a flush. All other callback-taking
//!    caps are wrapped uniformly per the template even though the
//!    backend currently drops them into text placeholders (the wrap
//!    keeps the delegation mechanically uniform and future-proof — the
//!    same rationale the terminal file gives for graphics callbacks it
//!    never fires).
//! 2. **Post-dispatch hook** ([`crate::dispatch_hook`]): author code
//!    that runs from a *scheduler* (`after_ms` debounces, `raf_loop`
//!    animation ticks) has no wrapped callback. The CPU backend owns NO
//!    scheduler — the host decides cadence — so there is no first-party
//!    fire site in this crate; a host that installs a runtime scheduler
//!    must fire the hook after each such callback, and headless hosts
//!    settle via [`flush_sync`] (the embedder contract in
//!    `dispatch_hook`'s module docs). [`start`] installs
//!    [`schedule_flush`] into the slot; no-op default, so the old core
//!    never pays.
//!
//! Everything funnels through [`schedule_flush`]/`flush_now`, which
//! skips re-entrant flushes (`world.is_flushing()`).
//!
//! # Viewport source
//!
//! [`CpuBackend::set_viewport`] is the one source of truth for the
//! pixel viewport (the host calls it on every resize). It already
//! writes the old-core TLS value (which seeds the per-world ctx at
//! creation); under `new-core` it ALSO forwards through
//! [`forward_viewport`] into the mounted world's viewport signal —
//! captured, not injected, so the push stages through the handle and
//! rides one deduped [`schedule_flush`] (the backend-web
//! resize-listener discipline). Pixel counts are pushed, matching the
//! old TLS write — the CPU backend's logical px is one framebuffer
//! pixel.
//!
//! # Residual seams (named, none silent)
//!
//! - The old-core `NavigatorRegistry`/inventory registrars keep serving
//!   the old path only; new-core navigators are vocabulary built-ins
//!   (swap/stack), so `Element::Navigator` with an SDK presentation
//!   type routes through `Backend::create_navigator` exactly as before
//!   (which, on this backend, renders the visible placeholder).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::animation::AnimProp;
use runtime_shared::primitives;
use runtime_shared::{
    Action, ColorScheme, Platform, StyleRules,
    VirtualizerCallbacks,
};
use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;
use crate::node::{NodeKind, ResolvedGradient};
use runtime_shared::primitives::icon::IconData;
use runtime_vocabulary::caps::ViewOps as _;
use runtime_shared::color::{parse_or, Rgba};
use runtime_shared::Length;

use crate::{CpuBackend, CpuNode};

// Re-exported so host shells and app wrappers can name the boot-path
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
    static BACKEND: RefCell<Option<Weak<RefCell<CpuBackend>>>> = const { RefCell::new(None) };
}

/// Everything the boot path must keep alive. Field order is drop order:
/// the realized tree unmounts before the world (its slots' owner) dies.
/// The host loop holds this value for the whole session and calls
/// [`NewCoreApp::stop`] on quit — the reactive teardown must run while
/// the thread's TLS is intact (same teardown-ordering care the old host
/// takes with its `Owner`).
pub struct NewCoreApp {
    realized: Realized<CpuNode>,
    _backend: Rc<RefCell<CpuBackend>>,
    _registry: Rc<Registry<CpuBackend>>,
    world: World,
}

impl NewCoreApp {
    /// Borrow the live tree (tests, diagnostics).
    pub fn with_realized<R>(&self, f: impl FnOnce(&Realized<CpuNode>) -> R) -> R {
        f(&self.realized)
    }

    /// The mounted world (tests can flush it explicitly).
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Unmount: drops the `Realized` (cleanups fire), uninstalls the
    /// flush driver + viewport sink, and drops the world. Call BEFORE
    /// tearing down whatever the host's surface lives on — the reactive
    /// teardown must run while the thread's TLS is intact.
    pub fn stop(self) {
        crate::dispatch_hook::clear_dispatch_hook();
        set_viewport_sink(None);
        set_flush_world(None);
        BACKEND.with(|b| *b.borrow_mut() = None);
        drop(self);
    }
}

/// Mount a new-core element tree into an already-constructed backend.
///
/// The host must have: constructed the backend, installed a scheduler
/// if it drives one (the flush driver rides `schedule_microtask`;
/// headless harnesses install a queue scheduler and drain it
/// explicitly), and applied the viewport size. Unlike the terminal
/// backend there is no global self-handle to install — `CpuBackend`'s
/// dispatch surface is `dispatch_click`, which the host calls on the
/// `Rc` it already owns.
///
/// `register` runs after [`runtime_vocabulary::register_builtins`], so
/// apps/SDKs can register their own payload handlers on the same
/// registry before the tree realizes. The build closure runs inside
/// `world.enter`, so free `signal()`/`effect()` calls work; top-level
/// creations are world-root-owned (they live until [`NewCoreApp::stop`]).
pub fn start(
    backend: Rc<RefCell<CpuBackend>>,
    register: impl FnOnce(&mut Registry<CpuBackend>),
    build: impl FnOnce() -> Element,
) -> NewCoreApp {
    // Monotonic clock (idempotent, first install wins) — animation and
    // presence timing read it; the old boot relied on the host's lazy
    // default, the new boot installs it explicitly like macOS/terminal.
    let platform = caps::AppEnvOps::platform(&*backend.borrow());
    runtime_shared::time::install_default_time_source(platform);

    let mut registry: Registry<CpuBackend> = Registry::new();
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
        // component (backend-web's identical ordering comment).
        let vp_sig = runtime_vocabulary::viewport::viewport_ctx().size_signal();
        (vp_sig, realized)
    });

    // Buffered-microtask drain — a no-op under a real scheduler,
    // load-bearing under a buffering test scheduler. Must run with NO
    // backend borrow held (drained tasks re-borrow); ENTERED because a
    // buffered task may do creation-side work.
    world.enter(runtime_shared::scheduling::drain_buffered_microtasks);

    // Single-root contract, matching the old-core mount (`find_root`
    // wants exactly one application root — id 1).
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_cpu::newcore::start: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    caps::LifecycleOps::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount before the first paint.
    world.flush();

    // Install the flush driver: schedule_flush becomes reachable from
    // (a) the author-callback wrappers in the caps impls below and
    // (b) a host scheduler's post-dispatch hook (if the host has one).
    crate::dispatch_hook::install_dispatch_hook(schedule_flush);
    set_flush_world(Some(world.clone()));
    // Live viewport source: `CpuBackend::set_viewport` now reaches the
    // world's ctx through `forward_viewport`.
    set_viewport_sink(Some(vp_sig));
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
pub fn with_backend<R>(f: impl FnOnce(&Rc<RefCell<CpuBackend>>) -> R) -> Option<R> {
    let rc = BACKEND.with(|b| b.borrow().as_ref().and_then(Weak::upgrade));
    rc.map(|rc| f(&rc))
}

// ===========================================================================
// Flush driver
// ===========================================================================

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// The author-callback wrappers and a host scheduler's dispatch hook
/// call this right after author-visible dispatch; the host's per-frame
/// loop drains microtasks before painting.
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
/// [`start`]). Harness seam: a headless host that staged writes and
/// must observe the committed framebuffer before returning (tests, a
/// one-shot render harness) cannot ride the async microtask — it
/// flushes before returning instead. This is the settle path the
/// dispatch-hook embedder contract names for scheduler-less hosts.
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
/// ambient (`World::enter`). Same rationale as the terminal/web glue:
/// virtualizer `mount_item`/`release_item` REALIZE/tear down a row —
/// creation-side work that needs the ambient world. The CPU backend's
/// virtualizer is a text placeholder today, but the wrap keeps the
/// delegation uniform (see the module docs' completeness argument).
/// Pre-boot the boot's own `enter` is still ambient, so the bare-call
/// fallback never double-books.
fn enter_mounted_world<R>(f: impl FnOnce() -> R) -> R {
    match FLUSH_WORLD.with(|w| w.borrow().clone()) {
        Some(world) => world.enter(f),
        None => f(),
    }
}

// ===========================================================================
// Viewport source (the new-core CPU viewport seam)
// ===========================================================================

thread_local! {
    /// The mounted world's viewport signal (`Copy` handle). `None`
    /// outside a new-core boot, so `set_viewport` costs one TLS read
    /// and nothing else on the old core.
    static VIEWPORT_SINK: Cell<Option<runtime_world::Signal<runtime_shared::ViewportSize>>> =
        const { Cell::new(None) };
}

fn set_viewport_sink(sig: Option<runtime_world::Signal<runtime_shared::ViewportSize>>) {
    VIEWPORT_SINK.with(|s| s.set(sig));
}

/// Forward one viewport report (in PIXELS, the CPU backend's logical
/// px — same value the old TLS write carries) into the mounted world's
/// viewport ctx. No-op before [`start`] / after teardown. Called by
/// [`CpuBackend::set_viewport`] right beside the old-core TLS write —
/// the two sinks must never diverge.
pub(crate) fn forward_viewport(size: runtime_shared::ViewportSize) {
    let Some(sig) = VIEWPORT_SINK.with(|s| s.get()) else {
        return;
    };
    // Staged write outside `enter` (handle-routed, equality-guarded)
    // + one deduped flush — commits on the next microtask drain, like
    // every wrapped callback.
    sig.set(size);
    schedule_flush();
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
// backend_terminal/src/newcore.rs / runtime_vocabulary::bridge — keep
// mechanically in sync; the AllCaps bound on register_builtins is the
// compile gate)
// ===========================================================================

// ---------------------------------------------------------------------------
// Host — the P1 structural seam
// ---------------------------------------------------------------------------

impl Host for CpuBackend {
    type Node = CpuNode;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let Some(parent_layout) = self.nodes.get(&parent.id).map(|d| d.layout) else { return };
        let Some(child_layout) = self.nodes.get(&child.id).map(|d| d.layout) else { return };
        self.layout.add_child(parent_layout, child_layout);
        if let Some(parent_data) = self.nodes.get_mut(&parent.id) {
            parent_data.children.push(child.id);
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
        let Some(child_ids) = self.nodes.get(&node.id).map(|d| d.children.clone()) else { return };
        for child_id in &child_ids {
            self.remove_subtree(*child_id);
        }
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.children.clear();
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

impl caps::AppEnvOps for CpuBackend {
    fn color_scheme(&self) -> ColorScheme {
        // The CPU backend has no host preference of its own; the
        // application's theme is the source of truth. Authors that
        // care can override via the framework's theme APIs.
        ColorScheme::Auto
    }

    fn platform(&self) -> Platform {
        // `Custom("cpu")` documents the renderer kind without
        // collapsing it into one of the named native platforms.
        // Author code that branches on `Platform::Custom("cpu")`
        // can opt into pixel-art / lower-density chrome.
        Platform::Custom("cpu")
    }
}

impl caps::LifecycleOps for CpuBackend {
    fn finish(&mut self, _root: Self::Node) {
        // Nothing to do — the host calls `render` when it wants a
        // frame. Unlike a windowed backend, we don't drive paints
        // on a vsync; the host decides cadence.
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for CpuBackend {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        self.alloc_node(NodeKind::View, String::new())
    }
}

impl caps::InputOps for CpuBackend {}

impl caps::PressableOps for CpuBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> Self::Node {
        // Dispatch-site glue: the wrapped closure lands in the node's
        // `on_click` slot and is exactly what `dispatch_click` hands
        // back in `ClickOutcome::HandlerFired`, so the host's plain
        // `h()` call gets the flush for free (the completeness
        // argument in the module docs).
        let on_click = flushing0(on_click);
        let node = self.alloc_node(NodeKind::Pressable, String::new());
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.on_click = Some(on_click);
        }
        node
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for CpuBackend {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        self.alloc_node(NodeKind::Text, content.to_string())
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        if let Some(data) = self.nodes.get_mut(&node.id) {
            if data.content != content {
                data.content = content.to_string();
            }
        }
    }
}

impl caps::ButtonOps for CpuBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&primitives::icon::IconData>,
        _trailing_icon: Option<&primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: wrap the Action's runtime evaluator (the
        // closure `dispatch_click` returns — `create_button` stores
        // `Action.fire` in the node's `on_click` slot); the
        // serialization metadata passes through untouched.
        let on_click = Action {
            method: on_click.method,
            inputs: on_click.inputs.clone(),
            initial: on_click.initial.clone(),
            output: on_click.output,
            fire: flushing0(on_click.fire.clone()),
        };
        let on_click = &on_click;
        let node = self.alloc_node(NodeKind::Button, label.to_string());
        let handler = on_click.fire.clone();
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.on_click = Some(handler);
        }
        node
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for CpuBackend {
    fn create_image(
        &mut self,
        _src: &str,
        _alt: Option<&str>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.alloc_node(
            NodeKind::Text,
            "Image not supported on CPU backend".to_string(),
        )
    }
}

impl caps::IconOps for CpuBackend {
    fn create_icon(
        &mut self,
        _data: &IconData,
        _color: Option<&runtime_shared::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.alloc_node(
            NodeKind::Text,
            "Icon not supported on CPU backend".to_string(),
        )
    }
}

impl caps::LinkOps for CpuBackend {}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for CpuBackend {
    fn create_text_input(
        &mut self,
        _initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        _secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // The CPU backend renders a placeholder (no key dispatch), but
        // the wrap keeps the delegation mechanically uniform.
        let _placeholder = placeholder;
        let _on_change = flushing1(on_change);
        let _on_key_down = on_key_down.map(flushing_key);
        let _on_blur = on_blur.map(|f| -> primitives::text_input::BlurHandler {
                Rc::new(move || {
                    let outcome = f();
                    schedule_flush();
                    outcome
                })
            });
        self.alloc_node(
            NodeKind::Text,
            "TextInput not supported on CPU backend".to_string(),
        )
    }

    fn create_text_area(
        &mut self,
        _initial_value: &str,
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
        self.alloc_node(
            NodeKind::Text,
            "TextArea not supported on CPU backend".to_string(),
        )
    }
}

impl caps::ToggleOps for CpuBackend {
    fn create_toggle(
        &mut self,
        _initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let _on_change = flushing1(on_change);
        self.alloc_node(
            NodeKind::Text,
            "Toggle not supported on CPU backend".to_string(),
        )
    }
}

impl caps::SliderOps for CpuBackend {
    fn create_slider(
        &mut self,
        _initial_value: f32,
        _min: f32,
        _max: f32,
        _step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let _on_change = flushing1(on_change);
        self.alloc_node(
            NodeKind::Text,
            "Slider not supported on CPU backend".to_string(),
        )
    }
}

impl caps::ActivityIndicatorOps for CpuBackend {
    fn create_activity_indicator(
        &mut self,
        _size: runtime_shared::primitives::activity_indicator::ActivityIndicatorSize,
        _color: Option<&runtime_shared::Color>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.alloc_node(
            NodeKind::Text,
            "ActivityIndicator not supported on CPU backend".to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for CpuBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: the CPU backend drops `on_scroll` today
        // (no wheel/touch scroll dispatch), but the wrap keeps the
        // delegation mechanically uniform; the flush microtask is
        // deduped so a future burst costs one commit.
        let on_scroll = on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
            Rc::new(move |x, y| {
                f(x, y);
                schedule_flush();
            })
        });
        let _on_scroll = on_scroll;
        let node = self.alloc_node(NodeKind::ScrollView, String::new());
        if let Some(data) = self.nodes.get_mut(&node.id) {
            // `horizontal` flag lives on the existing
            // `scroll_x` / `scroll_y` pair: we just remember which
            // axis to honor in dispatch. For the MVP we honor both
            // simultaneously regardless of the flag; surface a real
            // axis lock once we add wheel/touch scroll.
            let _ = horizontal;
            // Pin children inside our box.
            data.scroll_x = 0.0;
            data.scroll_y = 0.0;
        }
        node
    }
}

impl caps::SafeAreaOps for CpuBackend {}

impl caps::VirtualizerOps for CpuBackend {
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
        // flat_list-renders-zero-rows bug every backend shared). The
        // CPU backend's virtualizer is a text placeholder that never
        // fires these, but the wrap keeps the delegation uniform.
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
        self.alloc_node(
            NodeKind::Text,
            "Virtualizer not supported on CPU backend".to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for CpuBackend {
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: surface lifecycle callbacks run author
        // code (the CPU backend never fires them — placeholder node —
        // but the wrap keeps the delegation mechanically uniform).
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
        self.alloc_node(
            NodeKind::Text,
            "Graphics not supported on CPU backend".to_string(),
        )
    }
}

impl caps::PortalOps for CpuBackend {
    fn create_portal(
        &mut self,
        _target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let on_dismiss = on_dismiss.map(flushing0);
        let _on_dismiss = on_dismiss;
        self.alloc_node(
            NodeKind::Text,
            "Portal not supported on CPU backend".to_string(),
        )
    }
}

impl caps::PresenceOps for CpuBackend {}

impl caps::NavigatorOps for CpuBackend {}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for CpuBackend {
    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &Rc<dyn std::any::Any>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.alloc_node(
            NodeKind::Text,
            format!("External \"{type_name}\" not supported on CPU backend"),
        )
    }
}

impl caps::DocumentOps for CpuBackend {}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for CpuBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        let Some(layout_node) = self.nodes.get(&node.id).map(|d| d.layout) else { return };

        // Eagerly resolve `background` and `color` BEFORE handing the
        // rules to `runtime-layout`'s `set_style`. Same ordering
        // constraint the terminal backend documents — the cohort
        // driver Effect re-fires on token-signal changes, and the
        // resolve must happen before other style processing so the
        // per-token edges land in this Effect's dependency set on
        // the first re-fire. Without it, theme toggles update on
        // the second toggle, not the first.
        let _ = style.background.as_ref().map(|t| t.resolve());
        let _ = style.color.as_ref().map(|t| t.resolve());
        self.layout.set_style(layout_node, style);

        let fg = style
            .color
            .as_ref()
            .map(|t| parse_or(&t.resolve().0, Rgba::default()));
        let bg = style
            .background
            .as_ref()
            .map(|t| parse_or(&t.resolve().0, Rgba::TRANSPARENT));
        let opacity = style
            .opacity
            .as_ref()
            .map(|t| t.resolve().clamp(0.0, 1.0));

        // Borders.
        let bw = [
            style.border_top_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
            style.border_right_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
            style.border_bottom_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
            style.border_left_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0),
        ];
        let bc = [
            style
                .border_top_color
                .as_ref()
                .map(|t| parse_or(&t.resolve().0, Rgba::BLACK)),
            style
                .border_right_color
                .as_ref()
                .map(|t| parse_or(&t.resolve().0, Rgba::BLACK)),
            style
                .border_bottom_color
                .as_ref()
                .map(|t| parse_or(&t.resolve().0, Rgba::BLACK)),
            style
                .border_left_color
                .as_ref()
                .map(|t| parse_or(&t.resolve().0, Rgba::BLACK)),
        ];

        // Corner radii. We only honor Px units — Percent radii would
        // need the node's own frame size, which we don't have until
        // layout has run. ESP32-class targets shouldn't use percent
        // radii anyway (they're a CSS convenience, not load-bearing).
        let radius_px = |t: &runtime_shared::Tokenized<Length>| -> f32 {
            match t.resolve() {
                Length::Px(v) => v,
                _ => 0.0,
            }
        };
        let radii = [
            style.border_top_left_radius.as_ref().map(radius_px).unwrap_or(0.0),
            style.border_top_right_radius.as_ref().map(radius_px).unwrap_or(0.0),
            style.border_bottom_right_radius.as_ref().map(radius_px).unwrap_or(0.0),
            style.border_bottom_left_radius.as_ref().map(radius_px).unwrap_or(0.0),
        ];

        // Font size (Px-only; same rationale as radii).
        let font_size_px = style.font_size.as_ref().and_then(|t| match t.resolve() {
            Length::Px(v) => Some(v),
            _ => None,
        });

        // Gradient resolution. Stops are pre-parsed to Rgba so the
        // per-pixel sampler in `paint_node` doesn't reparse strings
        // on every paint.
        let gradient = style.background_gradient.as_ref().map(|g| {
            let stops: Vec<(f32, Rgba)> = g
                .stops
                .iter()
                .map(|s| (s.offset, parse_or(&s.color.0, Rgba::TRANSPARENT)))
                .collect();
            let animated_stops = vec![None; stops.len()];
            ResolvedGradient { kind: g.kind.clone(), stops, animated_stops }
        });

        // Static transform — TranslateX/Y only on the CPU backend.
        // Scale / Rotate would force a per-pixel inverse transform
        // (expensive without SIMD); skip for now and log a warning
        // via the debug build assertion below.
        let mut static_tx: Option<Length> = None;
        let mut static_ty: Option<Length> = None;
        if let Some(transforms) = style.transform.as_ref() {
            for t in transforms {
                match t {
                    runtime_shared::Transform::TranslateX(l) => static_tx = Some(*l),
                    runtime_shared::Transform::TranslateY(l) => static_ty = Some(*l),
                    _ => {
                        // Silently drop. Surface a real diagnostic
                        // once we have a logger wired into the
                        // backend; `println!` is the wrong shape
                        // here (won't reach the ESP32 host).
                    }
                }
            }
        }

        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.style = Some(style.clone());
            data.fg = fg;
            data.bg = bg;
            if let Some(o) = opacity {
                data.opacity = o;
            }
            data.border_widths = bw;
            data.border_colors = bc;
            data.corner_radii = radii;
            data.font_size_px = font_size_px;
            data.static_translate_x = static_tx;
            data.static_translate_y = static_ty;
            // Preserve animated stops across stylesheet re-apply when
            // the gradient's shape (stop count) matches — re-applying
            // a stylesheet (state overlay, theme refresh, hot patch)
            // shouldn't reset in-flight per-stop animations. The
            // terminal backend documents the same rule.
            let preserved = data
                .gradient
                .as_ref()
                .and_then(|old| {
                    gradient.as_ref().map(|new| {
                        if new.stops.len() == old.stops.len() {
                            old.animated_stops.clone()
                        } else {
                            vec![None; new.stops.len()]
                        }
                    })
                });
            data.gradient = gradient.map(|mut g| {
                if let Some(p) = preserved {
                    g.animated_stops = p;
                }
                g
            });
        }
    }
}

impl caps::AssetOps for CpuBackend {}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for CpuBackend {}

impl caps::AnimationOps for CpuBackend {
    /// Per-frame scalar-property write — opacity, translate, z-index.
    /// Scale / Rotate fall through to a no-op for now; implementing
    /// them correctly on a software rasterizer needs an inverse
    /// transform on every pixel of the affected subtree, which is
    /// the wrong cost to pay on an ESP32-class target. We log via
    /// debug-assertion so authors notice when they hit the gap.
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        let Some(data) = self.nodes.get_mut(&node.id) else { return };
        match prop {
            AnimProp::Opacity => {
                data.animated_opacity = Some(value.clamp(0.0, 1.0));
            }
            AnimProp::TranslateX => {
                data.animated_translate_x = value;
            }
            AnimProp::TranslateY => {
                data.animated_translate_y = value;
            }
            AnimProp::ZIndex => {
                data.z_index = value;
            }
            // Scale / ScaleX / ScaleY / RotateZ — not supported by
            // the axis-aligned rasterizer. Silently drop; documented
            // in `README.md`. (debug_assert! would crash tests that
            // exercise composite trees containing both supported and
            // unsupported animations.)
            _ => {}
        }
    }

    /// Per-frame color-property write — animated background,
    /// foreground, or gradient stop.
    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        let Some(data) = self.nodes.get_mut(&node.id) else { return };
        let rgba = Rgba::from_srgb_f32(value);
        match prop {
            AnimProp::BackgroundColor => {
                data.animated_bg = Some(rgba);
            }
            AnimProp::ForegroundColor => {
                data.animated_fg = Some(rgba);
            }
            AnimProp::GradientStopColor(idx) => {
                if let Some(g) = data.gradient.as_mut() {
                    let i = idx as usize;
                    if i < g.animated_stops.len() {
                        g.animated_stops[i] = Some(rgba);
                    }
                }
            }
            _ => {}
        }
    }
}

impl caps::IntrospectionOps for CpuBackend {}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for CpuBackend {}

impl caps::WireBindingOps for CpuBackend {}

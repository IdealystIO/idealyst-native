//! New-core adoption for the CPU-rasterizer backend (idea-lite
//! migration: host-driven cadence, the terminal shape applied to the
//! pixel framebuffer).
//!
//! Implements [`runtime_scene::Host`] plus **all 30** capability traits
//! (`runtime_vocabulary::caps`) directly on [`CpuBackend`] — the
//! production shape of the migration (no `LegacyBridge` in the render
//! path). Every trait method delegates via UFCS
//! (`<CpuBackend as Backend>::method(self, …)`) to the existing
//! `Backend` impl, so the rasterizer mechanism code (node allocation,
//! Taffy layout, hit-testing, [`CpuBackend::render`]) is REUSED
//! verbatim: the same scene renders to the same pixels on both cores
//! (pinned by `tests/newcore_parity.rs`). `CpuBackend` overrides only a
//! SUBSET of `Backend` (view / text / button / pressable / scroll_view
//! plus visible-placeholder `create_*` for the rest); where a method is
//! not overridden, the UFCS call resolves to the same trait-default the
//! old walker hits — behavior identical by construction, which is why
//! all 30 caps delegate uniformly rather than special-casing the
//! placeholder set. **30/30 direct, 0 adapted, 0 stubbed.**
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
//! 5. Single root → `Backend::finish`; `world.flush()` commits
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
    let platform = Backend::platform(&*backend.borrow());
    runtime_core::time::install_default_time_source(platform);

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
    world.enter(runtime_core::scheduling::drain_buffered_microtasks);

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
    Backend::finish(&mut *backend.borrow_mut(), root);

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
    runtime_core::scheduling::schedule_microtask(|| {
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
    static VIEWPORT_SINK: Cell<Option<runtime_world::Signal<runtime_core::ViewportSize>>> =
        const { Cell::new(None) };
}

fn set_viewport_sink(sig: Option<runtime_world::Signal<runtime_core::ViewportSize>>) {
    VIEWPORT_SINK.with(|s| s.set(sig));
}

/// Forward one viewport report (in PIXELS, the CPU backend's logical
/// px — same value the old TLS write carries) into the mounted world's
/// viewport ctx. No-op before [`start`] / after teardown. Called by
/// [`CpuBackend::set_viewport`] right beside the old-core TLS write —
/// the two sinks must never diverge.
pub(crate) fn forward_viewport(size: runtime_core::ViewportSize) {
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
        <CpuBackend as Backend>::insert(self, parent, child)
    }

    fn insert_many(&mut self, parent: &mut Self::Node, children: Vec<Self::Node>) {
        <CpuBackend as Backend>::insert_many(self, parent, children)
    }

    fn insert_at(&mut self, parent: &mut Self::Node, child: Self::Node, index: usize) {
        <CpuBackend as Backend>::insert_at(self, parent, child, index)
    }

    fn remove_child(&mut self, parent: &Self::Node, child: &Self::Node) {
        <CpuBackend as Backend>::remove_child(self, parent, child)
    }

    fn clear_children(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::clear_children(self, node)
    }

    fn create_anchor(&mut self) -> Self::Node {
        <CpuBackend as Backend>::create_reactive_anchor(self)
    }

    fn supports_splice(&self) -> bool {
        <CpuBackend as Backend>::supports_child_splice(self)
    }
}

// ---------------------------------------------------------------------------
// App environment + lifecycle
// ---------------------------------------------------------------------------

impl caps::AppEnvOps for CpuBackend {
    fn color_scheme(&self) -> ColorScheme {
        <CpuBackend as Backend>::color_scheme(self)
    }

    fn platform(&self) -> Platform {
        <CpuBackend as Backend>::platform(self)
    }

    fn url_opener(&self) -> Option<Rc<dyn Fn(&str)>> {
        <CpuBackend as Backend>::url_opener(self)
    }

    fn fullscreen_setter(&self) -> Option<Rc<dyn Fn(bool)>> {
        <CpuBackend as Backend>::fullscreen_setter(self)
    }

    fn set_page_metadata(&mut self, meta: &PageMetadata) {
        <CpuBackend as Backend>::set_page_metadata(self, meta)
    }

    fn set_app_background(&mut self, color: &Tokenized<Color>) {
        <CpuBackend as Backend>::set_app_background(self, color)
    }

    fn set_scrollbar_theme(&mut self, thumb: &Tokenized<Color>, track: &Tokenized<Color>) {
        <CpuBackend as Backend>::set_scrollbar_theme(self, thumb, track)
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        // Dispatch-site glue: the app-level key handler runs author
        // code. The CPU backend never dispatches keys today (trait
        // default drops the handler), but the wrap keeps the
        // delegation mechanically uniform.
        let handler = handler.map(flushing_key);
        <CpuBackend as Backend>::set_app_key_handler(self, handler)
    }
}

impl caps::LifecycleOps for CpuBackend {
    fn finish(&mut self, root: Self::Node) {
        <CpuBackend as Backend>::finish(self, root)
    }

    fn run_layout(&mut self) {
        <CpuBackend as Backend>::run_layout(self)
    }

    fn schedule_layout_pass() {
        <CpuBackend as Backend>::schedule_layout_pass()
    }

    fn is_hydrating(&self) -> bool {
        <CpuBackend as Backend>::is_hydrating(self)
    }

    fn renders_lazy_chunks(&self) -> bool {
        <CpuBackend as Backend>::renders_lazy_chunks(self)
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for CpuBackend {
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        <CpuBackend as Backend>::create_view(self, a11y)
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_core::ViewHandle {
        <CpuBackend as Backend>::make_view_handle(self, node)
    }
}

impl caps::InputOps for CpuBackend {
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
        <CpuBackend as Backend>::install_touch_handler(self, node, handler)
    }

    fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
        <CpuBackend as Backend>::claim_touch(self, node, touch_id)
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
        <CpuBackend as Backend>::install_wheel_handler(self, node, handler)
    }

    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        <CpuBackend as Backend>::install_hover_handler(self, node, flushing1(handler))
    }

    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::mark_preserves_focus(self, node)
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
        <CpuBackend as Backend>::install_file_drop_handler(self, node, handler)
    }
}

impl caps::PressableOps for CpuBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        // Dispatch-site glue: the wrapped closure lands in the node's
        // `on_click` slot and is exactly what `dispatch_click` hands
        // back in `ClickOutcome::HandlerFired`, so the host's plain
        // `h()` call gets the flush for free (the completeness
        // argument in the module docs).
        <CpuBackend as Backend>::create_pressable(self, flushing0(on_click), a11y)
    }

    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_core::PressableHandle {
        <CpuBackend as Backend>::make_pressable_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for CpuBackend {
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node {
        <CpuBackend as Backend>::create_text(self, content, a11y)
    }

    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
        <CpuBackend as Backend>::create_styled_text(self, runs, a11y)
    }

    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
        <CpuBackend as Backend>::update_styled_text(self, node, runs)
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        <CpuBackend as Backend>::update_text(self, node, content)
    }

    fn create_text_with_id(
        &mut self,
        content: &str,
        a11y: &AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
        <CpuBackend as Backend>::create_text_with_id(self, content, a11y)
    }

    fn update_text_by_id(&mut self, id: u32, content: String) {
        <CpuBackend as Backend>::update_text_by_id(self, id, content)
    }

    fn release_text_id(&mut self, id: u32) {
        <CpuBackend as Backend>::release_text_id(self, id)
    }

    fn supports_js_text_bindings(&self) -> bool {
        <CpuBackend as Backend>::supports_js_text_bindings(self)
    }

    fn register_reactive_text_binding(
        &mut self,
        text_id: u32,
        signal_ids: &[u64],
        template_parts: &[&str],
        initial_values: &[&str],
        stringifiers: &[Rc<dyn Fn() -> String>],
    ) {
        <CpuBackend as Backend>::register_reactive_text_binding(
            self,
            text_id,
            signal_ids,
            template_parts,
            initial_values,
            stringifiers,
        )
    }

    fn release_reactive_text_binding(&mut self, text_id: u32) {
        <CpuBackend as Backend>::release_reactive_text_binding(self, text_id)
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_core::TextHandle {
        <CpuBackend as Backend>::make_text_handle(self, node)
    }
}

impl caps::ButtonOps for CpuBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        a11y: &AccessibilityProps,
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
        <CpuBackend as Backend>::create_button(
            self,
            label,
            &on_click,
            leading_icon,
            trailing_icon,
            a11y,
        )
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        <CpuBackend as Backend>::update_button_label(self, node, label)
    }

    fn make_button_handle(&self, node: &Self::Node) -> runtime_core::ButtonHandle {
        <CpuBackend as Backend>::make_button_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for CpuBackend {
    fn create_image(&mut self, src: &str, alt: Option<&str>, a11y: &AccessibilityProps) -> Self::Node {
        <CpuBackend as Backend>::create_image(self, src, alt, a11y)
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        <CpuBackend as Backend>::update_image_src(self, node, src)
    }

    fn update_image_alt(&mut self, node: &Self::Node, alt: Option<&str>) {
        <CpuBackend as Backend>::update_image_alt(self, node, alt)
    }

    fn install_image_load_handler(&mut self, node: &Self::Node, handler: ImageLoadHandler) {
        let handler: ImageLoadHandler = {
            let f = handler;
            Rc::new(move |ev| {
                f(ev);
                schedule_flush();
            })
        };
        <CpuBackend as Backend>::install_image_load_handler(self, node, handler)
    }

    fn install_image_error_handler(&mut self, node: &Self::Node, handler: ImageErrorHandler) {
        <CpuBackend as Backend>::install_image_error_handler(self, node, flushing0(handler))
    }

    fn make_image_handle(&self, node: &Self::Node) -> primitives::image::ImageHandle {
        <CpuBackend as Backend>::make_image_handle(self, node)
    }
}

impl caps::IconOps for CpuBackend {
    fn create_icon(
        &mut self,
        data: &primitives::icon::IconData,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <CpuBackend as Backend>::create_icon(self, data, color, a11y)
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        <CpuBackend as Backend>::update_icon_color(self, node, color)
    }

    fn update_icon_data(&mut self, node: &Self::Node, data: &primitives::icon::IconData) {
        <CpuBackend as Backend>::update_icon_data(self, node, data)
    }

    fn update_icon_stroke(&mut self, node: &Self::Node, progress: f32) {
        <CpuBackend as Backend>::update_icon_stroke(self, node, progress)
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
        <CpuBackend as Backend>::animate_icon_stroke(
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
        <CpuBackend as Backend>::make_icon_handle(self, node)
    }
}

impl caps::LinkOps for CpuBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: link activation dispatches navigation
        // (stages nav-queue tick signals on the new core). The trait
        // default renders links via `create_pressable` on backends
        // without a native link, so the wrapped closure lands in an
        // on_click slot and nav-link clicks flush exactly like
        // pressables.
        let mut config = config;
        config.on_activate = flushing0(config.on_activate.clone());
        <CpuBackend as Backend>::create_link(self, config, a11y)
    }

    fn update_link_url(&mut self, node: &Self::Node, url: &str) {
        <CpuBackend as Backend>::update_link_url(self, node, url)
    }

    fn make_link_handle(&self, node: &Self::Node) -> primitives::link::LinkHandle {
        <CpuBackend as Backend>::make_link_handle(self, node)
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for CpuBackend {
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
        // The CPU backend renders a placeholder (no key dispatch), but
        // the wrap keeps the delegation mechanically uniform.
        <CpuBackend as Backend>::create_text_input(
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
        <CpuBackend as Backend>::update_text_input_value(self, node, value)
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        <CpuBackend as Backend>::update_text_input_secure(self, node, secure)
    }

    fn set_text_input_focus_handler(&mut self, node: &Self::Node, handler: Rc<dyn Fn(bool)>) {
        <CpuBackend as Backend>::set_text_input_focus_handler(self, node, flushing1(handler))
    }

    fn update_text_input_placeholder(&mut self, node: &Self::Node, placeholder: Option<&str>) {
        <CpuBackend as Backend>::update_text_input_placeholder(self, node, placeholder)
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
        <CpuBackend as Backend>::create_text_area(
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
        <CpuBackend as Backend>::update_text_area_value(self, node, value)
    }

    fn make_text_input_handle(&self, node: &Self::Node) -> primitives::text_input::TextInputHandle {
        <CpuBackend as Backend>::make_text_input_handle(self, node)
    }

    fn make_text_area_handle(&self, node: &Self::Node) -> primitives::text_area::TextAreaHandle {
        <CpuBackend as Backend>::make_text_area_handle(self, node)
    }
}

impl caps::ToggleOps for CpuBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <CpuBackend as Backend>::create_toggle(self, initial_value, flushing1(on_change), a11y)
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        <CpuBackend as Backend>::update_toggle_value(self, node, value)
    }

    fn make_toggle_handle(&self, node: &Self::Node) -> primitives::toggle::ToggleHandle {
        <CpuBackend as Backend>::make_toggle_handle(self, node)
    }
}

impl caps::SliderOps for CpuBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <CpuBackend as Backend>::create_slider(
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
        <CpuBackend as Backend>::update_slider_value(self, node, value)
    }

    fn make_slider_handle(&self, node: &Self::Node) -> primitives::slider::SliderHandle {
        <CpuBackend as Backend>::make_slider_handle(self, node)
    }
}

impl caps::ActivityIndicatorOps for CpuBackend {
    fn create_activity_indicator(
        &mut self,
        size: primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<&Color>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <CpuBackend as Backend>::create_activity_indicator(self, size, color, a11y)
    }

    fn update_activity_indicator_size(
        &mut self,
        node: &Self::Node,
        size: primitives::activity_indicator::ActivityIndicatorSize,
    ) {
        <CpuBackend as Backend>::update_activity_indicator_size(self, node, size)
    }

    fn make_activity_indicator_handle(
        &self,
        node: &Self::Node,
    ) -> primitives::activity_indicator::ActivityIndicatorHandle {
        <CpuBackend as Backend>::make_activity_indicator_handle(self, node)
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
        a11y: &AccessibilityProps,
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
        <CpuBackend as Backend>::create_scroll_view(self, horizontal, on_scroll, a11y)
    }

    fn node_scroll(&self, node: &Self::Node) -> (f32, f32) {
        <CpuBackend as Backend>::node_scroll(self, node)
    }

    fn set_node_scroll(&mut self, node: &Self::Node, x: f32, y: f32) {
        <CpuBackend as Backend>::set_node_scroll(self, node, x, y)
    }

    fn make_scroll_view_handle(&self, node: &Self::Node) -> primitives::scroll_view::ScrollViewHandle {
        <CpuBackend as Backend>::make_scroll_view_handle(self, node)
    }
}

impl caps::SafeAreaOps for CpuBackend {
    fn apply_safe_area_padding(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <CpuBackend as Backend>::apply_safe_area_padding(self, node, sides)
    }

    fn apply_scroll_view_safe_area_inset(&mut self, node: &Self::Node, sides: SafeAreaSides) {
        <CpuBackend as Backend>::apply_scroll_view_safe_area_inset(self, node, sides)
    }
}

impl caps::VirtualizerOps for CpuBackend {
    fn create_virtualizer(
        &mut self,
        callbacks: VirtualizerCallbacks<Self::Node>,
        overscan: f32,
        layout: primitives::virtualizer::VirtualLayout,
        a11y: &AccessibilityProps,
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
        <CpuBackend as Backend>::create_virtualizer(self, callbacks, overscan, layout, a11y)
    }

    fn virtualizer_data_changed(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::virtualizer_data_changed(self, node)
    }

    fn release_virtualizer(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::release_virtualizer(self, node)
    }

    fn make_virtualizer_handle(&self, node: &Self::Node) -> primitives::virtualizer::VirtualizerHandle {
        <CpuBackend as Backend>::make_virtualizer_handle(self, node)
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
        a11y: &AccessibilityProps,
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
        <CpuBackend as Backend>::create_graphics(self, on_ready, on_resize, on_lost, a11y)
    }

    fn release_graphics(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::release_graphics(self, node)
    }

    fn make_graphics_handle(&self, node: &Self::Node) -> primitives::graphics::GraphicsHandle {
        <CpuBackend as Backend>::make_graphics_handle(self, node)
    }
}

impl caps::PortalOps for CpuBackend {
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        let on_dismiss = on_dismiss.map(flushing0);
        <CpuBackend as Backend>::create_portal(self, target, on_dismiss, trap_focus, a11y)
    }

    fn release_portal(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::release_portal(self, node)
    }

    fn set_portal_hidden(&mut self, node: &Self::Node, hidden: bool) {
        <CpuBackend as Backend>::set_portal_hidden(self, node, hidden)
    }

    fn make_portal_handle(&self, node: &Self::Node) -> primitives::portal::PortalHandle {
        <CpuBackend as Backend>::make_portal_handle(self, node)
    }
}

impl caps::PresenceOps for CpuBackend {
    fn create_presence_placeholder(&mut self, a11y: &AccessibilityProps) -> Self::Node {
        <CpuBackend as Backend>::create_presence_placeholder(self, a11y)
    }

    fn apply_presence(
        &mut self,
        node: &Self::Node,
        state: primitives::presence::PresenceState,
        transition: Option<(u32, Easing)>,
    ) {
        <CpuBackend as Backend>::apply_presence(self, node, state, transition)
    }

    fn make_presence_handle(&self, node: &Self::Node) -> primitives::presence::PresenceHandle {
        <CpuBackend as Backend>::make_presence_handle(self, node)
    }
}

impl caps::NavigatorOps for CpuBackend {
    fn create_navigator(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        presentation: Rc<dyn Any>,
        host: primitives::navigator::NavigatorHost<Self::Node>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // NOT wrapped: NavigatorHost's callbacks belong to the OLD-core
        // navigator path; the vocabulary navigator handlers own screens
        // on the new core and their dispatch is handler-safe (queue +
        // tick signal committed by a driver effect on flush). Author
        // navigation stages via handlers already wrapped above. On this
        // backend the call renders the visible placeholder either way.
        <CpuBackend as Backend>::create_navigator(
            self,
            type_id,
            type_name,
            presentation,
            host,
            a11y,
        )
    }

    fn release_navigator(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::release_navigator(self, node)
    }

    fn apply_navigator_slot_style(
        &mut self,
        node: &Self::Node,
        slot: &'static str,
        style: &Rc<StyleRules>,
    ) {
        <CpuBackend as Backend>::apply_navigator_slot_style(self, node, slot, style)
    }

    fn make_navigator_handle(&self, node: &Self::Node) -> primitives::navigator::NavigatorHandle {
        <CpuBackend as Backend>::make_navigator_handle(self, node)
    }

    fn navigator_attach_initial(
        &mut self,
        navigator: &Self::Node,
        screen: Self::Node,
        scope_id: u64,
        options: Box<dyn Any>,
    ) {
        <CpuBackend as Backend>::navigator_attach_initial(self, navigator, screen, scope_id, options)
    }
}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for CpuBackend {
    fn create_external(
        &mut self,
        type_id: TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        <CpuBackend as Backend>::create_external(self, type_id, type_name, payload, a11y)
    }

    fn release_external(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::release_external(self, node)
    }

    fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
        <CpuBackend as Backend>::missing_primitive_placeholder(self, label)
    }
}

impl caps::DocumentOps for CpuBackend {
    fn create_element(&mut self, tag: &str) -> Self::Node {
        <CpuBackend as Backend>::create_element(self, tag)
    }

    fn attach_html_id(&self, node: &Self::Node, id: &str) {
        <CpuBackend as Backend>::attach_html_id(self, node, id)
    }

    fn attach_html_class(&self, node: &Self::Node, class: &str) {
        <CpuBackend as Backend>::attach_html_class(self, node, class)
    }

    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {
        <CpuBackend as Backend>::attach_html_style(self, node, prop, value)
    }

    fn register_raw_css(&mut self, css: &str) {
        <CpuBackend as Backend>::register_raw_css(self, css)
    }
}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for CpuBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        <CpuBackend as Backend>::apply_style(self, node, style)
    }

    fn mint_style_class(&mut self, style: &Rc<StyleRules>) -> Option<String> {
        <CpuBackend as Backend>::mint_style_class(self, style)
    }

    fn mint_class_for_app(&mut self, app: &StyleApplication) -> Option<String> {
        <CpuBackend as Backend>::mint_class_for_app(self, app)
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(StateBits, Rc<StyleRules>)],
    ) {
        <CpuBackend as Backend>::apply_styled_states(self, node, base, overlays)
    }

    fn apply_styled_variants(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        state_overlays: &[(StateBits, Rc<StyleRules>)],
        breakpoint_overlays: &[(Breakpoint, Rc<StyleRules>)],
        container_overlays: &[(f32, Rc<StyleRules>)],
    ) {
        <CpuBackend as Backend>::apply_styled_variants(
            self,
            node,
            base,
            state_overlays,
            breakpoint_overlays,
            container_overlays,
        )
    }

    fn mark_container(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::mark_container(self, node)
    }

    fn handles_states_natively(&self) -> bool {
        <CpuBackend as Backend>::handles_states_natively(self)
    }

    fn token_updates_propagate_via_cascade(&self) -> bool {
        <CpuBackend as Backend>::token_updates_propagate_via_cascade(self)
    }

    fn register_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <CpuBackend as Backend>::register_stylesheet(self, rules)
    }

    fn unregister_stylesheet(&mut self, rules: &[Rc<StyleRules>]) {
        <CpuBackend as Backend>::unregister_stylesheet(self, rules)
    }

    fn install_tokens(&mut self, tokens: &[TokenEntry]) {
        <CpuBackend as Backend>::install_tokens(self, tokens)
    }

    fn update_tokens(&mut self, tokens: &[TokenEntry]) {
        <CpuBackend as Backend>::update_tokens(self, tokens)
    }

    fn on_node_unstyled(&mut self, node: &Self::Node) {
        <CpuBackend as Backend>::on_node_unstyled(self, node)
    }

    fn attach_states(&mut self, node: &Self::Node, setter: Rc<dyn Fn(StateBits, bool)>) {
        // Dispatch-site glue: state flips can stage writes when the
        // style path routes states through signals.
        let setter: Rc<dyn Fn(StateBits, bool)> = {
            let f = setter;
            Rc::new(move |bits, on| {
                f(bits, on);
                schedule_flush();
            })
        };
        <CpuBackend as Backend>::attach_states(self, node, setter)
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        <CpuBackend as Backend>::set_disabled(self, node, disabled)
    }

    fn supports_preminted_styles(&self) -> bool {
        <CpuBackend as Backend>::supports_preminted_styles(self)
    }

    fn apply_default_text_font(&mut self, font: Option<&FontFamily>) {
        <CpuBackend as Backend>::apply_default_text_font(self, font)
    }

    fn supports_js_class_bindings(&self) -> bool {
        <CpuBackend as Backend>::supports_js_class_bindings(self)
    }

    fn register_reactive_class_binding(
        &mut self,
        node: &Self::Node,
        signal_id: u64,
        values: &[u32],
        classes: &[&str],
        value_reader: Rc<dyn Fn() -> u32>,
    ) -> u32 {
        <CpuBackend as Backend>::register_reactive_class_binding(
            self,
            node,
            signal_id,
            values,
            classes,
            value_reader,
        )
    }

    fn release_reactive_class_binding(&mut self, binding_id: u32) {
        <CpuBackend as Backend>::release_reactive_class_binding(self, binding_id)
    }
}

impl caps::AssetOps for CpuBackend {
    fn register_asset(&mut self, id: AssetId, kind: AssetTag, source: &AssetSource) {
        <CpuBackend as Backend>::register_asset(self, id, kind, source)
    }

    fn unregister_asset(&mut self, id: AssetId, kind: AssetTag) {
        <CpuBackend as Backend>::unregister_asset(self, id, kind)
    }

    fn register_typeface(
        &mut self,
        id: TypefaceId,
        family_name: &str,
        faces: &[TypefaceFace],
        fallback: SystemFallback,
    ) {
        <CpuBackend as Backend>::register_typeface(self, id, family_name, faces, fallback)
    }

    fn unregister_typeface(&mut self, id: TypefaceId) {
        <CpuBackend as Backend>::unregister_typeface(self, id)
    }
}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for CpuBackend {
    fn update_accessibility(
        &mut self,
        node: &Self::Node,
        a11y: &AccessibilityProps,
        inferred_role: Option<Role>,
    ) {
        <CpuBackend as Backend>::update_accessibility(self, node, a11y, inferred_role)
    }

    fn announce_for_accessibility(&mut self, msg: &str, priority: LiveRegionPriority) {
        <CpuBackend as Backend>::announce_for_accessibility(self, msg, priority)
    }

    fn dump_accessibility_tree(&self) -> Option<AccessibilityTree> {
        <CpuBackend as Backend>::dump_accessibility_tree(self)
    }
}

impl caps::AnimationOps for CpuBackend {
    fn set_animated_f32(&mut self, node: &Self::Node, prop: AnimProp, value: f32) {
        <CpuBackend as Backend>::set_animated_f32(self, node, prop, value)
    }

    fn set_animated_color(&mut self, node: &Self::Node, prop: AnimProp, value: [f32; 4]) {
        <CpuBackend as Backend>::set_animated_color(self, node, prop, value)
    }
}

impl caps::IntrospectionOps for CpuBackend {
    fn frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <CpuBackend as Backend>::frame(self, node)
    }

    fn absolute_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <CpuBackend as Backend>::absolute_frame(self, node)
    }

    fn device_frame(&self, node: &Self::Node) -> Option<ViewportRect> {
        <CpuBackend as Backend>::device_frame(self, node)
    }

    fn supports_native_introspection(&self) -> bool {
        <CpuBackend as Backend>::supports_native_introspection(self)
    }

    fn introspect_native(&self, node: &Self::Node) -> Option<NativeNode> {
        <CpuBackend as Backend>::introspect_native(self, node)
    }

    fn note_introspection_root(&self, node: &Self::Node) {
        <CpuBackend as Backend>::note_introspection_root(self, node)
    }

    fn supports_screenshot(&self) -> bool {
        <CpuBackend as Backend>::supports_screenshot(self)
    }

    fn capture_screenshot(&self, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
        <CpuBackend as Backend>::capture_screenshot(self, done)
    }
}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for CpuBackend {
    fn supports_batched_repeat(&self) -> bool {
        <CpuBackend as Backend>::supports_batched_repeat(self)
    }

    fn execute_batch(&mut self, batch: BackendBatch) -> Vec<Self::Node> {
        <CpuBackend as Backend>::execute_batch(self, batch)
    }

    fn execute_batch_with_attach(
        &mut self,
        batch: BackendBatch,
        parent: &mut Self::Node,
        attach_locals: &[u32],
    ) -> Vec<Self::Node> {
        <CpuBackend as Backend>::execute_batch_with_attach(self, batch, parent, attach_locals)
    }
}

impl caps::WireBindingOps for CpuBackend {
    fn note_text_binding(&mut self, node: &Self::Node, signal_ids: &[u64], method: &'static str) {
        <CpuBackend as Backend>::note_text_binding(self, node, signal_ids, method)
    }

    fn note_signal_initial(&mut self, signal_id: u64, value: &runtime_core::__serde_json::Value) {
        <CpuBackend as Backend>::note_signal_initial(self, signal_id, value)
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        <CpuBackend as Backend>::note_when_binding(
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
        <CpuBackend as Backend>::note_switch_binding(
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
        <CpuBackend as Backend>::note_repeat_binding(
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
        <CpuBackend as Backend>::note_virtualizer_binding(
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
        <CpuBackend as Backend>::supports_lazy_slot_capture(self)
    }

    fn begin_slot_capture(&mut self) {
        <CpuBackend as Backend>::begin_slot_capture(self)
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        <CpuBackend as Backend>::end_slot_capture(self, slot_root)
    }
}

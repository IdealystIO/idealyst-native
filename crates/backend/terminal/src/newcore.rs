//! Rendering: the `runtime_scene::Host` + capability-trait surface, the
//! boot entry, and the flush driver.
//!
//! [`TerminalBackend`] implements [`runtime_scene::Host`] plus **all 30**
//! capability traits (`runtime_vocabulary::caps`) — the production shape
//! of the migration. Every mechanism body in this file was moved here
//! verbatim from the crate's old `impl runtime_core::Backend for TerminalBackend`
//! when the 159-method mega-trait was deleted, so the grid mechanism code (node allocation, Taffy
//! layout, hit-testing, `render_to_grid`)
//! is unchanged: the same scene renders to the same cells
//! (pinned by `tests/newcore_parity.rs` against the frozen old-core grid
//! dumps).
//! Capabilities this backend does not implement are simply absent — the
//! caps-trait DEFAULT bodies serve them, and those defaults were audited
//! byte-for-byte against the `Backend` defaults they replace
//! (`docs/runtime-v2-deletion-baseline.md` S2.1; 120 of this backend's
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
//! The host (see `host_terminal::newcore::run`) constructs the backend,
//! installs the global self-handle + its scheduler, sets
//! `cell_size`/viewport, then calls [`start`]:
//!
//! 1. Monotonic time source (idempotent, first install wins).
//! 2. Registry: [`runtime_vocabulary::register_builtins`] + the app's
//!    `register` seam.
//! 3. Fresh [`World`]; build + [`realize`] inside `world.enter`;
//!    capture the per-world viewport ctx's size signal AFTER the build
//!    (the ctx's bucket memo pins the breakpoint table at creation —
//!    same ordering comment as backend-web/wgpu).
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
//! the wgpu/macOS shape:
//!
//! 1. **Dispatch-site glue** (this module's caps impls): every author
//!    callback the backend can fire — press handlers ([`crate::ClickOutcome`]'s
//!    returned handler included: it IS the wrapped closure), toggle /
//!    slider / input `on_change`, key handlers, `on_scroll`, hover /
//!    wheel / touch — is wrapped so that after the author code returns,
//!    one deduped [`schedule_flush`] microtask is queued. The host's
//!    per-frame `scheduler::tick()` drains microtasks BEFORE
//!    `render_to_grid`, so an input event commits in the same frame it
//!    was dispatched: input event → staged writes → flush → paint.
//! 2. **Post-dispatch hook** ([`crate::dispatch_hook`]): author code
//!    that runs from the *scheduler* (`after_ms` debounces, `raf_loop`
//!    animation ticks) has no wrapped callback; `host-terminal`'s tick
//!    fires the hook after each such callback ([`start`] installs
//!    [`schedule_flush`] into the slot; no-op default, so the old core
//!    never pays). Microtasks deliberately do NOT fire the hook — the
//!    flush itself rides one (see `dispatch_hook`'s module docs).
//!
//! Everything funnels through [`schedule_flush`]/`flush_now`, which
//! skips re-entrant flushes (`world.is_flushing()`).
//!
//! # Viewport source
//!
//! [`TerminalBackend::set_viewport`] is the one source of truth for the
//! cell viewport (the host calls it on every resize). It already writes
//! the old-core TLS value (which seeds the per-world ctx at creation);
//! under `new-core` it ALSO forwards through [`forward_viewport`] into
//! the mounted world's viewport signal — captured, not injected, so the
//! push stages through the handle and rides one deduped
//! [`schedule_flush`] (the backend-web resize-listener discipline).
//! Cell counts are pushed, matching the old TLS write — the terminal's
//! logical px is one cell.
//!
//! # Residual seams (named, none silent)
//!
//! - Navigators are vocabulary built-ins (swap/stack) mounted through
//!   `runtime_vocabulary::handlers::navigator`; the backend-side
//!   `NavigatorRegistry` + inventory registrars the old core dispatched
//!   through are gone (deletion-baseline S2.3), and every surviving
//!   `NavigatorOps` method resolves to its caps default.
//! - `dispatch_key`'s default-editing path defers its `on_change` to a
//!   microtask (borrow discipline); the wrapped `on_change` then queues
//!   the flush from inside that microtask — the host's drain-until-empty
//!   loop runs it the same tick.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::animation::AnimProp;
use runtime_shared::primitives;
use runtime_shared::{
    Action, ColorScheme, Platform, StyleRules,
};
use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;
use crate::node::{self, NodeKind};
use crate::{format_button_label, handles, terminal_advance_spinner, terminal_toggle_press};
use runtime_shared::color::{parse_or, Rgba};
use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_shared::Color as FwColor;
use runtime_vocabulary::caps::{TextOps as _, ViewOps as _};

use crate::{TermNode, TerminalBackend};

// Re-exported so the host shell (`host-terminal`) and app wrappers can
// name the boot-path types without a direct runtime-scene dependency —
// mirrors `render_wgpu::newcore`.
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
    static BACKEND: RefCell<Option<Weak<RefCell<TerminalBackend>>>> = const { RefCell::new(None) };
}

/// Everything the boot path must keep alive. Field order is drop order:
/// the realized tree unmounts before the world (its slots' owner) dies.
/// The live host loop holds this value for the whole session and calls
/// [`NewCoreApp::stop`] on quit (the terminal host RETURNS on quit —
/// same teardown-ordering care the old host takes with its `Owner`).
pub struct NewCoreApp {
    realized: Realized<TermNode>,
    _backend: Rc<RefCell<TerminalBackend>>,
    _registry: Rc<Registry<TerminalBackend>>,
    world: World,
}

impl NewCoreApp {
    /// Borrow the live tree (tests, diagnostics).
    pub fn with_realized<R>(&self, f: impl FnOnce(&Realized<TermNode>) -> R) -> R {
        f(&self.realized)
    }

    /// The mounted world (tests can flush it explicitly).
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Unmount: drops the `Realized` (cleanups fire), uninstalls the
    /// flush driver + viewport sink, and drops the world. The terminal
    /// host calls this on quit BEFORE restoring the terminal — the
    /// reactive teardown must run while the thread's TLS is intact
    /// (same rationale as the old host's explicit `Owner` drop).
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
/// The host must have: constructed the backend, installed the global
/// self-handle ([`crate::install_global_self`] — the toggle-press and
/// spinner paths use it), installed a scheduler (the flush driver rides
/// `schedule_microtask`; `host-terminal` installs its tick scheduler
/// before mounting), and applied `cell_size`/viewport. See
/// `host_terminal::newcore::run` for the canonical caller.
///
/// `register` runs after [`runtime_vocabulary::register_builtins`], so
/// apps/SDKs can register their own payload handlers on the same
/// registry before the tree realizes. The build closure runs inside
/// `world.enter`, so free `signal()`/`effect()` calls work; top-level
/// creations are world-root-owned (they live until [`NewCoreApp::stop`]).
#[inline]
pub fn start(
    backend: Rc<RefCell<TerminalBackend>>,
    register: impl FnOnce(&mut Registry<TerminalBackend>),
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
    backend: Rc<RefCell<TerminalBackend>>,
    register: R,
    build: B,
) -> NewCoreApp where
    S: runtime_vocabulary::BuiltinSet,
    R: FnOnce(&mut Registry<TerminalBackend>),
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
    let mut registry: Registry<TerminalBackend> = Registry::new();
    runtime_vocabulary::register_builtins_with::<_, S>(&mut registry);
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

    // Buffered-microtask drain — a no-op under the host's tick
    // scheduler, load-bearing under a buffering test scheduler. Must
    // run with NO backend borrow held (drained tasks re-borrow);
    // ENTERED because a buffered task may do creation-side work.
    world.enter(runtime_shared::scheduling::drain_buffered_microtasks);

    // Single-root contract, matching the old-core mount (`find_root`
    // wants exactly one application root — id 1).
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_terminal::newcore::start: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    caps::LifecycleOps::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount before the first paint.
    world.flush();

    // Install the flush driver: schedule_flush becomes reachable from
    // (a) the author-callback wrappers in the caps impls below and
    // (b) the host scheduler's post-dispatch hook.
    crate::dispatch_hook::install_dispatch_hook(schedule_flush);
    set_flush_world(Some(world.clone()));
    // Live viewport source: `TerminalBackend::set_viewport` now reaches
    // the world's ctx through `forward_viewport`.
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
pub fn with_backend<R>(f: impl FnOnce(&Rc<RefCell<TerminalBackend>>) -> R) -> Option<R> {
    let rc = BACKEND.with(|b| b.borrow().as_ref().and_then(Weak::upgrade));
    rc.map(|rc| f(&rc))
}

// ===========================================================================
// Flush driver
// ===========================================================================

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// The author-callback wrappers and the scheduler's dispatch hook call
/// this right after author-visible dispatch; the host's per-frame
/// `scheduler::tick()` drains it before painting.
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
/// observe the committed grid before returning (tests, a future robot
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

// NOTE: there is deliberately no `enter_mounted_world` helper here (the
// web/wgpu glue has one). It exists to make virtualizer
// `mount_item`/`release_item` — the one platform-callback family that
// REALIZES a row, i.e. creation-side work needing the ambient world —
// run inside `World::enter`. This backend implements no `VirtualizerOps`
// method, so `create_virtualizer` resolves to the caps default
// (`missing_primitive_placeholder`) and those callbacks are never
// invoked. If a real terminal virtualizer lands, port the helper from
// `backend_web::newcore` along with it.

// ===========================================================================
// Viewport source (the new-core terminal viewport seam)
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

/// Forward one viewport report (in CELLS, the terminal's logical px —
/// same value the old TLS write carries) into the mounted world's
/// viewport ctx. No-op before [`start`] / after teardown. Called by
/// [`TerminalBackend::set_viewport`].
pub(crate) fn forward_viewport(size: runtime_shared::ViewportSize) {
    let Some(sig) = VIEWPORT_SINK.with(|s| s.get()) else {
        return;
    };
    // Staged write outside `enter` (handle-routed, equality-guarded)
    // + one deduped flush — commits on the next scheduler turn, like
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
// render_wgpu/src/newcore.rs / runtime_vocabulary::bridge — keep
// mechanically in sync; the AllCaps bound on register_builtins is the
// compile gate)
// ===========================================================================

// ---------------------------------------------------------------------------
// Host — the P1 structural seam
// ---------------------------------------------------------------------------

impl Host for TerminalBackend {
    type Node = TermNode;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        let (parent_layout, child_layout) = match (
            self.nodes.get(&parent.id).map(|d| d.layout),
            self.nodes.get(&child.id).map(|d| d.layout),
        ) {
            (Some(p), Some(c)) => (p, c),
            _ => return,
        };
        self.layout.add_child(parent_layout, child_layout);
        if let Some(p) = self.nodes.get_mut(&parent.id) {
            p.children.push(child.id);
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
        let Some(data) = self.nodes.get(&node.id) else { return };
        let parent_layout = data.layout;
        let children = data.children.clone();
        for cid in &children {
            let cdata = self.nodes.remove(cid);
            if let Some(cd) = cdata {
                // Strip the Taffy edge first, then drop the slot.
                // Mirrors the iOS pattern; see
                // [[project_ios_clear_children_taffy_sync]].
                self.layout.remove_child(parent_layout, cd.layout);
                self.layout.remove_node(cd.layout);
                self.layout_to_id.remove(&cd.layout);
                // Also tear down any grandchildren that this node
                // owned — recursive free.
                self.drop_subtree(&cd.children);
            }
        }
        self.layout.mark_dirty(parent_layout);
        if let Some(p) = self.nodes.get_mut(&node.id) {
            p.children.clear();
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

impl caps::AppEnvOps for TerminalBackend {
    fn color_scheme(&self) -> ColorScheme {
        // Most terminals these days are dark by default. Apps that
        // care can branch on `Platform::Custom("Terminal")` for a
        // proper choice.
        ColorScheme::Dark
    }

    fn platform(&self) -> Platform {
        Platform::Custom("Terminal")
    }

    fn set_app_key_handler(&mut self, handler: Option<primitives::key::KeyDownHandler>) {
        // Dispatch-site glue: the app-level key handler runs author code
        // (`dispatch_key` routes here before the focused-input path).
        let handler = handler.map(flushing_key);
        self.app_key_handler = handler;
    }
}

impl caps::LifecycleOps for TerminalBackend {
    fn finish(&mut self, _root: Self::Node) {}
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for TerminalBackend {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> Self::Node {
        self.alloc_node(NodeKind::View, String::new())
    }

    fn make_view_handle(&self, node: &Self::Node) -> runtime_shared::ViewHandle {
        handles::make_view_handle(node)
    }
}

impl caps::InputOps for TerminalBackend {}

impl caps::PressableOps for TerminalBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> Self::Node {
        // Dispatch-site glue: the wrapped closure is what
        // `dispatch_click` hands back in `ClickOutcome::HandlerFired`,
        // so the host's plain `h()` call gets the flush for free.
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

impl caps::TextOps for TerminalBackend {
    fn create_text(&mut self, content: &str, _a11y: &AccessibilityProps) -> Self::Node {
        let node = self.alloc_node(NodeKind::Text, content.to_string());
        self.install_text_measure(node.id);
        node
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        let layout = match self.nodes.get(&node.id) {
            Some(d) if d.content == content => return,
            Some(d) => d.layout,
            None => return,
        };
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.content = content.to_string();
        }
        // The Taffy measure_fn captures its content snapshot by
        // value (we can't borrow `&mut self` inside the closure), so
        // the measure_fn still believes the text is the original
        // empty string until we re-install it. Without this, the
        // text node measures 0x0 and the rendered glyphs land in
        // a zero-size frame — nothing visible. Re-installing is
        // cheap (one Rc clone per swap).
        self.install_text_measure(node.id);
        self.layout.mark_dirty(layout);
    }

    fn make_text_handle(&self, node: &Self::Node) -> runtime_shared::TextHandle {
        handles::make_text_handle(node)
    }
}

impl caps::ButtonOps for TerminalBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        _leading_icon: Option<&primitives::icon::IconData>,
        _trailing_icon: Option<&primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: wrap the Action's runtime evaluator (the
        // closure `dispatch_click` returns); the serialization metadata
        // passes through untouched.
        let on_click = Action {
            method: on_click.method,
            inputs: on_click.inputs.clone(),
            initial: on_click.initial.clone(),
            output: on_click.output,
            fire: flushing0(on_click.fire.clone()),
        };
        let on_click = &on_click;
        // Render Button as `[ label ]` for a consistent at-a-glance
        // affordance on terminal — matches the existing Toggle's
        // `[ ● ]` bracket convention. Store the bracketed form
        // directly so the captured `measure_fn` (which reads
        // `data.content`) sizes the node for the bracketed width.
        // Paint goes through `paint_text` unchanged.
        let bracketed = format_button_label(label);
        let node = self.alloc_node(NodeKind::Button, bracketed);
        let fire = on_click.fire.clone();
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.on_click = Some(fire);
        }
        self.install_text_measure(node.id);
        node
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        // Re-wrap reactive label updates to keep the bracketed
        // form in sync with what `create_button` stored.
        self.update_text(node, &format_button_label(label));
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for TerminalBackend {
    fn create_image(
        &mut self,
        _src: &str,
        _alt: Option<&str>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.create_view(a11y)
    }
}

impl caps::IconOps for TerminalBackend {
    fn create_icon(
        &mut self,
        _data: &runtime_shared::primitives::icon::IconData,
        _color: Option<&FwColor>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        self.create_view(a11y)
    }
}

impl caps::LinkOps for TerminalBackend {
    fn create_link(
        &mut self,
        config: primitives::link::LinkConfig,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: link activation dispatches navigation
        // (stages nav-queue tick signals on the new core). The wrapped
        // closure lands in the node's on_click slot, so nav-link clicks
        // flush exactly like pressables.
        let mut config = config;
        config.on_activate = flushing0(config.on_activate.clone());
        // Terminal renders links as plain Pressable wrappers — a click
        // anywhere inside fires `on_activate`. The trait default
        // collapses to `create_view` and drops `on_activate` entirely,
        // which is why nav-link clicks were silently no-op'ing
        // before. The on_click slot mirrors what `create_pressable`
        // sets, so the existing hit-test path picks it up.
        let node = self.alloc_node(NodeKind::Pressable, String::new());
        if let Some(data) = self.nodes.get_mut(&node.id) {
            data.on_click = Some(config.on_activate);
        }
        node
    }
}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for TerminalBackend {
    fn create_text_input(
        &mut self,
        initial_value: &str,
        placeholder: Option<&str>,
        on_change: Rc<dyn Fn(String)>,
        on_key_down: Option<primitives::key::KeyDownHandler>,
        on_blur: Option<primitives::text_input::BlurHandler>,
        secure: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // on_change is fired from `apply_key_default`'s deferred
        // microtask; the wrapper queues the flush from inside it — the
        // host's drain-until-empty loop commits the same tick.
        let on_change = flushing1(on_change);
        let on_key_down = on_key_down.map(flushing_key);
        let _on_blur = on_blur.map(|f| -> primitives::text_input::BlurHandler {
                Rc::new(move || {
                    let outcome = f();
                    schedule_flush();
                    outcome
                })
            });
        let node = self.alloc_node(NodeKind::TextInput, String::new());
        if let Some(d) = self.nodes.get_mut(&node.id) {
            let placeholder_owned = placeholder.map(|s| s.to_string());
            // Seed an intrinsic width that fits the placeholder (so
            // empty inputs aren't 0-wide) plus 2 cells of breathing
            // room. Authors can override with explicit `width` in
            // the stylesheet.
            let intrinsic_cells = placeholder_owned
                .as_ref()
                .map(|s| s.chars().count() as f32)
                .unwrap_or(0.0)
                .max(initial_value.chars().count() as f32)
                .max(8.0)
                + 2.0;
            let (cw, ch) = self.cell_size;
            self.layout
                .set_intrinsic_size(d.layout, intrinsic_cells * cw, 1.0 * ch);
            d.input = Some(Box::new(node::InputState {
                value: initial_value.to_string(),
                cursor: initial_value.chars().count(),
                placeholder: placeholder_owned,
                secure,
                on_change,
                on_key_down,
            }));
        }
        node
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        let Some(d) = self.nodes.get_mut(&node.id) else { return };
        let Some(input) = d.input.as_mut() else { return };
        if input.value == value {
            return;
        }
        input.value = value.to_string();
        // Clamp the cursor in case the controlled value got
        // truncated below the previous cursor position.
        let max = input.value.chars().count();
        if input.cursor > max {
            input.cursor = max;
        }
    }

    fn update_text_input_secure(&mut self, node: &Self::Node, secure: bool) {
        // Flip the stored mask flag; the next render bullet-masks (or reveals)
        // the value. No rebuild — the cursor/value state is untouched.
        let Some(d) = self.nodes.get_mut(&node.id) else { return };
        let Some(input) = d.input.as_mut() else { return };
        input.secure = secure;
    }
}

impl caps::ToggleOps for TerminalBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // The backend's toggle click handler (via the global
        // self-handle) forwards to on_change — wrapping it here covers
        // the whole press path.
        let on_change = flushing1(on_change);
        // Render: `[ ]` (off) / `[●]` (on). 3 cells wide intrinsic.
        let node = self.alloc_node(NodeKind::Toggle, String::new());
        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.toggle_value = initial_value;
            // Wrap `on_change` so the click handler (no args) reads
            // the *current* value at click time, flips it, and
            // forwards the new value. The framework's controlled-
            // value Effect re-fires `update_toggle_value` so the
            // backend's `toggle_value` stays in sync with the
            // signal.
            //
            // We pull the current value from the backend via the
            // shared id — no need for a separate Cell.
            let id = node.id;
            let oc = on_change.clone();
            d.on_click = Some(Rc::new(move || {
                // The framework's controlled-value cycle: this fires
                // on press, we flip and call on_change with the new
                // value; the parent updates its `Signal<bool>`; the
                // framework's effect calls `update_toggle_value`
                // with the same new value, which is a no-op (we
                // skip on equality). One coherent state.
                terminal_toggle_press(id, &oc);
            }));
            // Cells: "[ x ]" — 5 cells wide for breathing room.
            let (cw, ch) = self.cell_size;
            self.layout.set_intrinsic_size(d.layout, 5.0 * cw, 1.0 * ch);
        }
        node
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.toggle_value = value;
        }
    }
}

impl caps::SliderOps for TerminalBackend {}

impl caps::ActivityIndicatorOps for TerminalBackend {
    fn create_activity_indicator(
        &mut self,
        size: ActivityIndicatorSize,
        color: Option<&FwColor>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        let node = self.alloc_node(NodeKind::ActivityIndicator, String::new());
        if let Some(d) = self.nodes.get_mut(&node.id) {
            // Color seed: optional explicit color, otherwise muted.
            if let Some(c) = color {
                d.fg = Some(parse_or(&c.0, Rgba::new(180, 180, 180, 255)));
            }
            // Small = 1 cell tall, Large = 1 cell tall too — we
            // can't actually grow a single braille glyph. Width: 3
            // cells either way to give the spinner some space.
            let w_cells = match size {
                ActivityIndicatorSize::Small => 3.0,
                ActivityIndicatorSize::Large => 5.0,
            };
            let (cw, ch) = self.cell_size;
            self.layout.set_intrinsic_size(d.layout, w_cells * cw, 1.0 * ch);
        }
        // The walker fires no per-frame effect for this primitive,
        // so we install our own `raf_loop` to advance the phase.
        // Each tick bumps `anim_phase` by ~one frame's worth of the
        // 10-step braille cycle. The render path samples
        // `anim_phase` to pick the current glyph.
        let id = node.id;
        let task = runtime_shared::raf_loop(move || {
            terminal_advance_spinner(id);
        });
        // Anchor to the current reactive scope so unmount cancels
        // the loop. `on_cleanup` is a no-op outside a scope, which
        // is fine — top-level binaries leak the handle until exit.
        runtime_shared::on_cleanup(move || drop(task));
        node
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for TerminalBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: on_scroll fires from `apply_scroll_delta`
        // per wheel tick; the flush microtask is deduped so a burst
        // costs one commit.
        let on_scroll = on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
            Rc::new(move |x, y| {
                f(x, y);
                schedule_flush();
            })
        });
        // Terminal owns its own mouse-wheel dispatch (see
        // `dispatch_scroll` + `apply_scroll_delta`) which mutates
        // each ScrollView's `(scroll_x, scroll_y)` in cell units.
        // We stash the `on_scroll` callback on the node and fire it
        // from `apply_scroll_delta` after the offset is clamped.
        // Offsets are reported in cells \u{2014} the terminal's
        // native unit \u{2014} matching the other backends'
        // "current offset in native coordinate space" semantic
        // (web pixels, iOS points, Android dp post-conversion).
        let node = self.alloc_node(NodeKind::ScrollView, String::new());
        let layout = self.nodes.get(&node.id).map(|d| d.layout);
        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.horizontal = horizontal;
            d.on_scroll = on_scroll;
        }
        // Tell Taffy this node behaves like CSS `overflow: scroll` on
        // the chosen axis. Without this, Taffy sizes the scroll view
        // to its content's intrinsic size — i.e. the content fits
        // inside it exactly and there's nothing to scroll. The
        // helper also sets `flex_grow: 1, flex_basis: 0` so the
        // scroll view fills its parent's available main-axis space
        // (matches how an unsized ScrollView behaves on
        // iOS/Android/web where the native scroll view's frame is
        // set by its parent and content has its own coordinate
        // space).
        if let Some(l) = layout {
            self.layout.set_overflow_scroll(l, horizontal);
        }
        node
    }
}

impl caps::SafeAreaOps for TerminalBackend {}

// No two-axis grid engine on this backend yet; every `GridOps`
// method defaults, so `virtual_grid` reports itself as an
// unsupported primitive instead of silently rendering nothing.
impl caps::GridOps for TerminalBackend {}

impl caps::VirtualizerOps for TerminalBackend {}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for TerminalBackend {}

impl caps::PortalOps for TerminalBackend {
    fn create_portal(
        &mut self,
        _target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        _trap_focus: bool,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        let on_dismiss = on_dismiss.map(flushing0);
        let _on_dismiss = on_dismiss;
        self.create_view(a11y)
    }
}

impl caps::PresenceOps for TerminalBackend {}

// The four surviving `NavigatorOps` methods all resolve to their caps
// DEFAULTS (no-op release / no-op slot style / no-op handle). The old
// bodies routed through a backend-side `NavigatorRegistry` +
// per-instance `NavigatorHandler`, both populated exclusively by
// `create_navigator` — the one caps method that CEASES TO EXIST with the
// old core (deletion-baseline S2.3), so the registry could never be
// populated again. Navigators mount through
// `runtime_vocabulary::handlers::navigator` over the Lifecycle/View caps
// instead, and never call this trait.
impl caps::NavigatorOps for TerminalBackend {}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for TerminalBackend {
    fn create_external(
        &mut self,
        _type_id: std::any::TypeId,
        type_name: &'static str,
        _payload: &std::rc::Rc<dyn std::any::Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        // The terminal backend has no external-primitive registry, so every
        // `Element::External` (codeblock, maps, webview, …) lands here. The
        // default `Backend::create_external` is `unimplemented!()`, which
        // PANICS — a terminal app that mounts any external aborts at render
        // (the tutorial mounts `codeblock`, so local `--terminal` crashed
        // here). Render the framework's standard "not supported" text
        // placeholder instead, mirroring the macOS/iOS backends. When a
        // terminal external handler is genuinely needed, add a registry +
        // inventory registrar here (see `MacosBackend::external_handlers`);
        // until then graceful degradation beats a crash.
        let text = format!("[external \"{type_name}\" not supported in terminal]");
        self.create_text(&text, a11y)
    }
}

impl caps::DocumentOps for TerminalBackend {}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for TerminalBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        // Degrade LOUDLY, once. `terminal` has no scrolling gesture model,
        // so `Position::Sticky` renders as `Relative` and
        // `overscroll-behavior` has nothing to govern. Both were
        // previously dropped in silence — the exact "no warning,
        // nothing to grep for" failure `runtime_shared::unsupported`
        // exists to end.
        if matches!(style.position, Some(runtime_shared::Position::Sticky)) {
            runtime_shared::unsupported::warn_once(
                "terminal.sticky",
                "position: Sticky on the terminal backend — rendered as Relative (this backend \
                 has no scroll gesture model). Web and the native backends pin.",
            );
        }
        if style.overscroll_behavior.is_some() {
            runtime_shared::unsupported::warn_once(
                "terminal.overscroll_behavior",
                "overscroll-behavior on the terminal backend — ignored (no scroll gesture \
                 model to govern).",
            );
        }
        let layout_node = match self.nodes.get(&node.id) {
            Some(d) => d.layout,
            None => return,
        };
        // Eagerly resolve `background` and `color` BEFORE handing the
        // rules to `runtime-layout`'s `set_style`. This is load-
        // bearing: the cohort driver Effect re-fires on token-signal
        // changes, calls `apply_one` → this `apply_style`, which
        // resolves the same Tokenized values to cache `bg`/`fg`
        // further down. Without this early read, the cohort path's
        // sidebar updates went through (`d.bg` was updated, the
        // log even showed the dark color), but the render didn't
        // visually pick up the change — the framework's token-
        // subscription bookkeeping needs the resolve to happen
        // BEFORE other style processing for the per-token edges to
        // land in this Effect's dependency set on the first
        // post-toggle re-fire. Without it, the sidebar darkened
        // only on the second-or-later toggle, which read to the
        // user as "doesn't update".
        let _ = style.background.as_ref().map(|t| t.resolve());
        let _ = style.color.as_ref().map(|t| t.resolve());
        self.layout.set_style(layout_node, style);

        // Cache the resolved fg/bg + gradient so the renderer's hot
        // path doesn't re-parse on every cell write.
        let fg = style
            .color
            .as_ref()
            .map(|t| parse_or(&t.resolve().0, Rgba::default()));
        let bg = style
            .background
            .as_ref()
            .map(|t| parse_or(&t.resolve().0, Rgba::TRANSPARENT));
        let gradient = style.background_gradient.as_ref().map(|g| {
            let stops: Vec<(f32, Rgba)> = g
                .stops
                .iter()
                .map(|s| (s.offset, parse_or(&s.color.0, Rgba::TRANSPARENT)))
                .collect();
            let animated_stops = vec![None; stops.len()];
            node::ResolvedGradient {
                kind: g.kind.clone(),
                stops,
                animated_stops,
            }
        });

        // Extract static translate from `style.transform: [...]`.
        // We only support TranslateX/Y on this backend — Scale /
        // Rotate / Skew don't translate to cell semantics. Last-write
        // wins per axis (matches the RN/web "matrix multiply" feel
        // for the translates-only subset).
        let mut static_tx: Option<runtime_shared::Length> = None;
        let mut static_ty: Option<runtime_shared::Length> = None;
        if let Some(transforms) = style.transform.as_ref() {
            for t in transforms {
                match t {
                    runtime_shared::Transform::TranslateX(l) => static_tx = Some(*l),
                    runtime_shared::Transform::TranslateY(l) => static_ty = Some(*l),
                    _ => {}
                }
            }
        }

        // Static opacity from the stylesheet. Without this, an
        // element declared with `opacity: 0.0` (welcome's sun, the
        // vignette wrapper, planets pre-Act-2) starts fully visible
        // because `NodeData.opacity` defaults to 1.0 — only the
        // animation path (`set_animated_f32(Opacity, …)`) ever
        // touched it. Read the resolved value and seed `data.opacity`
        // up front; the animation Effect later overwrites at every
        // frame.
        let static_opacity = style
            .opacity
            .as_ref()
            .map(|t| t.resolve().clamp(0.0, 1.0));

        if let Some(d) = self.nodes.get_mut(&node.id) {
            d.style = Some(style.clone());
            d.fg = fg;
            d.bg = bg;
            d.static_translate_x = static_tx;
            d.static_translate_y = static_ty;
            if let Some(o) = static_opacity {
                d.opacity = o;
            }
            // Preserve any already-animated stop overrides if the
            // gradient's shape didn't change — re-applying a static
            // stylesheet (state overlays, theme refresh) shouldn't
            // reset per-frame animation state. Conservative: only
            // preserve when the new gradient has the same stop
            // count as the old one. Anything more aggressive risks
            // mismatched indices.
            let preserved = d
                .gradient
                .as_ref()
                .and_then(|old| {
                    gradient
                        .as_ref()
                        .filter(|new| new.stops.len() == old.stops.len())
                        .map(|_| old.animated_stops.clone())
                });
            d.gradient = gradient.map(|mut g| {
                if let Some(p) = preserved {
                    g.animated_stops = p;
                }
                g
            });
        }
    }
}

impl caps::AssetOps for TerminalBackend {}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for TerminalBackend {}

impl caps::AnimationOps for TerminalBackend {
    fn set_animated_f32(
        &mut self,
        node: &Self::Node,
        prop: AnimProp,
        value: f32,
    ) {
        let Some(d) = self.nodes.get_mut(&node.id) else { return };
        match prop {
            // Route to the animated slot — apply_style replays
            // (hot-patch path) would otherwise clobber the in-
            // flight value with the stylesheet's static starting
            // opacity. See [`NodeData::animated_opacity`].
            AnimProp::Opacity => d.animated_opacity = Some(value.clamp(0.0, 1.0)),
            AnimProp::TranslateX => d.translate_x = value,
            AnimProp::TranslateY => d.translate_y = value,
            // Sibling-relative ordering. Higher value renders on top
            // of lower. Welcome's planets sweep through positive and
            // negative values as they orbit so they pass in front of
            // and behind the headline.
            AnimProp::ZIndex => d.z_index = value,
            // Scale / Rotate don't map cleanly to a cell grid —
            // documented no-ops so author code stays portable.
            _ => {}
        }
    }

    fn set_animated_color(
        &mut self,
        node: &Self::Node,
        prop: AnimProp,
        value: [f32; 4],
    ) {
        let Some(d) = self.nodes.get_mut(&node.id) else { return };
        let rgba = Rgba::from_srgb_f32(value);
        match prop {
            AnimProp::BackgroundColor => d.animated_bg = Some(rgba),
            AnimProp::ForegroundColor => d.animated_fg = Some(rgba),
            AnimProp::GradientStopColor(idx) => {
                if let Some(g) = d.gradient.as_mut() {
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

impl caps::IntrospectionOps for TerminalBackend {}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for TerminalBackend {}

impl caps::WireBindingOps for TerminalBackend {}

//! Rendering: the `runtime_scene::Host` + capability-trait surface, the
//! boot entry, and the flush driver.
//!
//! [`RokuBackend`] implements [`runtime_scene::Host`] plus **all 30**
//! capability traits (`runtime_vocabulary::caps`) — the production shape
//! of the migration. Every mechanism body in this file was moved here
//! verbatim from the crate's old `impl runtime_core::Backend for RokuBackend`
//! when the 159-method mega-trait was deleted, so the command-stream mechanism code (node
//! allocation, style translation, command emission)
//! is unchanged: the same scene emits the same commands
//! (pinned by `tests/newcore_parity.rs` against the frozen old-core
//! command streams).
//! Capabilities this backend does not implement are simply absent — the
//! caps-trait DEFAULT bodies serve them, and those defaults were audited
//! byte-for-byte against the `Backend` defaults they replace
//! (`docs/runtime-v2-deletion-baseline.md` S2.1; 115 of this backend's
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
//! The embedder constructs the backend and calls [`start`]:
//!
//! 1. Monotonic time source (idempotent, first install wins).
//! 2. Registry: [`runtime_vocabulary::register_builtins`] + the app's
//!    `register` seam.
//! 3. Fresh [`World`]; build + [`realize`] inside `world.enter`.
//! 4. Entered buffered-microtask drain (no-op without a buffering
//!    scheduler; load-bearing under one).
//! 5. Single root → `caps::LifecycleOps::finish` (emits the `Finish { root }`
//!    wire op, matching the old-core mount).
//! 6. `world.flush()` commits anything staged during mount, so the
//!    first [`RokuBackend::drain`] carries the complete initial scene.
//! 7. Install the flush driver and retain
//!    `{Realized, backend, registry, world}` in [`NewCoreApp`].
//!
//! # Flush driver — the embedder contract
//!
//! The new core stages signal writes; nothing is observable until the
//! driver calls [`World::flush`]. Roku's ONLY author-callback dispatch
//! surface is the embedder invoking a [`crate::HandlerTable`] closure
//! (device event → `dispatch_unit`/`dispatch_string`/`dispatch_bool`/
//! `dispatch_float`). Every callback-taking caps impl below wraps the
//! author callback before delegating, so **the wrapped closure is what
//! lands in the `HandlerTable`** — the embedder's plain dispatch call
//! gets the flush scheduling for free. That covers the hook sites
//! completely: press (button + pressable), text-input / toggle /
//! slider `on_change`, portal `on_dismiss`, and the rarely-wired
//! surfaces (hover/touch/wheel/scroll/graphics/virtualizer) are all
//! wrapped at their single registration choke point; there is no other
//! path by which Roku author code runs. (Author code fired by a
//! *scheduler*, if the embedder installs one, is the one exception —
//! see [`crate::dispatch_hook`].)
//!
//! **Embedder contract**: after dispatching device events (handler
//! invocations), call [`settle`] before draining the command queue.
//! That is the "input event → staged writes → flush → emitted
//! commands" boundary:
//!
//! ```text
//! device event → HandlerTable::dispatch_*()   (author code, staged writes)
//! newcore::settle()                           (drain microtasks + flush)
//! backend.drain()                             (ship the follow-up commands)
//! ```
//!
//! [`settle`] is safe in every scheduler configuration: with NO
//! scheduler installed, `schedule_microtask` runs synchronously off-web
//! so the wrapped callback's deduped flush already committed and
//! `settle` is a cheap no-op; with a buffering/queueing scheduler,
//! `settle` drains its buffered microtasks (entered, so creation-side
//! work has the ambient world) and then force-commits via
//! [`flush_sync`].
//!
//! # Viewport
//!
//! Roku has **no viewport report surface** — `RokuBackend` neither
//! implements a `set_viewport` nor writes the old-core viewport TLS
//! (the device's 1280×720 / 1920×1080 design space is the BrightScript
//! client's concern; the wire ships author intent, not resolved
//! layout). There is therefore no viewport sink to install here — the
//! world's viewport ctx keeps its default, exactly as the old core's
//! TLS default did. If a future wire op reports device resolution,
//! wire the sink beside it (see `backend_terminal::newcore` for the
//! shape).
//!
//! # Residual seams (named, none silent)
//!
//! - Navigators: `Element::Navigator` with an SDK presentation type
//!   routes through `Backend::create_navigator` exactly as before —
//!   which on Roku is the trait's `unimplemented!()` default on BOTH
//!   cores (the module docs on [`crate`] document the gap).
//! - There is no live device/thin-client: the serialized command
//!   stream IS the observable output, which is why the parity tests
//!   compare JSON bytes rather than pixels.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives;
use runtime_shared::{
    Action, Color, StyleRules,
};
use runtime_scene::{realize, Element, Host, Realized, Registry};
use runtime_vocabulary::caps;
use runtime_world::World;
use crate::command::{self, RokuCommand, SignalId, WireColor, WireStyle};
use crate::style;
use crate::inspect_simple_text_row;
use runtime_shared::primitives::activity_indicator::ActivityIndicatorSize;
use runtime_shared::primitives::icon::IconData;
use runtime_vocabulary::caps::WireBindingOps as _;

use crate::{NodeId, RokuBackend};

// Re-exported so embedders can name the boot-path types without a
// direct runtime-scene dependency — mirrors `backend_terminal::newcore`.
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
    static BACKEND: RefCell<Option<Weak<RefCell<RokuBackend>>>> = const { RefCell::new(None) };
}

/// Everything the boot path must keep alive. Field order is drop order:
/// the realized tree unmounts before the world (its slots' owner) dies.
/// The embedder holds this value for the whole session and calls
/// [`NewCoreApp::stop`] on shutdown.
pub struct NewCoreApp {
    realized: Realized<NodeId>,
    _backend: Rc<RefCell<RokuBackend>>,
    _registry: Rc<Registry<RokuBackend>>,
    world: World,
}

impl std::fmt::Debug for NewCoreApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewCoreApp").finish_non_exhaustive()
    }
}

impl NewCoreApp {
    /// Borrow the live tree (tests, diagnostics).
    pub fn with_realized<R>(&self, f: impl FnOnce(&Realized<NodeId>) -> R) -> R {
        f(&self.realized)
    }

    /// The mounted world (tests can flush it explicitly).
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Unmount: drops the `Realized` (cleanups fire), uninstalls the
    /// flush driver, and drops the world. Call while the mounting
    /// thread's TLS is intact (same rationale as the old host's
    /// explicit `Owner` drop). A late `HandlerTable` dispatch or
    /// [`settle`] after `stop` is a safe no-op — the flush driver's
    /// world slot is cleared before the reactive teardown runs.
    pub fn stop(self) {
        crate::dispatch_hook::clear_dispatch_hook();
        set_flush_world(None);
        BACKEND.with(|b| *b.borrow_mut() = None);
        drop(self);
    }
}

/// Mount a new-core element tree into an already-constructed backend.
///
/// The embedder must have constructed the backend; a scheduler is
/// OPTIONAL (see the module docs — without one, flushes commit
/// synchronously inside the wrapped callbacks and [`settle`] is a
/// cheap fence).
///
/// `register` runs after [`runtime_vocabulary::register_builtins`], so
/// apps/SDKs can register their own payload handlers on the same
/// registry before the tree realizes. The build closure runs inside
/// `world.enter`, so free `signal()`/`effect()` calls work; top-level
/// creations are world-root-owned (they live until [`NewCoreApp::stop`]).
#[inline]
pub fn start(
    backend: Rc<RefCell<RokuBackend>>,
    register: impl FnOnce(&mut Registry<RokuBackend>),
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
    backend: Rc<RefCell<RokuBackend>>,
    register: R,
    build: B,
) -> NewCoreApp where
    S: runtime_vocabulary::BuiltinSet,
    R: FnOnce(&mut Registry<RokuBackend>),
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
    let mut registry: Registry<RokuBackend> = Registry::new();
    runtime_vocabulary::register_builtins_with::<_, S>(&mut registry);
    register(&mut registry);
    let registry = Rc::new(registry);

    let world = World::new();
    let realized = world.enter(|| {
        let element = build();
        realize(&backend, &registry, element)
        // No viewport-ctx capture: Roku has no viewport report surface
        // (module docs) — there is nothing that would ever push into
        // the ctx's size signal.
    });

    // Buffered-microtask drain — a no-op without a buffering
    // scheduler, load-bearing under one (e.g. the parity tests'
    // queue scheduler). Must run with NO backend borrow held (drained
    // tasks re-borrow); ENTERED because a buffered task may do
    // creation-side work.
    world.enter(runtime_shared::scheduling::drain_buffered_microtasks);

    // Single-root contract, matching the old-core mount (the wire's
    // `Finish { root }` op names exactly one application root).
    let mut roots = realized.collect_nodes();
    let root = match roots.len() {
        1 => roots.pop().expect("len checked"),
        n => panic!(
            "backend_roku::newcore::start: the app root must contribute exactly one \
             top-level node (got {n}) — wrap fragment/multi-root trees in a view"
        ),
    };
    caps::LifecycleOps::finish(&mut *backend.borrow_mut(), root);

    // Commit anything staged during mount so the first `drain()`
    // carries the complete initial scene.
    world.flush();

    // Install the flush driver: schedule_flush becomes reachable from
    // (a) the author-callback wrappers in the caps impls below and
    // (b) an embedder scheduler's post-dispatch hook.
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
pub fn with_backend<R>(f: impl FnOnce(&Rc<RefCell<RokuBackend>>) -> R) -> Option<R> {
    let rc = BACKEND.with(|b| b.borrow().as_ref().and_then(Weak::upgrade));
    rc.map(|rc| f(&rc))
}

// ===========================================================================
// Flush driver
// ===========================================================================

/// Queue one flush of the mounted world on the framework microtask
/// queue (deduped). Safe to call any time; a no-op before [`start`].
/// The author-callback wrappers (and an embedder scheduler's dispatch
/// hook) call this right after author-visible dispatch.
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
/// [`start`]). The embedder-facing fence [`settle`] ends with this.
pub fn flush_sync() {
    flush_now();
}

/// The embedder's post-dispatch fence: after invoking `HandlerTable`
/// closures for a batch of device events, call this BEFORE draining
/// the command queue. Drains any scheduler-buffered microtasks with
/// the mounted world ambient (a buffered task may do creation-side
/// work — realize a `dyn` branch, mount keyed rows), then commits all
/// staged writes. Safe in every scheduler configuration and after
/// [`NewCoreApp::stop`] (dead-world no-op); see the module docs for
/// the full contract.
pub fn settle() {
    enter_mounted_world(runtime_shared::scheduling::drain_buffered_microtasks);
    flush_sync();
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
// pay a flush per event. On Roku the wrapped closure is what the caps
// impl hands to `Backend::create_*` — i.e. what lands in the
// `HandlerTable` — so the embedder's plain `dispatch_*` call is covered
// with no embedder-side glue.

/// Wrap a zero-arg author callback (`on_press`, portal `on_dismiss`, …).
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

impl Host for RokuBackend {
    type Node = NodeId;

    fn insert(&mut self, parent: &mut Self::Node, child: Self::Node) {
        self.push(RokuCommand::Insert {
            parent: *parent,
            child,
        });
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
        self.push(RokuCommand::ClearChildren { parent: *node });
    }

    fn create_anchor(&mut self) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateReactiveAnchor { id });
        id
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

impl caps::AppEnvOps for RokuBackend {
    fn platform(&self) -> runtime_shared::Platform {
        runtime_shared::Platform::Roku
    }
}

impl caps::LifecycleOps for RokuBackend {
    fn finish(&mut self, root: Self::Node) {
        self.push(RokuCommand::Finish { root });
    }
}

// ---------------------------------------------------------------------------
// View + input + pressable
// ---------------------------------------------------------------------------

impl caps::ViewOps for RokuBackend {
    fn create_view(
        &mut self,
        _a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateView { id });
        id
    }
}

impl caps::InputOps for RokuBackend {}

impl caps::PressableOps for RokuBackend {
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, _a11y: &AccessibilityProps) -> Self::Node {
        // Dispatch-site glue: the wrapped closure is what
        // `create_pressable` registers in the HandlerTable, so the
        // embedder's `dispatch_unit(id)` gets the flush for free.
        let on_click = flushing0(on_click);
        let id = self.mint_node();
        let handler = self.mint_handler();
        self.handlers.borrow_mut().unit.push((handler, on_click));
        self.push(RokuCommand::CreatePressable {
            id,
            on_click: handler,
        });
        id
    }
}

// ---------------------------------------------------------------------------
// Text + button
// ---------------------------------------------------------------------------

impl caps::TextOps for RokuBackend {
    fn create_text(
        &mut self,
        content: &str,
        _a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateText {
            id,
            content: content.to_string(),
        });
        id
    }

    fn update_text(&mut self, node: &Self::Node, content: &str) {
        self.push(RokuCommand::UpdateText {
            id: *node,
            content: content.to_string(),
        });
    }
}

impl caps::ButtonOps for RokuBackend {
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        leading_icon: Option<&primitives::icon::IconData>,
        trailing_icon: Option<&primitives::icon::IconData>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: wrap the Action's runtime evaluator (the
        // closure `create_button` registers in the HandlerTable); the
        // serialization metadata (method/inputs/output → BindButton
        // wire op) passes through untouched, keeping the emitted
        // command stream byte-identical to the old core.
        let on_click = Action {
            method: on_click.method,
            inputs: on_click.inputs.clone(),
            initial: on_click.initial.clone(),
            output: on_click.output,
            fire: flushing0(on_click.fire.clone()),
        };
        let on_click = &on_click;
        let id = self.mint_node();
        let handler = self.mint_handler();
        // Roku has no host runtime to evaluate the closure; we ship
        // the structured metadata (method + signal ids + optional
        // output signal) as a `BindButton` wire op below. The
        // closure itself is still registered in the handler table
        // so a host-side runtime-server shell (dev mode) can fire it; in
        // baked-binary builds the device's transpiled #[method]
        // does the work and the closure is dead weight.
        self.handlers
            .borrow_mut()
            .unit
            .push((handler, on_click.fire.clone()));
        let leading = leading_icon.map(|d| self.lower_icon(d));
        let trailing = trailing_icon.map(|d| self.lower_icon(d));
        self.push(RokuCommand::CreateButton {
            id,
            label: label.to_string(),
            on_click: handler,
            leading_icon: leading,
            trailing_icon: trailing,
        });
        // Carry the structured metadata onto the wire if the Action
        // has any (i.e. came from a `#[method]`-backed handler). An
        // opaque Action (closure with empty method) skips this —
        // generator backends can't ship a nameless handler.
        if !on_click.is_opaque() {
            // Declare each input signal first so the device has a
            // value to read at dispatch time.
            for (sid, val) in on_click.inputs.iter().zip(on_click.initial.iter()) {
                self.note_signal_initial(*sid, val);
            }
            self.push(RokuCommand::BindButton {
                button_id: id,
                input_signal_ids: on_click.inputs.iter().map(|i| SignalId(*i)).collect(),
                method: on_click.method.to_string(),
                output_signal_id: on_click.output.map(SignalId),
            });
        }
        id
    }

    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        self.push(RokuCommand::UpdateButtonLabel {
            id: *node,
            label: label.to_string(),
        });
    }
}

// ---------------------------------------------------------------------------
// Image + icon + link
// ---------------------------------------------------------------------------

impl caps::ImageOps for RokuBackend {
    fn create_image(
        &mut self,
        src: &str,
        alt: Option<&str>,
        _a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        self.push(RokuCommand::CreateImage {
            id,
            src: src.to_string(),
            alt: alt.map(|s| s.to_string()),
        });
        id
    }

    fn update_image_src(&mut self, node: &Self::Node, src: &str) {
        self.push(RokuCommand::UpdateImageSrc {
            id: *node,
            src: src.to_string(),
        });
    }
}

impl caps::IconOps for RokuBackend {
    fn create_icon(
        &mut self,
        data: &IconData,
        color: Option<&Color>,
        _a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        let wire = self.lower_icon(data);
        self.push(RokuCommand::CreateIcon {
            id,
            data: wire,
            color: color.map(|c| WireColor::literal(c.0.clone())),
        });
        id
    }

    fn update_icon_color(&mut self, node: &Self::Node, color: &Color) {
        self.push(RokuCommand::UpdateIconColor {
            id: *node,
            color: WireColor::literal(color.0.clone()),
        });
    }
}

impl caps::LinkOps for RokuBackend {}

// ---------------------------------------------------------------------------
// Form widgets
// ---------------------------------------------------------------------------

impl caps::TextInputOps for RokuBackend {
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
        // The wrapped on_change is what lands in the HandlerTable's
        // string slot — the embedder's dispatch_string covers it.
        let on_change = flushing1(on_change);
        let _on_key_down = on_key_down.map(flushing_key);
        let _on_blur = on_blur.map(|f| -> primitives::text_input::BlurHandler {
                Rc::new(move || {
                    let outcome = f();
                    schedule_flush();
                    outcome
                })
            });
        // `_on_key_down` is unused on Roku — the SceneGraph keyboard
        // surface doesn't expose pre-default key interception in the
        // way Web/UIKit/Android do. Document explicitly so the
        // asymmetry is visible at the API boundary.
        let id = self.mint_node();
        let handler = self.mint_handler();
        self.handlers.borrow_mut().string.push((handler, on_change));
        self.push(RokuCommand::CreateTextInput {
            id,
            initial_value: initial_value.to_string(),
            placeholder: placeholder.map(|s| s.to_string()),
            secure,
            on_change: handler,
        });
        id
    }

    fn update_text_input_value(&mut self, node: &Self::Node, value: &str) {
        self.push(RokuCommand::UpdateTextInputValue {
            id: *node,
            value: value.to_string(),
        });
    }
}

impl caps::ToggleOps for RokuBackend {
    fn create_toggle(
        &mut self,
        initial_value: bool,
        on_change: Rc<dyn Fn(bool)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // The wrapped on_change lands in the HandlerTable's bool slot.
        let on_change = flushing1(on_change);
        let id = self.mint_node();
        let handler = self.mint_handler();
        self.handlers.borrow_mut().bool_.push((handler, on_change));
        self.push(RokuCommand::CreateToggle {
            id,
            initial_value,
            on_change: handler,
        });
        id
    }

    fn update_toggle_value(&mut self, node: &Self::Node, value: bool) {
        self.push(RokuCommand::UpdateToggleValue { id: *node, value });
    }
}

impl caps::SliderOps for RokuBackend {
    fn create_slider(
        &mut self,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // The wrapped on_change lands in the HandlerTable's float slot.
        let on_change = flushing1(on_change);
        let id = self.mint_node();
        let handler = self.mint_handler();
        self.handlers.borrow_mut().float.push((handler, on_change));
        self.push(RokuCommand::CreateSlider {
            id,
            initial_value,
            min,
            max,
            step,
            on_change: handler,
        });
        id
    }

    fn update_slider_value(&mut self, node: &Self::Node, value: f32) {
        self.push(RokuCommand::UpdateSliderValue { id: *node, value });
    }
}

impl caps::ActivityIndicatorOps for RokuBackend {
    fn create_activity_indicator(
        &mut self,
        size: ActivityIndicatorSize,
        color: Option<&Color>,
        _a11y: &runtime_shared::accessibility::AccessibilityProps,
    ) -> Self::Node {
        let id = self.mint_node();
        let wire_size = match size {
            ActivityIndicatorSize::Small => command::ActivityIndicatorSize::Small,
            ActivityIndicatorSize::Large => command::ActivityIndicatorSize::Large,
        };
        self.push(RokuCommand::CreateActivityIndicator {
            id,
            size: wire_size,
            color: color.map(|c| WireColor::literal(c.0.clone())),
        });
        id
    }
}

// ---------------------------------------------------------------------------
// Scroll + safe area + virtualizer
// ---------------------------------------------------------------------------

impl caps::ScrollOps for RokuBackend {
    fn create_scroll_view(
        &mut self,
        horizontal: bool,
        on_scroll: Option<Rc<dyn Fn(f32, f32)>>,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: Roku's create_scroll_view currently drops
        // on_scroll (no wire op), but wrap anyway so the delegation
        // stays mechanically uniform if the wire grows a scroll report.
        let on_scroll = on_scroll.map(|f| -> Rc<dyn Fn(f32, f32)> {
            Rc::new(move |x, y| {
                f(x, y);
                schedule_flush();
            })
        });
        let _on_scroll = on_scroll;
        let id = self.mint_node();
        self.push(RokuCommand::CreateScrollView { id, horizontal });
        id
    }
}

impl caps::SafeAreaOps for RokuBackend {}

impl caps::VirtualizerOps for RokuBackend {}

// ---------------------------------------------------------------------------
// Graphics + portal + presence + navigator
// ---------------------------------------------------------------------------

impl caps::GraphicsOps for RokuBackend {
    fn create_graphics(
        &mut self,
        on_ready: primitives::graphics::OnReady,
        on_resize: primitives::graphics::OnResize,
        on_lost: primitives::graphics::OnLost,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // Dispatch-site glue: surface lifecycle callbacks run author
        // code (Roku never fires them — no GPU surface, the Backend
        // impl drops them — but the wrap keeps the delegation
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
        let id = self.mint_node();
        self.push(RokuCommand::CreateView { id });
        id
    }
}

impl caps::PortalOps for RokuBackend {
    fn create_portal(
        &mut self,
        target: primitives::portal::PortalTarget,
        on_dismiss: Option<Rc<dyn Fn()>>,
        trap_focus: bool,
        _a11y: &AccessibilityProps,
    ) -> Self::Node {
        // The wrapped on_dismiss lands in the HandlerTable's unit slot.
        let on_dismiss = on_dismiss.map(flushing0);
        use runtime_shared::primitives::portal as p;
        let id = self.mint_node();
        let on_dismiss_handler = on_dismiss.map(|cb| {
            let h = self.mint_handler();
            self.handlers.borrow_mut().unit.push((h, cb));
            h
        });
        let wire_target = match target {
            p::PortalTarget::Viewport(placement) => command::WirePortalTarget::Viewport {
                placement: match placement {
                    p::ViewportPlacement::Center => command::WireViewportPlacement::Center,
                    p::ViewportPlacement::Top => command::WireViewportPlacement::Top,
                    p::ViewportPlacement::Bottom => command::WireViewportPlacement::Bottom,
                    p::ViewportPlacement::Left => command::WireViewportPlacement::Left,
                    p::ViewportPlacement::Right => command::WireViewportPlacement::Right,
                    p::ViewportPlacement::FullScreen => {
                        command::WireViewportPlacement::FullScreen
                    }
                },
            },
            p::PortalTarget::Anchor { side, align, offset, .. } => {
                // No live anchor-rect signal yet — the Roku runtime
                // applies the side/align/offset hints against
                // whatever the composition lays down. Carrying a
                // sentinel id (0) tells the BS client this binding
                // is static; revisit once `AnchorTarget` exposes its
                // backing signal id to generator backends.
                command::WirePortalTarget::Anchor {
                    anchor_rect_signal_id: SignalId(0),
                    side: match side {
                        p::ElementSide::Above => command::WireElementSide::Above,
                        p::ElementSide::Below => command::WireElementSide::Below,
                        p::ElementSide::Start => command::WireElementSide::Start,
                        p::ElementSide::End => command::WireElementSide::End,
                    },
                    align: match align {
                        p::ElementAlign::Start => command::WireElementAlign::Start,
                        p::ElementAlign::Center => command::WireElementAlign::Center,
                        p::ElementAlign::End => command::WireElementAlign::End,
                    },
                    offset,
                }
            }
            p::PortalTarget::Named(slot) => command::WirePortalTarget::Named {
                slot: slot.to_string(),
            },
        };
        self.push(RokuCommand::CreatePortal {
            id,
            target: wire_target,
            on_dismiss: on_dismiss_handler,
            trap_focus,
        });
        id
    }
}

impl caps::PresenceOps for RokuBackend {}

impl caps::NavigatorOps for RokuBackend {}

// ---------------------------------------------------------------------------
// External + document
// ---------------------------------------------------------------------------

impl caps::ExternalOps for RokuBackend {}

impl caps::DocumentOps for RokuBackend {}

// ---------------------------------------------------------------------------
// Style + assets
// ---------------------------------------------------------------------------

impl caps::StyleOps for RokuBackend {
    fn apply_style(&mut self, node: &Self::Node, style: &Rc<StyleRules>) {
        // Degrade LOUDLY, once. `Roku` has no scrolling gesture model,
        // so `Position::Sticky` renders as `Relative` and
        // `overscroll-behavior` has nothing to govern. Both were
        // previously dropped in silence — the exact "no warning,
        // nothing to grep for" failure `runtime_shared::unsupported`
        // exists to end.
        if matches!(style.position, Some(runtime_shared::Position::Sticky)) {
            runtime_shared::unsupported::warn_once(
                "roku.sticky",
                "position: Sticky on the Roku backend — rendered as Relative (this backend \
                 has no scroll gesture model). Web and the native backends pin.",
            );
        }
        if style.overscroll_behavior.is_some() {
            runtime_shared::unsupported::warn_once(
                "roku.overscroll_behavior",
                "overscroll-behavior on the Roku backend — ignored (no scroll gesture \
                 model to govern).",
            );
        }
        let wire = style::lower_style(style);
        self.push(RokuCommand::ApplyStyle {
            id: *node,
            style: Box::new(wire),
        });
    }

    fn apply_styled_states(
        &mut self,
        node: &Self::Node,
        base: &Rc<StyleRules>,
        overlays: &[(runtime_shared::StateBits, Rc<StyleRules>)],
    ) {
        // Find the overlay (if any) for each well-known state.
        // The framework hands us a list, not a map, so we scan
        // once per state.
        let find = |target: runtime_shared::StateBits| -> Option<Box<WireStyle>> {
            overlays
                .iter()
                .find(|(bits, _)| *bits == target)
                .map(|(_, rules)| Box::new(style::lower_style(rules)))
        };

        self.push(RokuCommand::ApplyStyleStates {
            id: *node,
            base: Box::new(style::lower_style(base)),
            hovered: find(runtime_shared::StateBits::HOVERED),
            focused: find(runtime_shared::StateBits::FOCUSED),
            pressed: find(runtime_shared::StateBits::PRESSED),
            disabled: find(runtime_shared::StateBits::DISABLED),
        });
    }

    fn handles_states_natively(&self) -> bool {
        // Same posture as the web backend: the framework hands us
        // the base rules plus per-state overlays declaratively, and
        // we ship them through a single wire command. The Roku-side
        // runtime maintains its own focus/press state (driven by
        // D-pad input) and applies the right merged style locally —
        // no Rust round-trip per state change.
        true
    }

    fn install_tokens(&mut self, _tokens: &[runtime_shared::TokenEntry]) {
        // No-op (matches iOS / Android posture).
        //
        // The Roku wire protocol has no runtime variable layer — there is no
        // analog of CSS custom properties on SceneGraph. Styles are lowered
        // through `style::lower_style` at every `apply_style` call, and any
        // `Tokenized<T>` field has already been read via `Tokenized::value()`
        // by then, producing a literal `WireColor` / `WireLength` / number in
        // the emitted `ApplyStyle` command.
        //
        // When the app calls `update_tokens(...)`, the framework's
        // tokens-version signal re-fires every styled effect that subscribed
        // to any of the changed tokens; each of those effects calls
        // `apply_style` again with freshly-resolved literal values. So the
        // wire stream picks up the new values automatically — this method
        // doesn't need to emit anything.
        //
        // Previously this panicked via `unimplemented!()`, breaking any app
        // that touched the token system on Roku (theme switching, custom
        // tokens). The earlier comment referenced a removed
        // `register_theme_variant` hook; the framework moved on to a
        // re-apply-driven model, so the no-op is now the correct behavior.
    }

    fn update_tokens(&mut self, _tokens: &[runtime_shared::TokenEntry]) {
        // See `install_tokens` above — same no-op rationale. Updated token
        // values propagate to the wire via re-application of every styled
        // effect that subscribed to a changed token.
    }

    fn set_disabled(&mut self, node: &Self::Node, disabled: bool) {
        self.push(RokuCommand::SetDisabled {
            id: *node,
            disabled,
        });
    }
}

impl caps::AssetOps for RokuBackend {}

// ---------------------------------------------------------------------------
// A11y + animation + introspection
// ---------------------------------------------------------------------------

impl caps::A11yOps for RokuBackend {}

impl caps::AnimationOps for RokuBackend {}

impl caps::IntrospectionOps for RokuBackend {}

// ---------------------------------------------------------------------------
// Batch + wire bindings
// ---------------------------------------------------------------------------

impl caps::BatchOps for RokuBackend {}

impl caps::WireBindingOps for RokuBackend {
    fn note_text_binding(
        &mut self,
        node: &Self::Node,
        signal_ids: &[u64],
        method: &'static str,
    ) {
        // The walker hands us a `TextSource::Bound` after the
        // `create_text` step; we round-trip the binding into the
        // wire stream so the device-side runtime can subscribe the
        // Label to the signals and apply the transformer on every
        // change. The subsequent Effect will still fire once at
        // snapshot time and emit a redundant `UpdateText` — that's
        // a one-line wire dup with the same string the BindText's
        // initial subscriber-fire would produce anyway, so it's a
        // visual no-op. Worth optimizing later if wire size matters.
        self.push(RokuCommand::BindText {
            node_id: *node,
            signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
            method: method.to_string(),
        });
    }

    fn note_signal_initial(
        &mut self,
        signal_id: u64,
        value: &runtime_shared::__serde_json::Value,
    ) {
        // First-time signal observation: declare the signal to the
        // device with its current value. Subsequent observations of
        // the same id are dropped — the value lives in the BS-side
        // arena once it's been seeded; later mutations come from
        // button actions on the device, not from the framework's
        // snapshot. Without dedup, every structured binding that names the
        // same signal would emit a redundant CreateSignal and reset
        // it back to its initial each time.
        if self.created_signals.insert(signal_id) {
            // Bypass `push` — signals are global. If we routed this
            // through `push` and a nested bind happened to be capturing
            // when its inner signal was first declared, the
            // CreateSignal would land in a slot buffer and get
            // re-emitted on every slot replay, clobbering the signal's
            // current value.
            self.commands.push(RokuCommand::CreateSignal {
                id: SignalId(signal_id),
                initial: value.clone(),
            });
        }
    }

    fn note_when_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        then_node: &Self::Node,
        otherwise_node: &Self::Node,
    ) {
        let then_slot = command::Slot {
            root_node_id: *then_node,
            commands: self.take_captured_slot(*then_node),
        };
        let otherwise_slot = command::Slot {
            root_node_id: *otherwise_node,
            commands: self.take_captured_slot(*otherwise_node),
        };
        self.push(RokuCommand::BindWhen {
            anchor_id: *anchor,
            signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
            cond_method: cond_method.to_string(),
            then_slot,
            otherwise_slot,
        });
    }

    fn note_switch_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        cond_method: &'static str,
        arms: &[(runtime_shared::__serde_json::Value, Self::Node)],
        default_node: &Self::Node,
    ) {
        let arms_wire: Vec<command::SwitchArm> = arms
            .iter()
            .map(|(pat, node)| command::SwitchArm {
                pattern: pat.clone(),
                slot: command::Slot {
                    root_node_id: *node,
                    commands: self.take_captured_slot(*node),
                },
            })
            .collect();
        let default_slot = command::Slot {
            root_node_id: *default_node,
            commands: self.take_captured_slot(*default_node),
        };
        self.push(RokuCommand::BindSwitch {
            anchor_id: *anchor,
            signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
            cond_method: cond_method.to_string(),
            arms: arms_wire,
            default_slot,
        });
    }

    fn note_repeat_binding(
        &mut self,
        anchor: &Self::Node,
        signal_ids: &[u64],
        count_method: &'static str,
        row_template: &Self::Node,
        row_index_signal_id: Option<u64>,
    ) {
        let row_template = command::Slot {
            root_node_id: *row_template,
            commands: self.take_captured_slot(*row_template),
        };
        self.push(RokuCommand::BindRepeat {
            anchor_id: *anchor,
            signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
            count_method: count_method.to_string(),
            row_template,
            row_index_signal_id: row_index_signal_id.map(SignalId),
        });
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
        let row_template = command::Slot {
            root_node_id: *row_template,
            commands: self.take_captured_slot(*row_template),
        };
        // Inspect the slot. Today we only lower row templates that
        // are structurally one Text node with one BindText (and any
        // ApplyStyle/UpdateText decoration). Anything else falls
        // back to the existing BindRepeat path so the framework
        // stays correct on Roku while we grow MarkupList coverage
        // primitive-by-primitive.
        if let Some(dynamic_fields) = inspect_simple_text_row(
            &row_template,
            row_index_signal_id,
        ) {
            // Component name is keyed on the anchor's id — anchors
            // are unique per virtualizer in the snapshot, so this
            // produces a stable, unique name build-roku can use to
            // emit the .xml/.brs pair.
            let item_component = format!("IdealystListItem_{}", anchor.0);
            self.push(RokuCommand::CreateMarkupList {
                anchor_id: *anchor,
                item_component,
                count_method: count_method.to_string(),
                signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
                row_index_signal_id: row_index_signal_id.map(SignalId),
                dynamic_fields,
                row_template,
                // V1: hard-coded scroll-axis cell size. For
                // vertical lists this is row height; for
                // horizontal carousels we interpret it as the
                // row's height (cell width is then derived from
                // viewport / visibleItems). A future iteration
                // should read this from the row template's style
                // (height for vertical, width for horizontal).
                item_size: 200.0,
                horizontal,
            });
        } else {
            // Generic row template — fall back to the BindRepeat
            // path (the device-side replay machinery handles
            // arbitrary row shapes).
            self.push(RokuCommand::BindRepeat {
                anchor_id: *anchor,
                signal_ids: signal_ids.iter().map(|id| SignalId(*id)).collect(),
                count_method: count_method.to_string(),
                row_template,
                row_index_signal_id: row_index_signal_id.map(SignalId),
            });
        }
    }

    fn supports_lazy_slot_capture(&self) -> bool {
        true
    }

    fn begin_slot_capture(&mut self) {
        self.capture_stack.push(Vec::new());
    }

    fn end_slot_capture(&mut self, slot_root: &Self::Node) {
        // Walker is expected to balance begin/end calls. Popping
        // without a matching begin would mean the walker has a bug
        // — error loudly rather than silently swallow the slot.
        let buf = self
            .capture_stack
            .pop()
            .expect("end_slot_capture without matching begin_slot_capture");
        self.captured_slots.insert(*slot_root, buf);
    }
}

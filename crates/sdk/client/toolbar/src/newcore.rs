//! New-core surface — the scene-registry leg (idea-lite migration).
//!
//! The old-core leg is an `Element::External` payload dispatched through
//! the per-backend `ExternalRegistry`; that registry does not exist on
//! the new core. The scene [`Registry`] is the new core's unified
//! primitive==external contract, so this leg registers a payload handler
//! there instead:
//!
//! - **macOS** — [`register`] against `Registry<MacosBackend>` installs
//!   the `MacosBackend`-concrete handler in `macos_newcore`, which
//!   reuses the shared NSToolbar machinery (`macos_shared`): NSToolbar
//!   built + attached to the host window exactly as the old handler
//!   did, the reactive `items` closure driven through
//!   `runtime_world::effect`, every button `on_click` wrapped so it
//!   calls `backend_macos::newcore::schedule_flush` after the author
//!   code returns (NSToolbar target-action fires outside the new
//!   core's wrapped dispatch sites), and the same 0-size in-tree
//!   placeholder view returned.
//! - **Everywhere else** — [`register`] installs the frozen
//!   External-placeholder degradation path
//!   ([`ExternalOps::create_external`]), mirroring the old walker's
//!   posture for a backend with no registered handler (author style +
//!   ref fill + `release_external` teardown all still flow, exactly
//!   like `walker/external.rs`) — and matching the old-core
//!   non-desktop posture where the `items` closure is never evaluated.
//!
//! # One `register`, resolved at registration time
//!
//! Unlike svg (whose concrete arm is wasm32-only, a target where the
//! generic arm can never be exercised), toolbar's concrete target IS
//! the dev host: a macOS build must serve BOTH `Registry<MacosBackend>`
//! (the real NSToolbar handler, via `host_appkit::newcore::run_with`)
//! and `Registry<HostMock>` (the placeholder arm, exercised by
//! tests/newcore.rs). A cfg-split pair of same-named `register` fns
//! cannot express that, so [`register`] stays generic on every target
//! and type-dispatches ONCE at registration: on macOS it downcasts
//! `&mut Registry<H>` to `&mut Registry<MacosBackend>` (`H: 'static`
//! makes the registry `Any`) and installs the concrete handler on hit;
//! every other `H` — and every other target — gets the placeholder
//! handler. Mount-path cost: zero (the dispatch happens before any
//! element exists).
//!
//! Same authored call shape as the old-core leg:
//! `Toolbar(ToolbarProps { .. }).bind(r)` then element coercion — a
//! consuming app compiles the same source against either core. App
//! bootstrap passes [`register`] as the boot registration seam:
//! `host_appkit::newcore::run_with(build, opts, |registry|
//! toolbar::register(registry))` (there is no inventory
//! self-registration on the new core; the registry is built explicitly
//! at boot).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::Ref;
use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

pub use crate::items::{ToolbarButton, ToolbarItem};

// ============================================================================
// Public API surface — same shapes as the old core.
// ============================================================================

/// Author-supplied props for a `Toolbar` instance. Same field shapes as
/// the old core; carried inside the scene item payload and read back by
/// the registered handler.
///
/// `items` is reactive: the macOS handler subscribes via a world effect
/// and rebuilds the native toolbar's item list whenever the signals
/// captured by the closure change.
///
/// (No `IdealystSchema` derive on this leg — the MCP catalog documents
/// the old-core surface, per the svg/codeblock/table precedent.)
pub struct ToolbarProps {
    /// Reactive item list. Re-evaluated whenever its captured signals
    /// change; the result is diffed against the current toolbar and
    /// applied via the native toolbar's "set items" call.
    pub items: Box<dyn Fn() -> Vec<ToolbarItem>>,
    /// Whether the toolbar is visible initially. Reactive visibility
    /// (driven by a signal) goes through `ToolbarHandle::set_visible`
    /// from an effect in the app — kept off the props struct to
    /// avoid two ways of doing the same thing.
    pub visible: bool,
}

impl Default for ToolbarProps {
    fn default() -> Self {
        Self {
            items: Box::new(Vec::new),
            visible: true,
        }
    }
}

// ============================================================================
// Handle + ops trait — same shapes as the old core.
// ============================================================================

/// Typed handle to a mounted `Toolbar`. Filled at mount time when the
/// author chained [`ToolbarBind::bind`]; user code receives the handle
/// through `Ref::with`.
#[derive(Clone)]
pub struct ToolbarHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn ToolbarOps,
}

impl ToolbarHandle {
    /// Wrap a type-erased native toolbar node + its backend ops vtable.
    /// Called by the handler's ref fill after the toolbar mounts; you
    /// don't construct this directly.
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ToolbarOps) -> Self {
        Self { node, ops }
    }

    /// Show or hide the toolbar. Maps to `NSToolbar.setVisible:` on
    /// macOS. No-op on backends without toolbar support.
    pub fn set_visible(&self, visible: bool) {
        self.ops.set_visible(&*self.node, visible);
    }
}

/// Imperative-ops dispatch. Same trait shape as the old core; the
/// active target's `OPS` static supplies the impl.
pub trait ToolbarOps: Sync {
    /// Show or hide the native toolbar represented by `node`. Backends
    /// downcast `node` to their concrete native type; the default is a
    /// silent no-op for backends that don't drive toolbar visibility.
    fn set_visible(&self, _node: &dyn Any, _visible: bool) {}
}

/// Fallback ops used on targets with no toolbar impl. Every method
/// is a silent no-op; user code keeps compiling but no native toolbar
/// is created.
pub struct UnsupportedOps;
impl ToolbarOps for UnsupportedOps {}

// On macOS the crate-level OPS is the shared NSToolbar ops (its
// `set_visible` downcasts to `MacosNode` internally, so a non-macOS
// node — e.g. host-mock's — degrades to the same silent no-op as
// `UnsupportedOps`).
#[cfg(target_os = "macos")]
static OPS: &dyn ToolbarOps = crate::macos_shared::OPS;
#[cfg(not(target_os = "macos"))]
static OPS: &dyn ToolbarOps = &UnsupportedOps;

// ============================================================================
// Payload + builder — the old `Bound<ToolbarHandle>` call shape
// (`.with_style(…)` / `.bind(…)` then element coercion).
// ============================================================================

/// Scene payload for the `Toolbar` item. Single-take slots (the
/// vocabulary `PrimCell` discipline, inlined): the scene hands the
/// handler a shared `&Rc<Self>`, but the style/ref-fill must move at
/// mount.
pub(crate) struct ToolbarPrim {
    pub(crate) props: Rc<ToolbarProps>,
    pub(crate) style: RefCell<Option<StyleProp>>,
    pub(crate) ref_fill: RefCell<Option<Box<dyn FnOnce(Rc<dyn Any>)>>>,
}

/// Author-side builder returned by [`Toolbar`] — mirrors the old-core
/// `Bound<ToolbarHandle>` call shape.
pub struct ToolbarBound {
    props: Rc<ToolbarProps>,
    style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(Rc<dyn Any>)>>,
}

/// Build a `Toolbar` primitive — the new-core stand-in for the
/// old-core `Toolbar(props)`, same call shape at every author site.
///
/// PascalCase intentionally — matches first-party primitive cadence
/// inside a `ui!` block. Interpolate as `{ toolbar::Toolbar(props) }`.
#[allow(non_snake_case)]
pub fn Toolbar(props: ToolbarProps) -> ToolbarBound {
    ToolbarBound {
        props: Rc::new(props),
        style: None,
        ref_fill: None,
    }
}

impl ToolbarBound {
    /// Attach the author style — lands on the in-tree placeholder node,
    /// exactly like `.with_style` on the old-core `Bound` (not a
    /// Toolbar idiom — the placeholder is deliberately 0-size — but the
    /// channel exists for parity).
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

/// Adds `.bind(r)` — same trait name/shape as the old core so author
/// imports (`use toolbar::prelude::*`) compile unchanged.
pub trait ToolbarBind {
    /// Bind a `Ref<ToolbarHandle>` for imperative access (e.g.
    /// `r.with(|h| h.set_visible(false))`). The ref fills once the
    /// toolbar mounts.
    fn bind(self, r: Ref<ToolbarHandle>) -> Self;
}

impl ToolbarBind for ToolbarBound {
    fn bind(mut self, r: Ref<ToolbarHandle>) -> Self {
        self.ref_fill = Some(Box::new(move |node_any| {
            r.fill(ToolbarHandle::new(node_any, OPS));
        }));
        self
    }
}

impl IntoElement for ToolbarBound {
    fn into_element(self) -> Element {
        item(
            ToolbarPrim {
                props: self.props,
                style: RefCell::new(self.style),
                ref_fill: RefCell::new(self.ref_fill),
            },
            Vec::new(),
        )
    }
}

/// Mirror of the old core's `From<Bound<H>> for Element`.
impl From<ToolbarBound> for Element {
    fn from(b: ToolbarBound) -> Element {
        b.into_element()
    }
}

/// One-stop import: same names as the old-core prelude.
pub mod prelude {
    pub use super::{
        Toolbar, ToolbarBind, ToolbarButton, ToolbarHandle, ToolbarItem, ToolbarProps,
    };
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail — the old walker's `external.rs` sequence after
/// node creation: (children are none for toolbar) → author style →
/// ref fill (type-erased node clone) → scope-tied `release_external`
/// (the old walker's `ExternalHandleCleanup` guard released EVERY
/// External on unmount, handler-backed or not — same here).
pub(crate) fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &ToolbarPrim)
where
    H: ExternalOps + StyleServices,
{
    if let Some(style) = prim.style.borrow_mut().take() {
        attach_style(backend, node, style);
    }
    if let Some(fill) = prim.ref_fill.borrow_mut().take() {
        let any_node: Rc<dyn Any> = Rc::new(node.clone());
        fill(any_node);
    }
    let backend = backend.clone();
    let node = node.clone();
    on_teardown(move || {
        backend.borrow_mut().release_external(&node);
    });
}

/// Placeholder handler for hosts with no native toolbar — the frozen
/// External degradation path (`create_external` with no registered
/// old-core handler renders each backend's "not supported" box). The
/// `items` closure is never evaluated, matching the old-core posture
/// on non-desktop targets (no handler → no effect → no items call).
fn mount_placeholder<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<ToolbarPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: ExternalOps + StyleServices,
{
    let backend = cx.backend().clone();
    let payload: Rc<dyn Any> = prim.props.clone();
    let node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<ToolbarProps>(),
        std::any::type_name::<ToolbarProps>(),
        &payload,
        &runtime_core::accessibility::AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}

/// Register the toolbar payload handler on a scene registry. Pass this
/// as the boot registration seam — on macOS:
/// `host_appkit::newcore::run_with(build, opts, |registry|
/// toolbar::register(registry))` installs the real NSToolbar handler;
/// against any other host (`backend_web::newcore::start_in`,
/// host-mock, …) the frozen External-placeholder path is installed.
/// See the module docs for why the dispatch is registration-time type
/// dispatch rather than a cfg-split pair.
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    #[cfg(target_os = "macos")]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_macos::MacosBackend>>() {
            reg.register::<ToolbarPrim, _>(crate::macos_newcore::mount_toolbar_macos);
            return;
        }
    }
    registry.register::<ToolbarPrim, _>(mount_placeholder::<H>);
}

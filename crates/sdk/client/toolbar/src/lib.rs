//! Third-party `Toolbar` SDK for the idealyst framework.
//!
//! Provides a `Toolbar` primitive that attaches to the host window's
//! chrome on native desktop hosts — title bar on macOS (`NSToolbar`),
//! Common-Controls toolbar on Windows, `GtkHeaderBar` on GTK4. On every
//! other platform (iOS, Android, web, terminal, wgpu, ESP, CPU)
//! [`register`] installs the External-placeholder handler and the
//! in-tree primitive renders zero-size.
//!
//! That posture follows the project's mobile-first philosophy
//! ([[feedback_mobile_first_philosophy]]): toolbar / menu chrome
//! belongs in third-party SDKs, not the host capability set.
//!
//! # Usage
//!
//! ```ignore
//! // App bootstrap: the boot entry's registration closure IS the seam.
//! host_appkit::newcore::run_with(
//!     app,
//!     host_appkit::RunOptions::default(),
//!     |registry| toolbar::register(registry),
//! )?;
//!
//! // Inside a `ui!` block — the toolbar's in-tree footprint is zero,
//! // so its position in the tree doesn't matter visually. Convention:
//! // mount near the root so the items closure is owned by a long-
//! // lived scope. `.into()` lifts each `ToolbarButton` builder into
//! // the enum so `vec![]` accepts mixed kinds (buttons + spacers).
//! let count = signal(0_i32);
//! ui! {
//!     view {
//!         { toolbar::Toolbar(toolbar::ToolbarProps {
//!             items: Box::new(move || vec![
//!                 toolbar::ToolbarItem::button("Save")
//!                     .icon("square.and.arrow.down")
//!                     .on_click({ let c = count.clone(); move || c.set(c.get() + 1) })
//!                     .into(),
//!                 toolbar::ToolbarItem::flexible_space(),
//!                 toolbar::ToolbarItem::button("Reload")
//!                     .on_click(|| log::info!("reload"))
//!                     .into(),
//!             ]),
//!             ..Default::default()
//!         }) }
//!         // ... rest of the app
//!     }
//! }
//! ```
//!
//! An UNREGISTERED payload panics at realize (the scene contract), so a
//! missed `register` fails loud.
#![deny(missing_docs)]

// Core-free item model (ToolbarItem/ToolbarButton + builders), shared by
// the platform legs.
mod items;

// Shared macOS NSToolbar machinery (delegate, item construction,
// wipe+repopulate update, placeholder view, imperative ops). Kept
// separate from `macos` so the AppKit code stays free of any reactive
// or scene imports.
#[cfg(target_os = "macos")]
mod macos_shared;

// Per-platform concrete scene handlers. Each is
// `Registry<ConcreteBackend>`-typed: window chrome has no caps-trait
// expression, so these cannot be caps-generic.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;

// Hosts must drain freshly-allocated Win32 command ids into the
// backend's handler map for toolbar buttons to become clickable — see
// the fn's docs. Windows-only, historically part of this crate's public
// surface on that target.
#[cfg(target_os = "windows")]
pub use crate::windows::flush_pending;

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::Ref;
use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

pub use crate::items::{ToolbarButton, ToolbarItem};

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for a `Toolbar` instance. Carried inside the
/// scene item payload and read back by the registered handler.
///
/// `items` is reactive: each desktop handler subscribes via a world
/// effect and rebuilds the native toolbar's item list whenever the
/// signals captured by the closure change.
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
// Handle + ops trait
// ============================================================================

/// Typed handle to a mounted `Toolbar`. Filled at mount time when the
/// author chained [`ToolbarBind::bind`]; user code receives the handle
/// through `Ref::with`.
#[derive(Clone)]
pub struct ToolbarHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn ToolbarOps,
}

/// Pointer identity on the NODE — a `ToolbarHandle` names one mounted `Toolbar`, so
/// clones of it are equal and handles onto two different `Toolbar`s never are.
/// Exactly the shape (and reasoning) of `form::FormHandle`'s impl.
///
/// `node` is a type-erased native element behind `Rc<dyn Any>`: the address
/// is all there is to compare, and it is the right thing to compare. `ops`
/// is excluded deliberately — it is the backend's single `&'static` vtable,
/// identical for every handle on a target, so it says nothing about WHICH
/// `Toolbar` this is.
///
/// Needed because `Signal<T>` is bounded on `T: PartialEq` at creation and
/// `get`, not just on the guarded `set`; an author stashing the bound handle
/// in state cannot add the impl themselves (orphan rule).
impl PartialEq for ToolbarHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.node, &other.node)
    }
}

impl Eq for ToolbarHandle {}

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

/// Imperative-ops dispatch. The active target's `OPS` static supplies
/// the impl.
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

// On a desktop target the crate-level OPS is that platform's real ops
// (each `set_visible` downcasts to its own node type internally, so a
// foreign node — e.g. host-mock's — degrades to the same silent no-op as
// `UnsupportedOps`).
#[cfg(target_os = "macos")]
static OPS: &dyn ToolbarOps = crate::macos_shared::OPS;
#[cfg(target_os = "windows")]
static OPS: &dyn ToolbarOps = crate::windows::OPS;
#[cfg(target_os = "linux")]
static OPS: &dyn ToolbarOps = crate::linux::OPS;
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
static OPS: &dyn ToolbarOps = &UnsupportedOps;

// ============================================================================
// Payload + builder — the `.with_style(…)` / `.bind(…)` chain then
// element coercion.
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

/// Author-side builder returned by [`Toolbar`].
pub struct ToolbarBound {
    props: Rc<ToolbarProps>,
    style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(Rc<dyn Any>)>>,
}

/// Build a `Toolbar` primitive.
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
    /// Attach the author style — lands on the in-tree placeholder node
    /// (not a Toolbar idiom — the placeholder is deliberately 0-size —
    /// but the channel exists for parity with other primitives).
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

/// Adds `.bind(r)` so `use toolbar::prelude::*` brings the imperative
/// binding into scope.
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

/// Element coercion for the constructor form.
impl From<ToolbarBound> for Element {
    fn from(b: ToolbarBound) -> Element {
        b.into_element()
    }
}

/// One-stop import for the author-facing names.
pub mod prelude {
    pub use super::{
        Toolbar, ToolbarBind, ToolbarButton, ToolbarHandle, ToolbarItem, ToolbarProps,
    };
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail, run after node creation: (children are none for
/// toolbar) → author style → ref fill (type-erased node clone) →
/// scope-tied `release_external` (every External is released on unmount,
/// handler-backed or not).
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

/// Placeholder handler for hosts with no native toolbar — the External
/// degradation path (`create_external` renders each host's "not
/// supported" box). The `items` closure is never evaluated: no native
/// toolbar means no effect, so no items call.
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
        &runtime_shared::accessibility::AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}

/// Register the toolbar payload handler on a scene registry. Pass this
/// as the boot registration seam — on macOS
/// (`host_appkit::newcore::run_with(build, opts, |registry|
/// toolbar::register(registry))`) it installs the real NSToolbar
/// handler, on Windows the Common-Controls toolbar, on GTK4 the
/// HeaderBar; against any other host (`backend_web::newcore::start_in`,
/// host-mock, …) the External-placeholder path.
///
/// # One `register`, resolved at registration time
///
/// A desktop build must serve BOTH its concrete backend registry (the
/// real native handler) and `Registry<HostMock>` (the placeholder arm,
/// exercised by `tests/toolbar.rs`) from the same target. A cfg-split
/// pair of same-named `register` fns cannot express that, so `register`
/// stays generic on every target and type-dispatches ONCE at
/// registration: it downcasts `&mut Registry<H>` to the platform's
/// concrete registry (`H: 'static` makes the registry `Any`) and
/// installs the native handler on hit; every other `H` gets the
/// placeholder handler. Mount-path cost: zero (the dispatch happens
/// before any element exists).
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    #[cfg(target_os = "macos")]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_macos::MacosBackend>>() {
            reg.register::<ToolbarPrim, _>(crate::macos::mount_toolbar_macos);
            return;
        }
    }
    #[cfg(target_os = "windows")]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_windows::WindowsBackend>>() {
            reg.register::<ToolbarPrim, _>(crate::windows::mount_toolbar_windows);
            return;
        }
    }
    #[cfg(target_os = "linux")]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_linux::LinuxBackend>>() {
            reg.register::<ToolbarPrim, _>(crate::linux::mount_toolbar_linux);
            return;
        }
    }
    registry.register::<ToolbarPrim, _>(mount_placeholder::<H>);
}

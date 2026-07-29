//! Old-core surface — `Element::External` payload + the per-backend
//! `ExternalRegistry`. Byte-moved from the crate root when the
//! `new-core` leg landed (see lib.rs); the default build re-exports
//! everything here unchanged. The item model ([`ToolbarItem`] /
//! [`ToolbarButton`]) is core-free and lives in the shared `items`
//! module, re-exported here so `toolbar::ToolbarItem` paths are
//! unchanged.
//!
//! # Architecture
//!
//! - The `Element::External` payload type is [`ToolbarProps`].
//! - Per-backend `register(&mut backend)` impls live in cfg-gated
//!   modules. The macOS impl installs an `effect!` inside its
//!   handler closure, so the `items` closure re-runs whenever the
//!   signals it reads change — same reactive shape as `webview::url`.
//! - [`ToolbarHandle`] carries a type-erased `Rc<dyn Any>` to the
//!   native toolbar object plus a `&'static dyn ToolbarOps` pointer
//!   the active backend module exposes. Imperative ops
//!   (`set_visible`) route through it.
//! - The in-tree node returned by the backend handler is a 0-size
//!   transparent view — toolbars are window chrome, not view content,
//!   so the placeholder is invisible regardless of where it's mounted.

use runtime_core::{Bound, Element, IdealystSchema, Ref, RefFill};
use std::any::{Any, TypeId};
use std::rc::Rc;

pub use crate::items::{ToolbarButton, ToolbarItem};

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for a `Toolbar` instance. Owned by the SDK,
/// not the framework — the framework just type-erases this behind
/// `Element::External { payload: Rc<dyn Any>, .. }` and hands it
/// back to the registered backend handler on mount.
///
/// `items` is reactive: the backend handler wraps the call in an
/// `Effect` and rebuilds the native toolbar's item list whenever the
/// signals captured by the closure change.
#[derive(IdealystSchema)]
pub struct ToolbarProps {
    /// Reactive item list. Re-evaluated whenever its captured signals
    /// change; the result is diffed against the current toolbar and
    /// applied via the native toolbar's "set items" call.
    #[schema(constraint = "reactive: re-runs when captured signals change")]
    pub items: Box<dyn Fn() -> Vec<ToolbarItem>>,
    /// Whether the toolbar is visible initially. Reactive visibility
    /// (driven by a signal) goes through `ToolbarHandle::set_visible`
    /// from an `effect!` in the app — kept off the props struct to
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

/// Typed handle to a mounted `Toolbar`. Filled by `Ref::fill` after
/// the primitive mounts; users hold a `Ref<ToolbarHandle>` at the
/// call site and reach imperative ops via `r.with(|h| h.set_visible(false))`.
#[derive(Clone)]
pub struct ToolbarHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn ToolbarOps,
}

impl ToolbarHandle {
    /// Wrap a type-erased native toolbar node + its backend ops vtable.
    /// Called by the backend's `RefFill` after the toolbar mounts; you
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

/// Imperative-ops dispatch. Implementations live in each cfg-gated
/// backend module and downcast `node` to their concrete native type.
/// Every method defaults to a no-op so a backend that hasn't wired
/// a particular op degrades silently.
///
/// `Sync` bound: the trait object lives in a `static OPS: &dyn
/// ToolbarOps` slot per backend module, which Rust requires to be
/// `Sync`. The ZST impls each backend ships are trivially `Sync`.
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

// ============================================================================
// Constructor + bind
// ============================================================================

/// Build a `Toolbar` primitive. Returns a typed `Bound<ToolbarHandle>`
/// so `.bind(...)` is type-checked against `Ref<ToolbarHandle>`.
///
/// PascalCase intentionally — matches first-party primitive cadence
/// inside a `ui!` block. Interpolate as `{ toolbar::Toolbar(props) }`.
///
/// Under the hood this is `Element::External` with a `ToolbarProps`
/// payload; on non-desktop backends the framework's "External not
/// registered" placeholder fires, but since the toolbar is window
/// chrome (not view content) the in-tree footprint stays invisible.
#[allow(non_snake_case)]
pub fn Toolbar(props: ToolbarProps) -> Bound<ToolbarHandle> {
    Bound::new(Element::External {
        type_id: TypeId::of::<ToolbarProps>(),
        type_name: std::any::type_name::<ToolbarProps>(),
        payload: Rc::new(props) as Rc<dyn Any>,
        children: Vec::new(),
        style: None,
        ref_fill: None,
        on_touch: None,
        on_hover: None,
        accessibility: runtime_core::accessibility::AccessibilityProps::default(),
    })
}

/// Adds `.bind(r)` to `Bound<ToolbarHandle>` via an extension trait
/// (the orphan rule blocks an inherent `impl` on the foreign `Bound`).
/// Bring this trait into scope to use the builder-style `.bind(...)`
/// on the value returned by [`Toolbar`].
pub trait ToolbarBind {
    /// Bind a `Ref<ToolbarHandle>` for imperative access (e.g.
    /// `r.with(|h| h.set_visible(false))`). The ref fills once the
    /// toolbar mounts.
    fn bind(self, r: Ref<ToolbarHandle>) -> Self;
}

impl ToolbarBind for Bound<ToolbarHandle> {
    fn bind(mut self, r: Ref<ToolbarHandle>) -> Self {
        if let Element::External { ref_fill, .. } = self.primitive_mut() {
            *ref_fill = Some(RefFill::External(Box::new(move |node_any| {
                r.fill(ToolbarHandle::new(node_any, OPS));
            })));
        }
        self
    }
}

/// One-stop import for typical use: `use toolbar::prelude::*;` brings
/// in the constructor, props, handle, item types, and the `.bind(...)`
/// extension trait.
pub mod prelude {
    pub use super::{
        Toolbar, ToolbarBind, ToolbarButton, ToolbarHandle, ToolbarItem, ToolbarProps,
    };
}

// ============================================================================
// Backend selector
// ============================================================================

// Each platform module exposes:
//   - `pub fn register(backend: &mut <ConcreteBackend>)`
//   - `pub static OPS: &dyn ToolbarOps`
// Only one is compiled per target via cfg. On targets with no
// matching impl the fallback `register<B: Backend>` keeps user code
// compiling and `OPS` resolves to `UnsupportedOps`.

#[cfg(target_os = "macos")]
pub use crate::macos::register;
#[cfg(target_os = "macos")]
static OPS: &dyn ToolbarOps = crate::macos::OPS;

#[cfg(target_os = "windows")]
pub use crate::windows::{flush_pending, register};
#[cfg(target_os = "windows")]
static OPS: &dyn ToolbarOps = crate::windows::OPS;

#[cfg(target_os = "linux")]
pub use crate::linux::register;
#[cfg(target_os = "linux")]
static OPS: &dyn ToolbarOps = crate::linux::OPS;

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod fallback {
    use runtime_core::Backend;

    /// No-op register for targets with no toolbar concept (iOS,
    /// Android, web, terminal, wgpu, ESP, CPU). User code calls
    /// this unconditionally; the fallback ignores it.
    pub fn register<B: Backend>(_backend: &mut B) {}
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub use fallback::register;

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
static OPS: &dyn ToolbarOps = &UnsupportedOps;

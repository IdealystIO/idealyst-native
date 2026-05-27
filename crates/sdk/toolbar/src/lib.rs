//! Third-party `Toolbar` SDK for the idealyst framework.
//!
//! Provides a `Toolbar` primitive backed by `Primitive::External`. On
//! native desktop hosts (macOS via `NSToolbar`; Windows/Linux land in
//! follow-ups when their backends gain `register_external`), the
//! toolbar attaches to the host window's chrome — title bar on macOS,
//! command bar on Windows, HeaderBar on GTK. On every other platform
//! (iOS, Android, web, terminal, wgpu, ESP, CPU) `register` is a
//! no-op and the in-tree primitive renders zero-size.
//!
//! That posture follows the project's mobile-first philosophy
//! ([[feedback_mobile_first_philosophy]]): toolbar / menu chrome
//! belongs in third-party SDKs, not the core Backend trait.
//!
//! # Usage
//!
//! ```ignore
//! // App bootstrap: pass an `register_extensions` closure to host_appkit::run_with.
//! host_appkit::run_with(
//!     app,
//!     host_appkit::RunOptions::default(),
//!     |backend| {
//!         toolbar::register(backend);
//!         // other SDKs that need backend.register_external::<T>(...)
//!     },
//! )?;
//!
//! // Inside a `ui!` block — the toolbar's in-tree footprint is zero,
//! // so its position in the tree doesn't matter visually. Convention:
//! // mount near the root so the items closure is owned by a long-
//! // lived scope. `.into()` lifts each `ToolbarButton` builder into
//! // the enum so `vec![]` accepts mixed kinds (buttons + spacers).
//! let count = signal(0_i32);
//! ui! {
//!     View {
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
//! # Architecture
//!
//! - The `Primitive::External` payload type is [`ToolbarProps`].
//! - Per-backend `register(&mut backend)` impls live in cfg-gated
//!   modules. The macOS impl installs an `Effect::new` inside its
//!   handler closure, so the `items` closure re-runs whenever the
//!   signals it reads change — same reactive shape as `webview::url`.
//! - [`ToolbarHandle`] carries a type-erased `Rc<dyn Any>` to the
//!   native toolbar object plus a `&'static dyn ToolbarOps` pointer
//!   the active backend module exposes. Imperative ops
//!   (`set_visible`) route through it.
//! - The in-tree node returned by the backend handler is a 0-size
//!   transparent view — toolbars are window chrome, not view content,
//!   so the placeholder is invisible regardless of where it's mounted.

use runtime_core::{Bound, Primitive, Ref, RefFill};
use std::any::{Any, TypeId};
use std::rc::Rc;

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for a `Toolbar` instance. Owned by the SDK,
/// not the framework — the framework just type-erases this behind
/// `Primitive::External { payload: Rc<dyn Any>, .. }` and hands it
/// back to the registered backend handler on mount.
///
/// `items` is reactive: the backend handler wraps the call in an
/// `Effect` and rebuilds the native toolbar's item list whenever the
/// signals captured by the closure change.
pub struct ToolbarProps {
    /// Reactive item list. Re-evaluated whenever its captured signals
    /// change; the result is diffed against the current toolbar and
    /// applied via the native toolbar's "set items" call.
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

/// One entry in the toolbar. The native backend interprets the kind
/// into the right widget — `Button` becomes an `NSToolbarItem` on
/// macOS, `Separator` becomes a `NSToolbarSeparatorItemIdentifier`,
/// the two space variants become `NSToolbarSpaceItemIdentifier` /
/// `NSToolbarFlexibleSpaceItemIdentifier`.
///
/// Build via the constructor helpers ([`ToolbarItem::button`],
/// [`ToolbarItem::separator`], [`ToolbarItem::space`],
/// [`ToolbarItem::flexible_space`]) rather than the enum directly —
/// the builder shape leaves room for the SDK to grow new optional
/// fields (tooltip, badge, custom view) without breaking existing
/// call sites.
pub enum ToolbarItem {
    Button(ToolbarButton),
    Separator,
    /// Fixed-width gap. macOS draws an NSToolbarSpaceItem (~32 px).
    Space,
    /// Flex gap that pushes following items to the right edge.
    FlexibleSpace,
}

impl ToolbarItem {
    /// Builder for a button item. Chain `.icon(...)` and `.on_click(...)`
    /// to fill in details. Label is required — toolbar buttons without
    /// a label fail accessibility and look broken with `setDisplayMode:
    /// IconOnly` regardless.
    pub fn button(label: impl Into<String>) -> ToolbarButton {
        ToolbarButton {
            label: label.into(),
            icon: None,
            on_click: None,
            tooltip: None,
        }
    }

    pub fn separator() -> Self {
        Self::Separator
    }

    pub fn space() -> Self {
        Self::Space
    }

    pub fn flexible_space() -> Self {
        Self::FlexibleSpace
    }
}

/// Button item. Use [`ToolbarItem::button`] to construct, then chain
/// `.icon(...)`, `.tooltip(...)`, `.on_click(...)`. Implements
/// `Into<ToolbarItem>` so callers can mix builders + raw variants in
/// the same `vec![...]`.
pub struct ToolbarButton {
    pub label: String,
    /// Icon name. Interpreted by the active backend:
    /// - **macOS**: SF Symbol name (e.g. `"square.and.arrow.down"`,
    ///   `"arrow.clockwise"`). Falls back to a label-only button if
    ///   the symbol isn't found at runtime.
    /// - **Windows/Linux**: ignored until those backends grow real
    ///   toolbar support.
    ///
    /// We deliberately don't route through the framework's icon
    /// registry here — SF Symbols give the toolbar a native macOS
    /// look without an asset bundle. Authors who want their own
    /// glyphs can compose a custom-view toolbar item once that
    /// surface lands.
    pub icon: Option<String>,
    pub on_click: Option<Rc<dyn Fn()>>,
    /// Hover tooltip text. macOS shows it on the toolbar item's
    /// `label` + `paletteLabel`; both also serve as the
    /// accessibility description.
    pub tooltip: Option<String>,
}

impl ToolbarButton {
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }

    pub fn on_click<F: Fn() + 'static>(mut self, f: F) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

impl From<ToolbarButton> for ToolbarItem {
    fn from(b: ToolbarButton) -> Self {
        ToolbarItem::Button(b)
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
/// Under the hood this is `Primitive::External` with a `ToolbarProps`
/// payload; on non-desktop backends the framework's "External not
/// registered" placeholder fires, but since the toolbar is window
/// chrome (not view content) the in-tree footprint stays invisible.
#[allow(non_snake_case)]
pub fn Toolbar(props: ToolbarProps) -> Bound<ToolbarHandle> {
    Bound::new(Primitive::External {
        type_id: TypeId::of::<ToolbarProps>(),
        type_name: std::any::type_name::<ToolbarProps>(),
        payload: Rc::new(props) as Rc<dyn Any>,
        style: None,
        ref_fill: None,
        accessibility: runtime_core::accessibility::AccessibilityProps::default(),
    })
}

/// Adds `.bind(r)` to `Bound<ToolbarHandle>` via an extension trait
/// (the orphan rule blocks an inherent `impl` on the foreign `Bound`).
/// Bring this trait into scope to use the builder-style `.bind(...)`
/// on the value returned by [`Toolbar`].
pub trait ToolbarBind {
    fn bind(self, r: Ref<ToolbarHandle>) -> Self;
}

impl ToolbarBind for Bound<ToolbarHandle> {
    fn bind(mut self, r: Ref<ToolbarHandle>) -> Self {
        if let Primitive::External { ref_fill, .. } = self.primitive_mut() {
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
mod macos;
#[cfg(target_os = "macos")]
pub use macos::register;
#[cfg(target_os = "macos")]
static OPS: &dyn ToolbarOps = macos::OPS;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{flush_pending, register};
#[cfg(target_os = "windows")]
static OPS: &dyn ToolbarOps = windows::OPS;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::register;
#[cfg(target_os = "linux")]
static OPS: &dyn ToolbarOps = linux::OPS;

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

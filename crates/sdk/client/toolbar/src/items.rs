//! Runtime-free item model.
//!
//! [`ToolbarItem`] / [`ToolbarButton`] carry no reactive or element
//! types — they are pure data + builder methods — so they live here and
//! are re-exported from the crate root. The platform machinery
//! (`macos_shared`, `windows`, `linux`) consumes them directly, which is
//! what keeps the native code free of any runtime import.

use std::rc::Rc;

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
    /// A clickable button (with optional icon / tooltip / handler).
    /// Construct via [`ToolbarItem::button`].
    Button(ToolbarButton),
    /// A vertical divider between item groups. macOS draws an
    /// `NSToolbarSeparatorItem`.
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

    /// A vertical divider [`ToolbarItem`]. Use between logical groups
    /// of buttons.
    pub fn separator() -> Self {
        Self::Separator
    }

    /// A fixed-width gap [`ToolbarItem`] (~32 px on macOS).
    pub fn space() -> Self {
        Self::Space
    }

    /// A flexible gap [`ToolbarItem`] that pushes every following item
    /// toward the trailing (right) edge of the toolbar.
    pub fn flexible_space() -> Self {
        Self::FlexibleSpace
    }
}

/// Button item. Use [`ToolbarItem::button`] to construct, then chain
/// `.icon(...)`, `.tooltip(...)`, `.on_click(...)`. Implements
/// `Into<ToolbarItem>` so callers can mix builders + raw variants in
/// the same `vec![...]`.
pub struct ToolbarButton {
    /// Visible button label. Required — toolbar buttons without a label
    /// fail accessibility and look broken under `IconOnly` display mode.
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
    /// Click handler, invoked on the main thread when the user activates
    /// the button. `None` makes the button inert.
    pub on_click: Option<Rc<dyn Fn()>>,
    /// Hover tooltip text. macOS shows it on the toolbar item's
    /// `label` + `paletteLabel`; both also serve as the
    /// accessibility description.
    pub tooltip: Option<String>,
}

impl ToolbarButton {
    /// Set the button's icon (an SF Symbol name on macOS). See the
    /// [`icon`](Self::icon) field for per-backend interpretation.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }

    /// Set the click handler. Fires on the main thread when the button
    /// is activated.
    pub fn on_click<F: Fn() + 'static>(mut self, f: F) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }

    /// Set the hover tooltip / accessibility description.
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

//! Primitive handles + backend ops + state bitflags + `RefFill`.
//!
//! Each primitive kind has a corresponding handle type that the parent
//! reaches via a `Ref<Handle>`. A handle is a thin record:
//!   - `node`: an `Rc<dyn Any>` holding the backend's concrete node value
//!     (`web_sys::HtmlButtonElement` on web, `View` on Android, …).
//!   - `ops`: a `&'static dyn …Ops` trait object providing the kind's
//!     methods. Backends ship a single ZST `Ops` impl per kind.
//!
//! This shape keeps `Ref<Handle>` backend-agnostic in user code while
//! letting the backend implement methods against its native node type
//! via a single downcast inside each op.
//!
//! Also home to `StateBits` (the interaction-state bitmask) and
//! `RefFill` (the type-erased enum of mount-time ref-fill closures
//! `Primitive` variants carry).

use crate::primitives;
use std::any::Any;
use std::rc::Rc;

// =============================================================================
// StateBits
// =============================================================================

/// Bitflags for interaction states the framework recognizes. Backends
/// flip these bits when corresponding native events fire (hover,
/// press, focus, disabled state). Each bit corresponds to one of the
/// `__state_*` axes a `stylesheet!` may declare as `state hovered`
/// etc. — when the bit is on, the framework adds the axis to the
/// node's `StyleApplication` so the overlay applies.
///
/// Only the listed states are supported, matching the cross-platform
/// contract enforced by the `stylesheet!` macro. Adding more would
/// need backend + macro updates in lockstep.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct StateBits(pub u8);

impl StateBits {
    pub const HOVERED: StateBits = StateBits(1 << 0);
    pub const PRESSED: StateBits = StateBits(1 << 1);
    pub const FOCUSED: StateBits = StateBits(1 << 2);
    pub const DISABLED: StateBits = StateBits(1 << 3);

    pub const NONE: StateBits = StateBits(0);

    pub fn contains(self, other: StateBits) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn with(self, other: StateBits) -> StateBits {
        StateBits(self.0 | other.0)
    }

    pub fn without(self, other: StateBits) -> StateBits {
        StateBits(self.0 & !other.0)
    }

    /// The CSS-axis name for this bit, used in `StyleApplication`
    /// variant lookups. Returns `None` for empty (zero) bits.
    pub fn axis_name(self) -> Option<&'static str> {
        match self {
            Self::HOVERED => Some("__state_hovered"),
            Self::PRESSED => Some("__state_pressed"),
            Self::FOCUSED => Some("__state_focused"),
            Self::DISABLED => Some("__state_disabled"),
            _ => None,
        }
    }

    /// Iterate the set bits in this bitmask, yielding their
    /// `__state_*` axis names. Used by the framework to build a
    /// `VariantSet` for resolution from the current active states.
    pub fn active_axes(self) -> impl Iterator<Item = &'static str> {
        [Self::HOVERED, Self::PRESSED, Self::FOCUSED, Self::DISABLED]
            .into_iter()
            .filter(move |&bit| self.contains(bit))
            .filter_map(|bit| bit.axis_name())
    }
}

// =============================================================================
// ButtonHandle / ViewHandle / TextHandle + their Ops traits
// =============================================================================

/// A handle to a mounted `Button` primitive.
///
/// `Clone` is cheap: an `Rc` bump plus copying a `'static` pointer.
/// Cloning is what lets `Ref::get()` hand back an owned handle rather
/// than forcing callers through a `.with(|h| ...)` closure.
#[derive(Clone)]
pub struct ButtonHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn ButtonOps,
}

impl ButtonHandle {
    /// Backend constructor. Called by `Backend::make_button_handle`
    /// impls. The `node` is type-erased here so user code can hold
    /// `Ref<ButtonHandle>` without naming the backend's node type.
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ButtonOps) -> Self {
        Self { node, ops }
    }

    /// Programmatically triggers the button's click handler (and on
    /// platforms with native click semantics, dispatches the native
    /// event).
    pub fn click(&self) {
        self.ops.click(&*self.node);
    }
}

pub trait ButtonOps {
    fn click(&self, node: &dyn Any);
    /// Viewport-relative rect, used when a `Button` is the anchor
    /// target of an `Overlay`. Default returns the zero rect, which
    /// causes overlays to fall back to viewport-centered placement.
    /// Backends that can measure (web `getBoundingClientRect`,
    /// iOS `UIView.frame`, Android `View.getLocationOnScreen`)
    /// override to return real values.
    #[allow(unused_variables)]
    fn rect(&self, node: &dyn Any) -> primitives::overlay::ViewportRect {
        primitives::overlay::ViewportRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
    }
}

impl primitives::overlay::AnchorableHandle for ButtonHandle {
    fn rect(&self) -> primitives::overlay::ViewportRect {
        self.ops.rect(&*self.node)
    }
}

/// A handle to a mounted `View` primitive.
#[derive(Clone)]
pub struct ViewHandle {
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn ViewOps,
}

impl ViewHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn ViewOps) -> Self {
        Self { node, ops }
    }

    /// Type-erased access to the backend's native node. Each
    /// backend stores its own `Node` type behind an `Rc<dyn Any>`
    /// here; framework helpers (notably `LayoutPlan`'s outlet
    /// resolution) downcast it back to the concrete type.
    pub fn as_any(&self) -> &dyn Any {
        &*self.node
    }
}

pub trait ViewOps {
    /// Viewport-relative rect for overlay anchoring. Same shape as
    /// `ButtonOps::rect`; see that docstring for the contract.
    /// Default returns the zero rect → centered fallback.
    #[allow(unused_variables)]
    fn rect(&self, node: &dyn Any) -> primitives::overlay::ViewportRect {
        primitives::overlay::ViewportRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 }
    }
}

impl primitives::overlay::AnchorableHandle for ViewHandle {
    fn rect(&self) -> primitives::overlay::ViewportRect {
        self.ops.rect(&*self.node)
    }
}

/// A handle to a mounted `Text` primitive.
#[derive(Clone)]
pub struct TextHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn TextOps,
}

impl TextHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn TextOps) -> Self {
        Self { node, ops }
    }
}

pub trait TextOps {
    // Reserved for future text-specific operations.
}

// =============================================================================
// RefOps + RefFill
// =============================================================================

/// Per-backend bundle of `Ops` trait objects, returned from
/// `Backend::ref_ops()`. The framework asks the backend for these once
/// (during render setup) and uses them to construct primitive handles
/// at mount time.
pub struct RefOps {
    pub button: &'static dyn ButtonOps,
    pub view: &'static dyn ViewOps,
    pub text: &'static dyn TextOps,
}

/// The mount-time closure that populates a `Ref<H>` slot. One variant
/// per primitive kind so the framework can build the correctly-typed
/// handle without runtime kind-matching on the closure itself. The
/// closure is monomorphic to `H`, so type-checking against the
/// call-site `Ref<H>` happens at `.bind()`. User code never constructs
/// this directly; it's exposed only because `Primitive`'s variants
/// carry it.
pub enum RefFill {
    Button(Box<dyn FnOnce(ButtonHandle)>),
    View(Box<dyn FnOnce(ViewHandle)>),
    Text(Box<dyn FnOnce(TextHandle)>),
    Image(Box<dyn FnOnce(primitives::image::ImageHandle)>),
    TextInput(Box<dyn FnOnce(primitives::text_input::TextInputHandle)>),
    Toggle(Box<dyn FnOnce(primitives::toggle::ToggleHandle)>),
    ScrollView(Box<dyn FnOnce(primitives::scroll_view::ScrollViewHandle)>),
    Slider(Box<dyn FnOnce(primitives::slider::SliderHandle)>),
    WebView(Box<dyn FnOnce(primitives::web_view::WebViewHandle)>),
    Video(Box<dyn FnOnce(primitives::video::VideoHandle)>),
    ActivityIndicator(Box<dyn FnOnce(primitives::activity_indicator::ActivityIndicatorHandle)>),
    Virtualizer(Box<dyn FnOnce(primitives::virtualizer::VirtualizerHandle)>),
    Graphics(Box<dyn FnOnce(primitives::graphics::GraphicsHandle)>),
    Navigator(Box<dyn FnOnce(primitives::navigator::NavigatorHandle)>),
    Link(Box<dyn FnOnce(primitives::link::LinkHandle)>),
    Overlay(Box<dyn FnOnce(primitives::overlay::OverlayHandle)>),
    Presence(Box<dyn FnOnce(primitives::presence::PresenceHandle)>),
}

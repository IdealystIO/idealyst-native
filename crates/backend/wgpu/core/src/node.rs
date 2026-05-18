//! The backend's `Node` type.
//!
//! Each node owns a `LayoutNode` (Taffy handle) and a kind tag with
//! per-kind state (text content, click handler, etc.). Children are
//! tracked here too — Taffy already stores the parent→children
//! relation, but the renderer walks via these direct `Rc` pointers
//! so we don't have to round-trip through Taffy on every frame.

use std::cell::RefCell;
use std::rc::Rc;

use framework_core::{StateBits, StyleRules};
use native_layout::LayoutNode;

use crate::style_convert::RenderStyle;

/// Default size for an unstyled `Toggle` (matches UISwitch /
/// `NSSwitch.controlSize = .regular`). Authors can override via
/// `width` / `height` in the stylesheet; this is what the Taffy
/// `set_intrinsic_size` call seeds.
pub const TOGGLE_WIDTH: f32 = 51.0;
pub const TOGGLE_HEIGHT: f32 = 31.0;

/// Apple Human Interface Guidelines minimum hit-target. UIKit
/// expands the touch region around small controls (`UISwitch`,
/// `UISlider`, tiny buttons) to at least this size so users hit
/// them at finger precision rather than pixel precision.
/// We apply the same inflation in our hit-test path.
pub const IOS_MIN_HIT_TARGET: f32 = 44.0;

/// Unconditional touch slop added on every side of an interactive
/// leaf's hit rect, on top of the [`IOS_MIN_HIT_TARGET`] floor.
/// Real iOS apps routinely add a few pixels of slop even when a
/// control is nominally large enough — clicking 1px past the
/// visual edge of a switch should still register. 8pt matches
/// UIKit's typical `hitTest` override patterns.
pub const HIT_SLOP: f32 = 8.0;
/// Padding between the track edge and the thumb at rest.
pub const TOGGLE_THUMB_INSET: f32 = 2.0;
/// Duration of the toggle's thumb-slide animation. Matches the
/// iOS UISwitch transition.
pub const TOGGLE_ANIM_MS: u32 = 200;

/// Default size for an unstyled `Slider`. Width is a sensible
/// minimum but the slider really wants to flex; authors should
/// set `flex_grow` or an explicit `width` on the slider node.
pub const SLIDER_DEFAULT_WIDTH: f32 = 200.0;
pub const SLIDER_HEIGHT: f32 = 28.0;
/// Thumb diameter. Matches the iOS / macOS slider thumb.
pub const SLIDER_THUMB_SIZE: f32 = 28.0;
/// Track thickness (the visible bar through the middle of the
/// slider's vertical extent).
pub const SLIDER_TRACK_HEIGHT: f32 = 4.0;

/// Default height for a `TextInput` that hasn't been explicitly
/// sized. Authors should still pass a `font_size` (and usually a
/// `padding`/`background`) via style — but a sensible height keeps
/// unstyled inputs visible in early bring-up.
pub const TEXT_INPUT_DEFAULT_HEIGHT: f32 = 36.0;
/// Width of the blinking caret. Half-pixel widths render fuzzy on
/// HiDPI; 1.5 splits the difference cleanly between thinness and
/// visibility.
pub const TEXT_INPUT_CARET_WIDTH: f32 = 1.5;

/// Public alias used by the `Backend` impl's associated type.
pub type WgpuNode = Rc<RefCell<NodeData>>;

/// Per-node kind discriminant + payload.
pub enum NodeKind {
    View,
    Text {
        content: String,
    },
    Pressable {
        on_click: Rc<dyn Fn()>,
    },
    Button {
        label: String,
        on_click: Rc<dyn Fn()>,
    },
    /// Editable single-line text input. The framework owns the
    /// authoritative value via a `Signal<String>`; the backend
    /// fires `on_change` on each native edit and the framework
    /// pushes value updates back through `update_text_input_value`.
    /// `placeholder` is shown when `value` is empty.
    TextInput {
        value: String,
        placeholder: Option<String>,
        on_change: Rc<dyn Fn(String)>,
    },
    /// On/off switch. Same controlled-component pattern as `TextInput`.
    Toggle {
        value: bool,
        on_change: Rc<dyn Fn(bool)>,
    },
    /// Continuous slider with optional step. Step is enforced when
    /// the user drags by rounding the candidate value to
    /// `min + step * round((v - min) / step)`. `None` = freely
    /// continuous.
    Slider {
        value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    },
    /// Stable parent for reactive `when`/`switch` branch swaps.
    /// Same shape as a View; named separately so the renderer can
    /// treat it as layout-transparent later if needed.
    ReactiveAnchor,
}

impl std::fmt::Debug for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeKind::View => f.write_str("View"),
            NodeKind::Text { content } => write!(f, "Text({content:?})"),
            NodeKind::Pressable { .. } => f.write_str("Pressable"),
            NodeKind::Button { label, .. } => write!(f, "Button({label:?})"),
            NodeKind::TextInput { value, .. } => write!(f, "TextInput({value:?})"),
            NodeKind::Toggle { value, .. } => write!(f, "Toggle({value})"),
            NodeKind::Slider { value, min, max, .. } => {
                write!(f, "Slider({value} in {min}..={max})")
            }
            NodeKind::ReactiveAnchor => f.write_str("ReactiveAnchor"),
        }
    }
}

impl std::fmt::Debug for NodeData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeData")
            .field("kind", &self.kind)
            .field("layout", &self.layout)
            .field("children", &self.children.len())
            .field("has_style", &self.style.is_some())
            .finish()
    }
}

pub struct NodeData {
    pub kind: NodeKind,
    /// Taffy node handle. Always set — even text nodes get one,
    /// with a measure function installed.
    pub layout: LayoutNode,
    /// Direct child pointers, in insertion order. The renderer uses
    /// this to walk the tree front-to-back without going through
    /// Taffy. The Taffy tree mirrors this hierarchy.
    pub children: Vec<WgpuNode>,
    /// The framework's most recently applied style. Held so the
    /// renderer can re-read paint properties (background, borders,
    /// opacity, …) on every frame without re-deriving them.
    pub style: Option<Rc<StyleRules>>,
    /// Cached render-time projection of `style` — concrete colors,
    /// resolved border widths, etc. Rebuilt by `apply_style`.
    pub render: RenderStyle,
    /// State-bits setter installed by the framework's
    /// `attach_states` hook. Present only on nodes whose stylesheet
    /// declares one or more `state {hovered,pressed,focused,disabled}`
    /// overlays. The host calls
    /// `setter(StateBits::PRESSED, true|false)` from press tracking;
    /// the framework re-resolves the style and pushes the result
    /// through `apply_style`. Unused state bits are no-ops.
    pub state_setter: Option<Rc<dyn Fn(StateBits, bool)>>,
}

pub fn new_node(kind: NodeKind, layout: LayoutNode) -> WgpuNode {
    Rc::new(RefCell::new(NodeData {
        kind,
        layout,
        children: Vec::new(),
        style: None,
        render: RenderStyle::default(),
        state_setter: None,
    }))
}

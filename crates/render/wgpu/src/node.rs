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

/// Distance the pointer must move from its press origin before a
/// tap-press is converted to a scroll pan. Matches iOS
/// `UIScrollView`'s default `panGestureRecognizer` slop.
pub const PAN_THRESHOLD: f32 = 10.0;

/// Width of the scrollbar track / thumb. Matches iOS's overlay
/// scrollbar style.
pub const SCROLLBAR_WIDTH: f32 = 3.0;
/// Inset from the scrollview's trailing edge. iOS-style: hugs
/// the edge with a hairline of breathing room so the thumb's
/// rounded cap isn't cropped at high DPI.
pub const SCROLLBAR_INSET: f32 = 1.0;
/// Minimum thumb length so a very long content extent still
/// shows a tappable / visible thumb.
pub const SCROLLBAR_MIN_THUMB: f32 = 24.0;

/// Momentum-scroll exponential decay rate (per second). Matches
/// the feel of `UIScrollView` between its "Normal" (k≈2) and
/// "Fast" (k≈10) deceleration constants. After one second of
/// coasting the velocity has dropped to `exp(-k) ≈ 8%`.
pub const SCROLL_MOMENTUM_DECAY_PER_SEC: f32 = 2.5;
/// Below this speed (px/sec) the momentum scroll is considered
/// settled and the tick loop ends. Also the threshold for
/// kicking off momentum on lift-off — a release at less than
/// this velocity stays put.
pub const SCROLL_MOMENTUM_MIN_VELOCITY: f32 = 30.0;
/// If the user holds the pointer still for longer than this
/// before releasing, the residual velocity is treated as zero
/// (their finger had settled). Without this guard a long
/// "stop, then lift" gesture would re-fire the velocity from
/// before the stop.
pub const SCROLL_MOMENTUM_STALE_MS: u128 = 80;
/// EMA mix used to smooth raw `delta/dt` samples into a stable
/// pan velocity. Higher = more weight on the latest move.
pub const SCROLL_VELOCITY_SMOOTHING: f32 = 0.6;

/// Rubber-band resistance constant. Smaller values make the
/// overshoot stiffer; larger values let the user drag further
/// past the edge before resistance kicks in. iOS uses a similar
/// `c/(c+d)` saturation curve where `c` is roughly half the
/// viewport — keeping our overshoot subtle.
pub const SCROLL_RUBBERBAND_RESISTANCE: f32 = 0.55;
/// Exponential approach rate (per second) used by the
/// rubber-band spring-back. After 1s the gap to the target has
/// dropped to `exp(-k) ≈ 1.8%` at k=4.
pub const SCROLL_SPRINGBACK_RATE_PER_SEC: f32 = 6.0;
/// Below this distance to target (in px), the spring-back tick
/// snaps to the bound and ends.
pub const SCROLL_SPRINGBACK_EPSILON: f32 = 0.5;

/// Total height of the simulator's on-screen keyboard, in
/// logical px. iOS portrait QWERTY is ~291pt; we round to a
/// clean number that leaves room for content above.
pub const KEYBOARD_HEIGHT: f32 = 280.0;
/// Horizontal margin between the keyboard's edge and the
/// first / last key in a row.
pub const KEYBOARD_SIDE_MARGIN: f32 = 4.0;
/// Vertical padding above the first row and below the last.
pub const KEYBOARD_VERT_MARGIN: f32 = 8.0;
/// Per-row vertical gap between keys.
pub const KEYBOARD_ROW_GAP: f32 = 8.0;
/// Per-row horizontal gap between keys.
pub const KEYBOARD_KEY_GAP: f32 = 6.0;
/// Corner radius on each key's rounded rect.
pub const KEYBOARD_KEY_RADIUS: f32 = 6.0;
/// Font size used for letter keys.
pub const KEYBOARD_KEY_FONT_SIZE: f32 = 18.0;

/// Caret blink period, full on→off→on cycle. Matches iOS's
/// ~1.06 sec UITextField caret rhythm.
pub const CARET_BLINK_PERIOD_SEC: f32 = 1.06;

/// Duration of the keyboard's slide-up / slide-down animation.
/// iOS uses ~250ms with an ease-out curve.
pub const KEYBOARD_ANIM_MS: u32 = 250;
/// Padding above the keyboard when auto-scrolling a focused
/// input into view. Gives the input a little breathing room
/// from the keyboard's top edge.
pub const KEYBOARD_INPUT_MARGIN: f32 = 16.0;
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

/// Pixel diameter of an `ActivityIndicatorSize::Small` spinner.
/// Matches `UIActivityIndicatorView.Style.medium` (20pt).
pub const ACTIVITY_INDICATOR_SMALL_SIZE: f32 = 20.0;
/// Pixel diameter of an `ActivityIndicatorSize::Large` spinner.
/// Matches `UIActivityIndicatorView.Style.large` (37pt rounded
/// down to keep dot-on-orbit math integer-clean).
pub const ACTIVITY_INDICATOR_LARGE_SIZE: f32 = 36.0;
/// One full rotation of the spinner's leading dot. Matches the
/// UIKit medium-size spinner — 12 ticks ≈ 1 Hz.
pub const ACTIVITY_INDICATOR_SPIN_PERIOD_SEC: f32 = 1.0;

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
    /// Scrolling container. `horizontal=false` scrolls vertically;
    /// `true` scrolls horizontally. The current scroll position
    /// lives here and is mutated by the host's `scroll` event
    /// dispatch. Children are laid out by Taffy at their natural
    /// sizes (no main-axis constraint from this node) and the
    /// renderer translates them by `-offset` when painting.
    ScrollView {
        horizontal: bool,
        offset_x: f32,
        offset_y: f32,
    },
    /// Stable parent for reactive `when`/`switch` branch swaps.
    /// Same shape as a View; named separately so the renderer can
    /// treat it as layout-transparent later if needed.
    ReactiveAnchor,
    /// Indeterminate loading spinner. `size` selects the diameter
    /// at construction (matches `UIActivityIndicatorView.Style`).
    /// `color` is the author's tint override — `None` means use
    /// the platform default (iOS systemGray, M3 primary).
    ActivityIndicator {
        size: framework_core::primitives::activity_indicator::ActivityIndicatorSize,
        color: Option<[f32; 4]>,
    },
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
            NodeKind::ScrollView { horizontal, offset_x, offset_y } => {
                write!(
                    f,
                    "ScrollView(horizontal={horizontal}, offset={offset_x},{offset_y})"
                )
            }
            NodeKind::ReactiveAnchor => f.write_str("ReactiveAnchor"),
            NodeKind::ActivityIndicator { size, .. } => {
                write!(f, "ActivityIndicator({size:?})")
            }
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

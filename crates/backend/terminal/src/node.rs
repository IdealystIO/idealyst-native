//! Node representation for the terminal backend.

use std::rc::Rc;

use framework_core::color::Rgba;
use framework_core::primitives::key::KeyDownHandler;
use framework_core::{Length, StyleRules};
use native_layout::LayoutNode;

/// Public handle the framework holds in its `Self::Node` slot. Just
/// an id — actual node data lives keyed by this id in
/// [`crate::TerminalBackend::nodes`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TermNode {
    pub id: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    View,
    Text,
    Button,
    Pressable,
    /// Boolean switch. The backend tracks the value in
    /// `NodeData.toggle_value` and renders `[ ]` / `[●]`. Clicks
    /// toggle and fire the on_change.
    Toggle,
    /// Loading spinner. Renders a single braille cell that cycles
    /// through `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` via a `raf_loop`.
    ActivityIndicator,
    /// Single-line text input. Clicks focus it; keystrokes route
    /// through the backend's [`crate::TerminalBackend::dispatch_key`]
    /// while focused. State lives in [`InputState`] on the
    /// `NodeData.input` field.
    TextInput,
}

/// Per-node data the backend stores in its `HashMap<u32, NodeData>`.
pub(crate) struct NodeData {
    pub kind: NodeKind,
    /// Visible text content. Used by `Text` and `Button` only.
    pub content: String,
    /// Optional press handler. Set by `create_button` (from
    /// `Action.fire`) and `create_pressable`.
    pub on_click: Option<Rc<dyn Fn()>>,
    /// The most recently applied resolved style. Cached for the
    /// renderer to read borders / opacity / etc.
    pub style: Option<Rc<StyleRules>>,
    pub layout: LayoutNode,
    pub children: Vec<u32>,
    /// Foreground color (text) parsed from `style.color`. None
    /// means "inherit / default".
    pub fg: Option<Rgba>,
    /// Background color parsed from `style.background`. None means
    /// "transparent" — children's bg shows through.
    pub bg: Option<Rgba>,
    /// Cached gradient pulled from `style.background_gradient`.
    /// When present, the renderer samples per-cell instead of
    /// painting a solid fill. The framework gives us `Color` strings
    /// for stops; we parse them up-front so the per-cell hot path
    /// only does arithmetic.
    pub gradient: Option<ResolvedGradient>,
    // -----------------------------------------------------------------
    // Per-frame animation overrides. Driven by `set_animated_f32` /
    // `set_animated_color`; consulted by the renderer on every paint.
    // -----------------------------------------------------------------
    /// 0.0..=1.0. Multiplied into bg's alpha and used to blend fg
    /// against the painted-under bg. Default 1.0.
    pub opacity: f32,
    /// Pixel-space translate applied on top of the laid-out frame.
    /// The renderer adds these to the resolved (x, y) before
    /// composing.
    pub translate_x: f32,
    pub translate_y: f32,
    /// Animated background override. When `Some`, wins over `bg`.
    pub animated_bg: Option<Rgba>,
    /// Animated foreground override. When `Some`, wins over `fg`.
    pub animated_fg: Option<Rgba>,
    /// Static translate from `style.transform: [translate(...)]`.
    /// Resolved at paint time because `Length::Percent` is relative
    /// to the node's own laid-out size (which we only know post-
    /// compute). The animation-driven translate (`translate_x` /
    /// `translate_y`) composes additively on top.
    pub static_translate_x: Option<Length>,
    pub static_translate_y: Option<Length>,
    /// Toggle value (only meaningful when kind == Toggle).
    pub toggle_value: bool,
    /// Backend-allocated id used by ActivityIndicator's animation
    /// loop to look itself up. The trait's required `Self::Node` is
    /// `Copy`, so we route per-instance state through this id.
    pub anim_phase: f32,
    /// Sibling-relative z-order. Higher values paint later (in
    /// front). Driven by `set_animated_f32(AnimProp::ZIndex, …)`
    /// — welcome's planets sweep through positive and negative
    /// values per orbit to pass in front of and behind the
    /// headline. Default 0.0.
    pub z_index: f32,
    /// Per-instance state for `NodeKind::TextInput`. None for other
    /// kinds. Held boxed so `NodeData` stays slim for the common
    /// (non-input) case.
    pub input: Option<Box<InputState>>,
}

/// Backend-flavoured gradient: stops resolved to `Rgba` so the
/// per-cell sampler in the renderer doesn't reparse strings on
/// every paint.
#[derive(Clone)]
pub(crate) struct ResolvedGradient {
    pub kind: framework_core::GradientKind,
    pub stops: Vec<(f32, Rgba)>,
    /// Per-stop animated color overrides written by
    /// `set_animated_color(GradientStopColor(idx))`. `None` means
    /// "use the stop's base color". The vector is initialised to
    /// `stops.len()` entries when the gradient is first cached.
    /// Welcome's vignette + sun-glare raf-driver writes through
    /// this — without it, stops stay at their static (often
    /// transparent) starting color.
    pub animated_stops: Vec<Option<Rgba>>,
}

/// Mutable runtime state for a `TextInput` node.
pub(crate) struct InputState {
    /// Current text. The framework's controlled-value pattern
    /// re-writes this via `update_text_input_value` after each
    /// `on_change` round-trip; the backend also mutates it locally
    /// on each keystroke before firing `on_change` so the cursor
    /// can sit in the right place between renders.
    pub value: String,
    /// Cursor position as a **char index** (not byte). 0..=value.chars().count().
    pub cursor: usize,
    pub placeholder: Option<String>,
    pub on_change: std::rc::Rc<dyn Fn(String)>,
    pub on_key_down: Option<KeyDownHandler>,
}

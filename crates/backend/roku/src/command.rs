//! Wire commands the Roku backend emits.
//!
//! The Rust side mints `NodeId`s and produces a stream of `RokuCommand`s.
//! A BrightScript runtime running on the Roku device deserializes each
//! command and translates it into SceneGraph operations:
//!
//! | RokuCommand                | SceneGraph node           |
//! |----------------------------|---------------------------|
//! | `CreateView`               | `LayoutGroup`             |
//! | `CreateText`               | `Label`                   |
//! | `CreateButton`             | `Button`                  |
//! | `CreatePressable`          | `Group` + `roInputEvent`  |
//! | `CreateImage`              | `Poster`                  |
//! | `CreateIcon`               | `Poster` (rasterized)     |
//! | `CreateTextInput`          | `TextEditBox`             |
//! | `CreateToggle`             | `Rectangle` + script      |
//! | `CreateSlider`             | `ProgressBar`             |
//! | `CreateScrollView`         | `LayoutGroup` (scrollable)|
//! | `CreateActivityIndicator`  | `BusySpinner`             |
//!
//! Commands are flat — no nesting — so a single `roMessagePort` loop on
//! the BrightScript side can drain a batch in order. Parent/child
//! relations are expressed via `Insert { parent, child }`.

use serde::{Deserialize, Serialize};

/// Opaque node identifier. Minted by the Rust side; the BrightScript
/// client maps each id to the SceneGraph node it owns.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

/// Identifier for a handler closure. The Rust side owns the closure;
/// when the BrightScript client observes an event it sends an
/// `Event { handler, payload }` message back through the transport.
/// Resolving the handler back to the closure is the transport's job.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HandlerId(pub u64);

/// Activity indicator size — mirrors the framework's enum so the
/// command stream is decoupled from `framework-core` internal types.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum ActivityIndicatorSize {
    Small,
    Large,
}

/// Color the BrightScript client renders. Stored as the framework's
/// portable string form (CSS-style `"#rrggbb"` / `"rgba(...)"`) so the
/// client can parse straight to `roColor` without an intermediate
/// representation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireColor(pub String);

/// A length value in pixels-on-Roku (Roku has its own 1280×720 /
/// 1920×1080 design coordinate system; the transport may scale).
/// `Auto` means defer to layout; `Percent` is a ratio of the parent's
/// relevant axis (0..=100).
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum WireLength {
    Px(f32),
    Percent(f32),
    Auto,
}

/// Subset of `StyleRules` the BrightScript client consumes. Each
/// field is `Option` so the client can leave unset values at their
/// current resolved value (no implicit reset).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WireStyle {
    pub background: Option<WireColor>,
    pub color: Option<WireColor>,
    pub font_size: Option<f32>,
    pub font_weight: Option<u32>,

    pub width: Option<WireLength>,
    pub height: Option<WireLength>,
    pub min_width: Option<WireLength>,
    pub min_height: Option<WireLength>,
    pub max_width: Option<WireLength>,
    pub max_height: Option<WireLength>,

    pub padding_top: Option<f32>,
    pub padding_right: Option<f32>,
    pub padding_bottom: Option<f32>,
    pub padding_left: Option<f32>,

    pub margin_top: Option<f32>,
    pub margin_right: Option<f32>,
    pub margin_bottom: Option<f32>,
    pub margin_left: Option<f32>,

    // SceneGraph LayoutGroup props (snake_case so the client can
    // map directly to layoutDirection / horizAlignment / itemSpacings).
    pub flex_direction: Option<FlexDirection>,
    pub justify_content: Option<JustifyContent>,
    pub align_items: Option<AlignItems>,
    pub gap: Option<f32>,

    pub border_top_left_radius: Option<f32>,
    pub border_top_right_radius: Option<f32>,
    pub border_bottom_left_radius: Option<f32>,
    pub border_bottom_right_radius: Option<f32>,

    pub opacity: Option<f32>,
    pub text_align: Option<TextAlign>,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum FlexDirection {
    Row,
    Column,
    RowReverse,
    ColumnReverse,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum JustifyContent {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum AlignItems {
    Start,
    Center,
    End,
    Stretch,
    Baseline,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    Justify,
}

/// Vector icon path data. Roku has no native SVG renderer, so the
/// BrightScript client either:
/// - rasterizes paths at boot via a Bitmap drawing routine, or
/// - uses a pre-baked sprite atlas keyed by `cache_key`.
///
/// `cache_key` is the icon's stable identity (the framework derives
/// it from the static `paths` slice address) — the client uses it as
/// a sprite-atlas lookup key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireIconData {
    pub cache_key: u64,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub paths: Vec<String>,
}

/// The full command set. Every `Backend` trait method we support
/// emits exactly one variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum RokuCommand {
    // ---------------- Create primitives ----------------
    CreateView { id: NodeId },
    CreateText { id: NodeId, content: String },
    CreateButton {
        id: NodeId,
        label: String,
        on_click: HandlerId,
        leading_icon: Option<WireIconData>,
        trailing_icon: Option<WireIconData>,
    },
    CreatePressable { id: NodeId, on_click: HandlerId },
    CreateImage { id: NodeId, src: String, alt: Option<String> },
    CreateIcon {
        id: NodeId,
        data: WireIconData,
        color: Option<WireColor>,
    },
    CreateTextInput {
        id: NodeId,
        initial_value: String,
        placeholder: Option<String>,
        on_change: HandlerId,
    },
    CreateToggle {
        id: NodeId,
        initial_value: bool,
        on_change: HandlerId,
    },
    CreateSlider {
        id: NodeId,
        initial_value: f32,
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: HandlerId,
    },
    CreateScrollView { id: NodeId, horizontal: bool },
    CreateActivityIndicator {
        id: NodeId,
        size: ActivityIndicatorSize,
        color: Option<WireColor>,
    },
    CreateReactiveAnchor { id: NodeId },

    // ---------------- Tree mutation ----------------
    Insert { parent: NodeId, child: NodeId },
    ClearChildren { parent: NodeId },

    // ---------------- Updates ----------------
    UpdateText { id: NodeId, content: String },
    UpdateButtonLabel { id: NodeId, label: String },
    UpdateImageSrc { id: NodeId, src: String },
    UpdateIconColor { id: NodeId, color: WireColor },
    UpdateTextInputValue { id: NodeId, value: String },
    UpdateToggleValue { id: NodeId, value: bool },
    UpdateSliderValue { id: NodeId, value: f32 },
    ApplyStyle { id: NodeId, style: Box<WireStyle> },
    /// State-aware style application. Carries the base rules plus
    /// per-state overlays (hovered, focused, pressed, disabled). The
    /// device-side runtime stores all of them and applies the right
    /// merged style based on the node's current state — e.g. when
    /// D-pad navigation moves "focus" to a button, the runtime
    /// re-applies (base ∪ hovered) for that node and base-only for
    /// the previously-focused one. This is the Roku analog of
    /// CSS's :hover / :focus / :active pseudo-classes — same
    /// `state hovered { ... }` stylesheet syntax works on both
    /// targets.
    ApplyStyleStates {
        id: NodeId,
        base: Box<WireStyle>,
        hovered: Option<Box<WireStyle>>,
        focused: Option<Box<WireStyle>>,
        pressed: Option<Box<WireStyle>>,
        disabled: Option<Box<WireStyle>>,
    },
    SetDisabled { id: NodeId, disabled: bool },

    // ---------------- Reactivity ----------------
    //
    // Phase 2 wire additions. Signals and bindings let the device
    // express reactive UI without a host round-trip: a signal lives
    // entirely in BrightScript, bindings register subscribers, and
    // button presses execute `#[method]`-transpiled BrightScript on
    // the device to mutate signals — which fires bound texts/styles.
    /// Declare a signal in BS-side storage. `initial` is whatever
    /// the framework's `signal!(...)` was constructed with at
    /// snapshot time. The BS runtime opens a slot for it and any
    /// subsequent `BindText` / `BindButton` can reference it by id.
    CreateSignal {
        id: SignalId,
        initial: serde_json::Value,
    },
    /// Bind a Text node's `text` field to the result of a method
    /// called with the listed signals' current values. Fires once
    /// at bind time to populate the initial text, then on every
    /// subsequent change to any of `signal_ids`.
    BindText {
        node_id: NodeId,
        signal_ids: Vec<SignalId>,
        method: String,
    },
    /// Bind a Button's press event to a method call. On every
    /// press: read the input signals in order, dispatch to
    /// `method`, and (if `output_signal_id` is set) write the
    /// return value to that signal — which propagates to its
    /// bound subscribers.
    ///
    /// The classic read-modify-write counter pattern uses the
    /// same signal for input and output: `input_signal_ids:
    /// [count]`, `method: "increment"`, `output_signal_id:
    /// Some(count)`.
    BindButton {
        button_id: NodeId,
        input_signal_ids: Vec<SignalId>,
        method: String,
        output_signal_id: Option<SignalId>,
    },
    /// Bind an anchor node to a boolean transformer over signals.
    /// Each branch ships as a `Slot` carrying *construction commands*
    /// — not pre-built node ids. The device-side runtime plays the
    /// active slot's commands to materialize the subtree on demand,
    /// and tears it down (releasing every node it created) when the
    /// binding flips to the other branch. Inactive subtrees do not
    /// exist as `roSGNode` objects.
    BindWhen {
        anchor_id: NodeId,
        signal_ids: Vec<SignalId>,
        cond_method: String,
        then_slot: Slot,
        otherwise_slot: Slot,
    },
    /// N-way structural reactivity. Same lazy materialization model
    /// as `BindWhen` — each arm and the default ship as `Slot`s; the
    /// device plays the matching one's commands on signal change
    /// and tears down the previous slot.
    BindSwitch {
        anchor_id: NodeId,
        signal_ids: Vec<SignalId>,
        cond_method: String,
        arms: Vec<SwitchArm>,
        default_slot: Slot,
    },
    /// Reactive unbounded list. The wire carries one row `Slot` as
    /// a template; the device clones it per row, allocating fresh
    /// node ids each time, and tears down clones when `count`
    /// shrinks. No upper bound — `count` directly drives how many
    /// live row instances exist.
    ///
    /// `row_index_signal_id`, if set, names the snapshot-time signal
    /// the row closure's `i` parameter was bound to. Per clone, the
    /// runtime mints a fresh synthetic signal, sets it to the row's
    /// index, and remaps the template's `signal_ids` references so
    /// any `bind!(method(i))` inside the row dispatches with the
    /// right per-row value.
    BindRepeat {
        anchor_id: NodeId,
        signal_ids: Vec<SignalId>,
        count_method: String,
        row_template: Slot,
        row_index_signal_id: Option<SignalId>,
    },

    // ---------------- Lifecycle ----------------
    /// First command on a fresh session. The BrightScript client
    /// uses this to clear its node table and mount `root` as the
    /// scene's content.
    Finish { root: NodeId },
}

/// A `bind_switch!` arm — pattern value (compared by JSON equality
/// on device) + the slot whose commands the device replays when the
/// arm matches.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SwitchArm {
    pub pattern: serde_json::Value,
    pub slot: Slot,
}

/// A lazily-materialized subtree. The device replays `commands`
/// when the slot becomes active and tears down every node it
/// created when the slot deactivates. `root_node_id` identifies
/// the subtree root inside `commands` so the runtime knows which
/// node to attach to the anchor after replay (and which subtree to
/// walk during teardown).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Slot {
    pub root_node_id: NodeId,
    pub commands: Vec<RokuCommand>,
}

/// Identifier for a signal. Minted by the Rust side at snapshot
/// time; opaque to BrightScript (just an integer key into
/// `m.signals` and `m.signalSubscribers`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignalId(pub u64);

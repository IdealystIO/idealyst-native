//! The `Chart` component — `charts-core` bound to idealyst.
//!
//! # The split
//!
//! Marks go through a `Canvas`; axis labels and the legend are ordinary
//! elements. That is not a compromise between two rendering strategies, it
//! is the point: text drawn into a canvas has to be shaped and rasterized
//! by whoever draws it, and would ignore the app's fonts, its theme colors,
//! and the platform's accessibility text scaling. Text rendered as a `text`
//! primitive gets all three for free on every backend.
//!
//! It is also why `charts-core` emits label *placements* rather than glyph
//! runs, and why this crate never implements its `LabelMetrics` — the plot
//! rect arrives from the framework's own layout, and label sizing is the
//! layout engine's job.
//!
//! # Reactivity
//!
//! One `memo` holds the rendered [`ChartOutput`], keyed on the spec and the
//! measured plot size. Everything downstream reads it:
//!
//! - the canvas painter runs inside the renderer's effect, so reading the
//!   memo there repaints on change;
//! - the label layer is a `switch` keyed on the label vector itself, so it
//!   rebuilds only when a label actually changes — a resize that moves no
//!   ticks does not churn the text nodes.
//!
//! The memo matters: without it the painter and the label layer would each
//! re-run a full render on every change.
//!
//! # No hover surface
//!
//! The chart reports hover through [`ChartProps::on_hover`] and renders no
//! tooltip. This is deliberate and it is not a gap.
//!
//! A tooltip is composable from the hover callback plus a surface, so by the
//! framework's own rule it belongs in a caller or a wrapper rather than in
//! the SDK. Owning one here also forced three bad positions at once: the
//! bubble lived inside the plot's `overflow: hidden` and was clipped by it;
//! it had to either hardcode colors (wrong in half the themes it is dropped
//! into) or render unbacked text over the marks; and its placement was a
//! fixed cursor-follow, when real charts variously snap to the hovered mark,
//! track x while pinning y, or park in a corner. Every one of those is a
//! caller's decision.
//!
//! What the SDK owes a caller instead is enough information to place a
//! surface anywhere: [`PointerFrame`] carries the pointer in plot-local AND
//! window space plus the plot's viewport rect, and `HitResult` carries each
//! mark's anchor and its full `MarkBounds`. Cursor-follow, snap-to-bar and
//! pinned-axis placements are then all expressible outside the chart, and
//! none of them is privileged by the SDK.

use std::cell::RefCell;
use std::rc::Rc;

use canvas_core::{Canvas, CanvasProps, Scene};
use charts_core::{
    render_tween, render_with, ChartOutput, ChartSpec, Color as MarkColor, DatumRef, Gutters,
    HAlign, HitResult, LabelPlacement, LabelRole, Point, Rect as IrRect, TweenAt, VAlign,
};
use runtime_core::{
    after_animation_frame, component, memo, on_scope_drop, signal, switch, view, AlignItems,
    AnchorableHandle, Element, FlexDirection, IdealystSchema, IntoElement, LayoutSubscription,
    Length, Overflow, PointerEvents, Position, Reactive, Ref, StyleRules,
    StyleSheet, TextAlign, Tokenized, Transform, Transition, ViewHandle, ViewportRect,
};
use runtime_core::Color as StyleColor;
use runtime_core::{Signal, TouchEvent, TouchPhase, TouchResponse};

/// Where the pointer is, in every space the SDK can report it.
///
/// The three travel together because they are only useful together. The chart
/// renders no tooltip of its own, so a caller's surface lives OUTSIDE the
/// chart's tree and has to place itself in its own coordinate space —
/// `local` alone cannot get it there.
///
/// - `local` is plot-local, the space [`HitResult::position`] and
///   [`HitResult::bounds`] are in. Use it to query, not to place.
/// - `window` is the pointer in window pixels, for a surface that follows
///   the cursor.
/// - `plot` is the plot box in viewport space. Adding its origin to any
///   plot-local point converts that point into the same space `window` is
///   in — which is what makes "sit beside the hovered bar" expressible
///   from outside the chart. Without it, plot-local geometry is unplaceable
///   and cursor-following is the only implementable behaviour.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct PointerFrame {
    /// Pointer in plot-local pixels.
    pub local: Point,
    /// Pointer in window pixels.
    pub window: Point,
    /// The plot box in viewport space.
    pub plot: ViewportRect,
}

impl PointerFrame {
    /// Convert a plot-local point (a [`HitResult::position`], a corner of a
    /// [`HitResult::bounds`]) into viewport space — the space a surface
    /// rendered outside the chart positions itself in.
    ///
    /// Provided because getting it wrong is silent: a surface placed with
    /// raw plot-local coordinates lands at the top-left of the WINDOW and
    /// looks like a z-order or portal bug rather than a coordinate one.
    pub fn to_viewport(&self, local: Point) -> Point {
        Point { x: self.plot.x + local.x, y: self.plot.y + local.y }
    }
}

/// What the pointer is currently over.
///
/// Reported through [`ChartProps::on_hover`] on every pointer move, not only
/// when the hovered column changes — a surface that follows the cursor needs
/// the finer rate. Dedupe on `entries` if yours snaps to a mark instead.
#[derive(Clone, PartialEq, Debug)]
pub struct ChartHover {
    /// Where the pointer is. See [`PointerFrame`].
    pub at: PointerFrame,
    /// Every series' datum in the hovered column, ordered by series. This
    /// is a column rather than a single nearest datum because that is what
    /// a multi-series readout shows — see `HitIndex::column_at`.
    pub entries: Vec<HitResult>,
}
/// Notified whenever the hovered column changes; `None` on leave.
pub type HoverCallback = Rc<dyn Fn(Option<ChartHover>)>;

/// Width in pixels reserved for the y-axis labels.
///
/// An explicit number rather than a measured one, deliberately. Tick labels
/// must sit at their exact tick position, which means absolute placement,
/// which takes them out of flow — so they cannot also size the gutter they
/// live in. Measuring would need a second layout pass and a frame of
/// visible reflow. Every production chart library makes the same call, and
/// the number is a prop precisely because the right value depends on the
/// data's magnitude.
pub const DEFAULT_Y_AXIS_WIDTH: f32 = 44.0;
/// Height in pixels reserved for the x-axis labels. See
/// [`DEFAULT_Y_AXIS_WIDTH`] for why this is explicit.
pub const DEFAULT_X_AXIS_HEIGHT: f32 = 22.0;

/// Width of the invisible box each x-axis label is centred inside.
///
/// The label is placed at `tick_x - SLOT/2` with centred text, which
/// centres it on the tick without anyone having to know how wide the
/// rendered string is. The boxes overlap; they are transparent and
/// pointer-transparent, so that is harmless.
const X_LABEL_SLOT: f32 = 88.0;
/// Line box height for a tick label, used to centre y labels on their tick.
const LABEL_LINE_PX: f32 = 16.0;
/// Line box height for the larger center readout of a donut or gauge.
const CENTER_LINE_PX: f32 = 26.0;
/// Width of the invisible box an in-plot label is aligned inside. The boxes
/// overlap; they are transparent and pointer-transparent, so that is harmless
/// — and it is what lets a label be centred on its anchor without this crate
/// knowing how wide the rendered string is.
const LABEL_SLOT: f32 = 120.0;

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct ChartProps {
    /// The chart to draw. `Reactive<ChartSpec>` — swap the whole spec to
    /// change data, series visibility, or the axis domain (which is how a
    /// pan/zoom addon drives the viewport).
    #[schema(constraint = "reactive: static ChartSpec or Signal/rx!")]
    pub spec: ChartSpec,
    /// Pixels reserved for y-axis labels. Default [`DEFAULT_Y_AXIS_WIDTH`].
    pub y_axis_width: f32,
    /// Pixels reserved for x-axis labels. Default [`DEFAULT_X_AXIS_HEIGHT`].
    pub x_axis_height: f32,
    /// Let hovering drive the spec's [`Highlight`](charts_core::Highlight) —
    /// enlarged markers, thicker lines, recolored bars. Default `true`.
    ///
    /// When on, hover OVERRIDES any `highlight.column` the author set, since
    /// the pointer is the more immediate intent. Turn it off to drive
    /// emphasis purely from the spec.
    pub highlight_on_hover: bool,
    /// Fade series containing nothing emphasised, so the highlighted one
    /// stands out. Default `false`.
    pub dim_others: bool,
    /// Animate datum values and the axis domain. `None` (the default) snaps.
    ///
    /// Same [`Transition`] the style system uses — one vocabulary, so
    /// `Transition::new(420, Easing::EaseInOut)` means here what it means on
    /// a `background_transition`. The mechanism differs (marks are painted
    /// into a canvas, so the chart drives its own frame loop rather than
    /// handing the backend a CSS transition), but the declaration does not.
    ///
    /// Tick labels switch to the new ones immediately so they do not churn
    /// through intermediate numbers. A change that alters the chart's SHAPE
    /// — a series added or removed, a series' length changed, a kind swapped
    /// — snaps, because pairing unrelated points animates a bar toward a
    /// value it has nothing to do with, which reads worse than not animating.
    #[prop(static)]
    #[schema(constraint = "optional Transition { duration_ms, easing }")]
    pub value_transition: Option<Transition>,
    /// Animate colour changes. `None` (the default) snaps.
    ///
    /// Covers both a series' own colour and whatever a
    /// [`StyleFn`](charts_core::StyleFn) resolves to — a threshold recolor
    /// fades across the transition instead of flipping at the frame the
    /// value crosses the threshold.
    ///
    /// Its own channel because colour rarely wants the value clock: a 420 ms
    /// glide is right for a bar changing height and sluggish for the same
    /// bar changing hue. What it does NOT cover is highlight — a point
    /// becoming selected lands at once, because easing into a state the user
    /// just caused feels laggy rather than smooth.
    #[prop(static)]
    #[schema(constraint = "optional Transition { duration_ms, easing }")]
    pub color_transition: Option<Transition>,
    /// Points to render as selected. Reactive, so a host can drive selection
    /// from its own signal without rebuilding the whole spec.
    ///
    /// `#[prop(reactive)]` because `#[props]` treats a bare `Vec` as children
    /// and would otherwise leave it non-reactive.
    #[prop(reactive)]
    #[schema(constraint = "reactive: Vec<DatumRef> of selected points")]
    pub selected: Vec<DatumRef>,
    /// Called on every pointer move over the plot, and with `None` when the
    /// pointer leaves. This is the ONLY tooltip mechanism the SDK provides:
    /// the chart draws marks, labels and legend, and a caller renders any
    /// hover surface itself, outside the chart's tree. See [`ChartHover`].
    ///
    /// `#[prop(static)]`: `#[props]` recognises handlers by matching the
    /// literal path segments `Rc`/`Arc`/`Box` followed by `dyn Fn`, so the
    /// `HoverCallback` alias hides the handler from it and the field would
    /// be wrapped as `Reactive<Option<..>>` — which then rejects a plain
    /// `Some(..)`/`None` at every call site.
    #[prop(static)]
    #[schema(constraint = "optional Fn(Option<ChartHover>)")]
    pub on_hover: Option<HoverCallback>,
    /// Style for tick labels. The SDK ships a deliberately plain default
    /// (small, inheriting color) so a chart picks up the surrounding theme
    /// rather than imposing one; pass a sheet to size or color them.
    #[prop(static)]
    pub label_style: Option<Rc<StyleSheet>>,
    /// Style for the chart's root box.
    #[prop(static)]
    pub style: Option<Rc<StyleSheet>>,
}

impl Default for ChartProps {
    fn default() -> Self {
        Self {
            spec: Reactive::Static(ChartSpec::default()),
            y_axis_width: Reactive::Static(DEFAULT_Y_AXIS_WIDTH),
            x_axis_height: Reactive::Static(DEFAULT_X_AXIS_HEIGHT),
            highlight_on_hover: Reactive::Static(true),
            dim_others: Reactive::Static(false),
            selected: Reactive::Static(Vec::new()),
            value_transition: None,
            color_transition: None,
            on_hover: None,
            label_style: None,
            style: None,
        }
    }
}

// Hand-written: the props carry `Rc<dyn Fn>` handlers, which cannot derive
// Debug, and requiring it on `Reactive<ChartSpec>` would push the bound onto
// every spec a caller builds. Reporting whether the optional hooks are set is
// the part that is actually useful when debugging a chart that "does nothing".
impl std::fmt::Debug for ChartProps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChartProps")
            .field("on_hover", &self.on_hover.is_some())
            .field("label_style", &self.label_style.is_some())
            .field("style", &self.style.is_some())
            .finish_non_exhaustive()
    }
}

pub(crate) fn sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

pub(crate) fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

/// Root: a column of [plot row] over [x-axis gutter].
///
/// The height chain is the fiddly part, and getting any link wrong makes
/// the chart silently unusable rather than merely ugly.
///
/// The canvas asks for `height: 100%`, which resolves only against a parent
/// with a DEFINITE height. So every box between the author's container and
/// the canvas has to be definite too:
///
/// - `height: 100%` here, so the chart root takes its container's height
///   instead of its own content's. Without it the root is content-sized,
///   the content is the canvas, and the canvas is sized from the root —
///   a cycle the web backend resolves by growing the canvas every frame
///   until it hits the 16384px element limit. Every axis label ends up
///   thousands of pixels off-screen and the chart looks blank.
/// - `min_height: 0` on the root, the plot row, and the plot. A flex item's
///   `min-height` defaults to `auto`, which refuses to shrink below content
///   — so even with a definite root, an oversized canvas keeps its
///   ancestors inflated. This is the brake on the same cycle.
/// - `flex_basis: 0` on the plot row, so it is sized purely by `flex_grow`
///   rather than starting from its content height.
///
/// Requirement on the caller: the chart's container must itself have a
/// resolvable height (a fixed height, or `flex_grow` inside a parent that
/// has one). A chart in a purely auto-height column has no height to take.
fn root_rules() -> StyleRules {
    StyleRules {
        flex_direction: Some(FlexDirection::Column),
        flex_grow: Some(Tokenized::Literal(1.0)),
        width: Some(Tokenized::Literal(Length::pct(100.0))),
        height: Some(Tokenized::Literal(Length::pct(100.0))),
        min_height: px(0.0),
        position: Some(Position::Relative),
        ..Default::default()
    }
}

/// The default label sheet, shared by the cartesian and polar components so
/// one unstyled chart looks like another.
pub(crate) fn label_sheet() -> Rc<StyleSheet> {
    sheet(label_rules())
}

/// The default tick-label look: small text that inherits its color.
///
/// Colorless on purpose — an SDK that hardcoded a label color would fight
/// every theme it is dropped into, and would be wrong in dark mode.
fn label_rules() -> StyleRules {
    StyleRules {
        font_size: px(11.0),
        // Labels must never eat pointer events: the plot's hover handler
        // sits underneath them and a label swallowing a move would make
        // the tooltip flicker as the cursor crossed a tick.
        pointer_events: Some(PointerEvents::None),
        ..Default::default()
    }
}

/// Width of the box an axis TITLE is centred in before rotation.
const TITLE_SLOT: f32 = 120.0;

/// One label, absolutely placed against the gutter it lives in.
///
/// Placement is per-role rather than straight from the anchor point,
/// because the anchors `charts-core` emits are in PLOT-local space and each
/// gutter is a different box. Routing by role is what keeps the y title off
/// the x axis — dropping every non-`AxisY` label into the x gutter piles the
/// title and the legend on top of the tick labels.
fn label_element(l: &LabelPlacement, style: Rc<StyleSheet>, y_axis_width: f32) -> Element {
    let mut rules = StyleRules { position: Some(Position::Absolute), ..Default::default() };
    match l.role {
        LabelRole::AxisY => {
            // Right-aligned against the gutter's inner edge, vertically
            // centred on the tick.
            rules.right = px(6.0);
            rules.top = px(l.anchor.y - LABEL_LINE_PX / 2.0);
            rules.height = px(LABEL_LINE_PX);
            rules.text_align = Some(TextAlign::Right);
        }
        LabelRole::AxisTitleY => {
            // Rotated to read bottom-to-top, the near-universal convention.
            // The box is laid out horizontally and then rotated about its
            // own centre, so `left` is chosen to put that centre on the
            // gutter's centre line — after rotation it becomes a vertical
            // strip in the same place.
            rules.left = px(y_axis_width / 2.0 - TITLE_SLOT / 2.0);
            rules.width = px(TITLE_SLOT);
            rules.top = px(l.anchor.y - LABEL_LINE_PX / 2.0);
            rules.height = px(LABEL_LINE_PX);
            rules.text_align = Some(TextAlign::Center);
            rules.transform = Some(vec![Transform::Rotate(l.rotation)]);
        }
        LabelRole::AxisTitleX => {
            rules.left = px(y_axis_width + l.anchor.x - TITLE_SLOT / 2.0);
            rules.width = px(TITLE_SLOT);
            rules.top = px(LABEL_LINE_PX + 2.0);
            rules.text_align = Some(TextAlign::Center);
        }
        _ => {
            // x tick labels live in the gutter below the plot, which starts
            // at the chart's left edge — shift by the y gutter to get back
            // into plot-local x.
            rules.left = px(y_axis_width + l.anchor.x - X_LABEL_SLOT / 2.0);
            rules.top = px(2.0);
            rules.width = px(X_LABEL_SLOT);
            rules.text_align = Some(match l.h_align {
                HAlign::Left => TextAlign::Left,
                HAlign::Center => TextAlign::Center,
                HAlign::Right => TextAlign::Right,
            });
        }
    }
    view(vec![runtime_core::text(l.text.clone()).with_style(style).into_element()])
        .with_style(sheet(rules))
        .into_element()
}

/// The gutter sizes a spec actually needs: `(y width, x height)`.
///
/// An axis that draws no labels and no title has nothing to put in a gutter,
/// so reserving one would make [`ChartSpec::sparkline`] cost exactly what the
/// furniture it removed cost — a 40px-tall sparkline handed the default 22px
/// x-gutter has 18px of plot left, and it looks broken rather than small.
///
/// Derived from the spec rather than from the props, so an author turning
/// labels off does not also have to remember to zero the widths. Passing the
/// widths in keeps them a prop for the case that matters: how much room the
/// labels that DO exist need depends on the data's magnitude.
pub fn gutters_for(spec: &ChartSpec, y_width: f32, x_height: f32) -> (f32, f32) {
    let y = if spec.y.labels || spec.y.title.is_some() { y_width } else { 0.0 };
    let x = if spec.x.labels || spec.x.title.is_some() { x_height } else { 0.0 };
    (y, x)
}

/// One label, absolutely placed at its anchor inside the plot box.
///
/// For labels whose anchor is already where they belong: every polar label,
/// and the cartesian annotation labels. Placement is a direct translation of
/// the alignment the core asked for, unlike [`label_element`], which has to
/// dispatch on role because each axis gutter is a different box.
pub(crate) fn overlay_label(l: &LabelPlacement, style: Rc<StyleSheet>, plot_w: f32) -> Element {
    let line = if l.role == LabelRole::Title { CENTER_LINE_PX } else { LABEL_LINE_PX };
    let mut rules = StyleRules {
        position: Some(Position::Absolute),
        width: px(LABEL_SLOT),
        height: px(line),
        ..Default::default()
    };
    match l.h_align {
        HAlign::Left => {
            rules.left = px(l.anchor.x);
            rules.text_align = Some(TextAlign::Left);
        }
        HAlign::Center => {
            rules.left = px(l.anchor.x - LABEL_SLOT / 2.0);
            rules.text_align = Some(TextAlign::Center);
        }
        HAlign::Right => {
            // Anchored from the container's right edge, since that is what
            // absolute `right` means — the alternative would need the
            // rendered text width, which this crate deliberately never has.
            rules.right = px((plot_w - l.anchor.x).max(0.0));
            rules.text_align = Some(TextAlign::Right);
        }
    }
    rules.top = px(match l.v_align {
        VAlign::Top => l.anchor.y,
        VAlign::Middle => l.anchor.y - line / 2.0,
        VAlign::Bottom | VAlign::Baseline => l.anchor.y - line,
    });

    // A slice-tinted label carries its color on the placement; anything
    // else inherits, so an unstyled chart picks up the surrounding theme
    // instead of imposing one.
    let text_style = match (l.role, l.color) {
        (LabelRole::Title, _) => sheet(StyleRules {
            font_size: px(20.0),
            pointer_events: Some(PointerEvents::None),
            ..Default::default()
        }),
        (_, Some(c)) => sheet(StyleRules {
            font_size: px(11.0),
            pointer_events: Some(PointerEvents::None),
            color: Some(Tokenized::Literal(StyleColor(format!(
                "#{:02x}{:02x}{:02x}",
                c.r, c.g, c.b
            )))),
            ..Default::default()
        }),
        _ => style,
    };
    view(vec![runtime_core::text(l.text.clone()).with_style(text_style).into_element()])
        .with_style(sheet(rules))
        .into_element()
}

/// The eased fraction of one transition channel at `elapsed_ms`.
///
/// `None` — no transition declared — is settled immediately, which is what
/// makes "no transition" mean "snap" rather than "animate instantly over zero
/// milliseconds and divide by zero". A declared duration of `0` is treated
/// the same way for the same reason.
///
/// Pure so the two-clock arithmetic is a test rather than something to notice
/// by eye on a 420 ms animation.
pub fn channel_at(tr: Option<Transition>, elapsed_ms: f32) -> f32 {
    match tr {
        Some(t) if t.duration_ms > 0 => {
            let linear = (elapsed_ms / t.duration_ms as f32).clamp(0.0, 1.0);
            // The framework's own curve evaluator, so `Easing::Ease` means
            // here exactly what it means on a `background_transition`.
            runtime_core::animation::apply_easing(linear, t.easing)
        }
        _ => 1.0,
    }
}

/// Both channels' eased fractions at `elapsed_ms`.
pub fn tween_at(
    value: Option<Transition>,
    color: Option<Transition>,
    elapsed_ms: f32,
) -> TweenAt {
    TweenAt { value: channel_at(value, elapsed_ms), color: channel_at(color, elapsed_ms) }
}

/// How long the whole transition runs: the longer of the two channels.
///
/// One frame loop drives both, so it has to outlast the slower one — stopping
/// at the shorter would freeze the other channel part-resolved.
pub fn transition_span_ms(value: Option<Transition>, color: Option<Transition>) -> u32 {
    let d = |t: Option<Transition>| t.map_or(0, |t| t.duration_ms);
    d(value).max(d(color))
}

/// The spec the chart currently DISPLAYS.
///
/// Mid-transition that is the interpolated state; at rest it is the settled
/// target. This is the value a new transition must start from, and getting
/// it wrong is not obvious from the code: an earlier cut started every
/// transition from `origin` alone, which is only ever written when an
/// animation begins and so stayed pinned to the first spec the chart saw.
/// The second data change then animated from those original values, and once
/// the shape had changed in between (a different series kind, say) it took
/// the snap path and stopped animating at all. Pure, so that is a test
/// rather than something to notice by eye.
pub fn visual_state(
    origin: Option<&ChartSpec>,
    target: Option<&ChartSpec>,
    at: TweenAt,
) -> Option<ChartSpec> {
    match (origin, target) {
        (Some(o), Some(tg)) if at.value < 1.0 || at.color < 1.0 => {
            Some(charts_core::lerp_data(o, tg, at).unwrap_or_else(|| tg.clone()))
        }
        (_, tg) => tg.cloned(),
    }
}

/// Resolve a pointer position into a hovered column.
///
/// Split out as a pure function so the mapping from pixels to a
/// [`ChartHover`] is testable without a backend, a mounted tree, or a
/// synthetic touch event — the interesting logic is here, and the handler
/// below is only event plumbing around it.
///
/// Takes the whole [`PointerFrame`] rather than a local `(x, y)`: the window
/// position and plot rect are carried through to the callback unchanged, and
/// threading them past this function instead would let the component report a
/// frame that disagrees with the hit it was resolved from.
pub fn hover_at(out: &ChartOutput, at: PointerFrame) -> Option<ChartHover> {
    let entries = out.hit.column_at(charts_core::pt(at.local.x, at.local.y));
    if entries.is_empty() {
        None
    } else {
        Some(ChartHover { at, entries })
    }
}

/// Assemble the [`PointerFrame`] for one event.
///
/// The plot rect is read live from the mounted handle rather than cached,
/// because it moves for reasons the chart never observes — an ancestor
/// scrolling, a sibling panel opening, the window moving. A cached origin is
/// correct until the first scroll and then silently offsets every surface a
/// caller places, which is exactly the class of bug that is hardest to
/// attribute back to the chart. The read is a backend rect query on a handle
/// we already hold, on an event that is already doing a hit-test.
///
/// An unmounted handle yields a zero rect, so `local` and `window` still
/// arrive intact and only viewport conversion degrades — the frame before
/// first layout, where there is nothing to place against anyway.
pub(crate) fn pointer_frame(plot_ref: Ref<ViewHandle>, ev: &TouchEvent) -> PointerFrame {
    PointerFrame {
        local: Point { x: ev.position.x, y: ev.position.y },
        window: Point { x: ev.window_position.x, y: ev.window_position.y },
        plot: plot_ref.with(|h| h.rect()).unwrap_or_default(),
    }
}

/// One legend entry: a color swatch and the series name.
///
/// Built from the SPEC, not from the `LabelRole::Legend` placements the core
/// emits. Those placements carry a naive fixed spacing because the core
/// cannot measure text — which is exactly the case the core documents as
/// "the host lays these out". Here the host is a layout engine, so a flex
/// row does it properly and the placements go unused.
pub(crate) fn legend_entry(
    name: &str,
    color: MarkColor,
    label_style: Rc<StyleSheet>,
) -> Element {
    let swatch = view(Vec::new())
        .with_style(sheet(StyleRules {
            width: px(10.0),
            height: px(10.0),
            border_top_left_radius: px(5.0),
            border_top_right_radius: px(5.0),
            border_bottom_left_radius: px(5.0),
            border_bottom_right_radius: px(5.0),
            background: Some(Tokenized::Literal(StyleColor(format!(
                "#{:02x}{:02x}{:02x}",
                color.r, color.g, color.b
            )))),
            ..Default::default()
        }))
        .into_element();
    view(vec![swatch, runtime_core::text(name.to_string()).with_style(label_style).into_element()])
        .with_style(sheet(StyleRules {
            flex_direction: Some(FlexDirection::Row),
            align_items: Some(AlignItems::Center),
            column_gap: px(5.0),
            ..Default::default()
        }))
        .into_element()
}

/// Renders a chart: marks on a canvas, labels and legend as real elements.
///
/// No hover surface: see [`ChartProps::on_hover`].
#[component]
pub fn Chart(props: &ChartProps) -> Element {
    let spec = props.spec.clone();
    let y_axis_width = props.y_axis_width.get();
    let x_axis_height = props.x_axis_height.get();
    let highlight_on_hover = props.highlight_on_hover.get();
    let dim_others = props.dim_others.get();
    let value_transition = props.value_transition;
    let color_transition = props.color_transition;
    let span_ms = transition_span_ms(value_transition, color_transition);
    let selected = props.selected.clone();
    let label_style = props.label_style.clone().unwrap_or_else(label_sheet);

    // Measured plot size. Starts at zero: nothing draws until the first
    // layout callback arrives, which is one frame after mount. Drawing
    // against a guessed size instead would paint a visibly wrong chart and
    // then jump.
    let plot_w = signal(0.0_f32);
    let plot_h = signal(0.0_f32);
    let plot_ref: Ref<ViewHandle> = Ref::new();

    let hover = signal(None::<ChartHover>);
    // The hovered COLUMN, kept separate from `hover` on purpose.
    //
    // `hover` carries pixel coordinates and therefore changes on every
    // pointer move; the render memo must not depend on that or the whole
    // chart re-renders per pixel. The column is the datum's x, so it changes
    // only when the pointer crosses into a new column — which is exactly the
    // granularity emphasis needs.
    let hover_col = signal(None::<f64>);

    // --- transition state --------------------------------------------------
    // `target` is the spec as the author last supplied it. `origin` is where
    // the current animation started FROM, and `progress` runs 0..1 across it.
    // Keeping the origin as a whole spec (rather than a diff) is what lets a
    // change landing mid-flight restart cleanly from wherever the chart
    // currently looks, instead of snapping back to the previous target.
    let origin: Signal<Option<ChartSpec>> = signal(None);
    // The spec the chart is animating TOWARD. Tracked separately from
    // `origin` because the two answer different questions, and conflating
    // them was a real bug: `origin` alone is never advanced once a
    // transition ends, so every later change tried to animate from the very
    // first spec the chart ever saw — usually a different shape by then, so
    // it took the snap path and nothing ever animated.
    let target: Signal<Option<ChartSpec>> = signal(None);
    // Wall-clock milliseconds since the current transition began. ONE clock
    // for both channels — `tween_at` derives each channel's own eased
    // fraction from it, so a 420 ms value glide and a 150 ms colour fade are
    // one frame loop and one signal, not two of each.
    let elapsed = signal(f32::INFINITY);
    let anim: Rc<RefCell<Option<runtime_core::scheduling::RafLoop>>> = Rc::new(RefCell::new(None));
    // Holds the one-shot that tears the frame loop down. It cannot be
    // dropped from inside the loop's own callback (that would re-enter the
    // RefCell the callback is running under), so the stop is deferred by a
    // frame and parked here.
    let anim_stop: Rc<RefCell<Option<runtime_core::ScheduledTask>>> = Rc::new(RefCell::new(None));

    if span_ms > 0 {
        let spec_for_anim = spec.clone();
        let anim_slot = anim.clone();
        let stop_slot = anim_stop.clone();
        // Watches the spec ONLY. Reading `progress` tracked here would make
        // the effect its own dependency and re-arm the loop every frame.
        runtime_core::effect!({
            let next = spec_for_anim.get();

            // Where the chart LOOKS right now. Mid-flight that is the
            // interpolated state, so a change landing during an animation
            // continues from what is on screen instead of jumping back.
            let visual = runtime_core::untrack(|| {
                let at = tween_at(value_transition, color_transition, elapsed.get());
                visual_state(origin.get().as_ref(), target.get().as_ref(), at)
            });

            match visual {
                // First spec ever: nothing to animate from.
                None => {
                    origin.set(Some(next.clone()));
                    target.set(Some(next));
                    elapsed.set(f32::INFINITY);
                }
                Some(v) if v == next => {
                    // Same values — keep the target current (colors or
                    // highlight may still have changed) but do not animate.
                    target.set(Some(next));
                }
                Some(v) => {
                    origin.set(Some(v));
                    target.set(Some(next));
                    elapsed.set(0.0);
                    stop_slot.borrow_mut().take();

                    // Drive from wall-clock elapsed rather than a per-frame
                    // increment, so the duration is honest on a slow frame
                    // and cannot run long on a fast display.
                    let start = runtime_core::time::now_micros();
                    let span_us = (span_ms as u64).max(1) * 1000;
                    let inner = anim_slot.clone();
                    let stop_slot = stop_slot.clone();
                    *anim_slot.borrow_mut() =
                        Some(runtime_core::scheduling::raf_loop(move || {
                            let us = runtime_core::time::now_micros().saturating_sub(start);
                            elapsed.set(us as f32 / 1000.0);
                            // Runs until the LONGER channel is done; the
                            // shorter one has already clamped to 1.0 and
                            // simply stops changing.
                            if us >= span_us && stop_slot.borrow().is_none() {
                                // Stop from OUTSIDE the callback's own
                                // borrow: dropping the RafLoop is what
                                // cancels it, and doing that while the
                                // closure runs would re-enter the RefCell
                                // it is held in.
                                let slot = inner.clone();
                                let task = runtime_core::after_animation_frame(move || {
                                    slot.borrow_mut().take();
                                });
                                *stop_slot.borrow_mut() = Some(task);
                            }
                        }));
                }
            }
        });
    }

    // The frame loop is scope-owned: a chart that unmounts mid-transition
    // must stop ticking, or the loop writes signals whose scope is gone.
    {
        let anim = anim.clone();
        let anim_stop = anim_stop.clone();
        on_scope_drop(move || {
            anim.borrow_mut().take();
            anim_stop.borrow_mut().take();
        });
    }

    // Reactive because `sparkline()` (or any `labels(false)`) has to give the
    // space back — see `gutters_for`. Part of every label layer's switch key
    // below, so a spec that turns its axes off rebuilds the gutters too.
    let gutters = {
        let spec = spec.clone();
        memo(move || gutters_for(&spec.get(), y_axis_width, x_axis_height))
    };

    // The single render. Everything else reads this.
    let output = {
        let spec = spec.clone();
        let selected = selected.clone();
        memo(move || {
            let (w, h) = (plot_w.get(), plot_h.get());
            let mut s = spec.get();
            if highlight_on_hover {
                if let Some(x) = hover_col.get() {
                    s.highlight.column = Some(x);
                }
            }
            let sel = selected.get();
            if !sel.is_empty() {
                s.highlight.points = sel;
            }
            if dim_others {
                s.highlight.dim_others = true;
            }
            // A dim factor of zero would erase the other series entirely;
            // fall back to the same default `Highlight::column` uses so a
            // caller enabling dimming without setting one gets a sane fade.
            if s.highlight.dim_others && s.highlight.dim_opacity <= 0.0 {
                s.highlight.dim_opacity = 0.35;
            }
            let rect = IrRect::new(0.0, 0.0, w, h);
            let at = tween_at(value_transition, color_transition, elapsed.get());
            match origin.get() {
                // Mid-flight on EITHER channel: values may have settled while
                // a shorter-or-longer colour fade is still running, and the
                // tween render is what interpolates the colour.
                Some(from) if span_ms > 0 && (at.value < 1.0 || at.color < 1.0) => {
                    render_tween(&from, &s, at, rect, &Gutters::None)
                }
                _ => render_with(&s, rect, &Gutters::None),
            }
        })
    };

    // --- plot size subscription -------------------------------------------
    // Deferred to the next frame because `plot_ref` is not filled until the
    // mount pass completes; subscribing during render would find an empty
    // Ref. Both the task and the subscription are anchored to the component
    // scope via `on_scope_drop` — NOT leaked. A leaked LayoutSubscription
    // outlives the signals it writes, and a late callback then writes a
    // freed slot, which aborts.
    let sub_holder: Rc<RefCell<Option<LayoutSubscription>>> = Rc::new(RefCell::new(None));
    let holder = sub_holder.clone();
    let setup = after_animation_frame(move || {
        // The last size we WROTE, in a plain cell rather than read back from
        // the signals.
        //
        // `set` only STAGES; the driver's flush is what commits. The seeding
        // write below and the first `on_layout` callback land in the same
        // turn, so reading `plot_w.get()` in the guard returns the committed
        // 0.0 rather than the width just staged — the guard then always
        // fires and costs an extra render on mount. (The framework warns
        // about exactly this read in debug builds.) A local is always
        // current and costs no signal read per layout pass.
        let seen = Rc::new(std::cell::Cell::new((0.0f32, 0.0f32)));
        let s = plot_ref.with(|h| {
            // Seed from the view's CURRENT frame before subscribing.
            //
            // Subscribing alone is not enough. `on_layout` fires during a
            // layout PASS, and by the time this next-frame task runs the
            // first pass is already done — so on a backend that only
            // notifies on a pass (macOS/AppKit does; the web's
            // ResizeObserver instead fires once on observe) the callback may
            // never fire at all, the plot size stays (0, 0), and the chart
            // renders permanently blank. Reading the frame here makes the
            // first size independent of that difference, and the
            // subscription then handles every later resize.
            let r = h.rect();
            let mut seed = (0.0f32, 0.0f32);
            if r.width > 0.5 {
                plot_w.set(r.width);
                seed.0 = r.width;
            }
            if r.height > 0.5 {
                plot_h.set(r.height);
                seed.1 = r.height;
            }
            seen.set(seed);
            let seen = seen.clone();
            h.on_layout(move |w, hgt| {
                // Guarded: layout fires on every pass, and an unconditional
                // set would re-run the memo, the painter and the label
                // switches on every unrelated relayout. `seen` only advances
                // when we actually write, so a run of sub-threshold changes
                // still accumulates into one that crosses it.
                let (mut pw, mut ph) = seen.get();
                if (pw - w).abs() > 0.5 {
                    plot_w.set(w);
                    pw = w;
                }
                if (ph - hgt).abs() > 0.5 {
                    plot_h.set(hgt);
                    ph = hgt;
                }
                seen.set((pw, ph));
            })
        });
        if let Some(s) = s {
            *holder.borrow_mut() = Some(s);
        }
    });
    on_scope_drop(move || {
        drop(setup);
        drop(sub_holder);
    });

    // --- the canvas --------------------------------------------------------
    let canvas = {
        Canvas(CanvasProps {
            // Runs inside the renderer's effect: reading `output` here is
            // what makes the chart repaint when the spec or the size
            // changes. No manual invalidation anywhere.
            draw: canvas_core::draw(move |s: &mut Scene| {
                let out = output.get();
                crate::adapt::marks_into_scene(&out.scene.marks, s, 0.0, 0.0);
            }),
            ..Default::default()
        })
        .into_element()
    };

    // --- pointer -----------------------------------------------------------
    // `Hovered` is pointer motion with no button down — the desktop/web
    // hover channel. Touch backends never emit it, so `Began`/`Moved` give
    // mobile a drag-to-scrub crosshair, which is the correct touch idiom
    // rather than a workaround. One handler, no platform branch.
    let on_hover_cb = props.on_hover.clone();
    let touch = {
        let on_hover_cb = on_hover_cb.clone();
        move |ev: &TouchEvent| -> TouchResponse {
            match ev.phase {
                TouchPhase::Hovered | TouchPhase::Moved | TouchPhase::Began => {
                    let out = output.get();
                    let next = hover_at(&out, pointer_frame(plot_ref, ev));
                    // Column first: it is what the render memo watches, and
                    // it changes far less often than the pointer does.
                    let col = next.as_ref().and_then(|h| h.entries.first().map(|e| e.datum.x));
                    if hover_col.get() != col {
                        hover_col.set(col);
                    }
                    if hover.get() != next {
                        hover.set(next.clone());
                        if let Some(cb) = &on_hover_cb {
                            cb(next);
                        }
                    }
                    // Never consume: a chart must not swallow a scroll or a
                    // parent's gesture just because the pointer crossed it.
                    TouchResponse::IGNORED
                }
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    if hover_col.get().is_some() {
                        hover_col.set(None);
                    }
                    if hover.get().is_some() {
                        hover.set(None);
                        if let Some(cb) = &on_hover_cb {
                            cb(None);
                        }
                    }
                    TouchResponse::IGNORED
                }
            }
        }
    };
    let leave_cb = on_hover_cb.clone();

    // --- in-plot labels ----------------------------------------------------
    // Annotation text is anchored in PLOT-local space — it belongs beside its
    // own rule, not in a gutter — so it gets its own absolutely-positioned
    // layer rather than being routed by role like the axis labels. Without
    // this the core emits the placements and the component silently drops
    // them: the rule draws and its label does not.
    let plot_labels = {
        let label_style = label_style.clone();
        switch(
            move || {
                output
                    .get()
                    .scene
                    .labels
                    .iter()
                    .filter(|l| matches!(l.role, LabelRole::Annotation | LabelRole::DataLabel))
                    .cloned()
                    .collect::<Vec<_>>()
            },
            move |ls: &Vec<LabelPlacement>| {
                let w = plot_w.get();
                view(ls.iter().map(|l| overlay_label(l, label_style.clone(), w)).collect())
                    .with_style(sheet(StyleRules {
                        position: Some(Position::Absolute),
                        left: px(0.0),
                        top: px(0.0),
                        right: px(0.0),
                        bottom: px(0.0),
                        pointer_events: Some(PointerEvents::None),
                        ..Default::default()
                    }))
                    .into_element()
            },
        )
    };

    // --- plot area ---------------------------------------------------------
    let plot = view(vec![canvas, plot_labels])
        .bind(plot_ref)
        .on_touch(move |ev| touch(ev))
        .on_hover(move |entering| {
            if !entering && hover_col.get().is_some() {
                hover_col.set(None);
            }
            if !entering && hover.get().is_some() {
                hover.set(None);
                if let Some(cb) = &leave_cb {
                    cb(None);
                }
            }
        })
        .with_style(sheet(StyleRules {
            flex_grow: Some(Tokenized::Literal(1.0)),
            // See `root_rules`: without this the canvas's `height: 100%`
            // and this box's auto min-height inflate each other.
            min_height: px(0.0),
            position: Some(Position::Relative),
            // The plot clips its own overflow so a mark at the domain edge
            // cannot paint over the axis gutters.
            overflow: Some(Overflow::Hidden),
            ..Default::default()
        }))
        .into_element();

    // --- label layers ------------------------------------------------------
    // Keyed on the label vector itself, so the text nodes are rebuilt only
    // when a label's text or position actually changes.
    let y_labels = {
        let label_style = label_style.clone();
        // TWO nested switches, and the split is load-bearing.
        //
        // The OUTER one is keyed on the reserved width alone — which is
        // spec-derived, so it changes when an author turns the axis off
        // and effectively never otherwise. It owns the box that HOLDS the
        // gutter's space in the plot row.
        //
        // The INNER one carries the label churn. Folding them into one
        // switch keyed on `(width, labels)` — which is what this was —
        // tore the whole gutter out of the flex row every time a label
        // moved, and the labels carry PIXEL positions, so they move
        // whenever the plot resizes. That closed a loop with no fixed
        // point: gutter torn down -> the `flex_grow: 1` plot takes its
        // 44px -> the wider plot is measured and written to `plot_w` ->
        // `output` recomputes -> new label positions -> new key -> gutter
        // torn down again. Measured on iOS at ~125 relayouts/second with
        // the plot size cycling 312x166 / 356x166 / 312x188 — the swings
        // are exactly the two gutters — and the canvas repainting on
        // every one of them, which is what made the chart flicker.
        switch(
            move || gutters.get().0,
            move |w: &f32| {
                let w = *w;
                let label_style = label_style.clone();
                let inner = switch(
                    move || {
                        output
                            .get()
                            .scene
                            .labels
                            .iter()
                            .filter(|l| matches!(l.role, LabelRole::AxisY | LabelRole::AxisTitleY))
                            .cloned()
                            .collect::<Vec<_>>()
                    },
                    move |labels: &Vec<LabelPlacement>| {
                        view(
                            labels
                                .iter()
                                .map(|l| label_element(l, label_style.clone(), w))
                                .collect(),
                        )
                        .with_style(sheet(StyleRules {
                            // Fills the gutter without sizing it, so the
                            // rebuild above can never move the plot.
                            position: Some(Position::Absolute),
                            left: px(0.0),
                            top: px(0.0),
                            right: px(0.0),
                            bottom: px(0.0),
                            ..Default::default()
                        }))
                        .into_element()
                    },
                );
                view(vec![inner])
                    .with_style(sheet(StyleRules {
                        width: px(w),
                        // Fixed gutter: never let the growing plot squeeze it.
                        flex_shrink: Some(Tokenized::Literal(0.0)),
                        position: Some(Position::Relative),
                        ..Default::default()
                    }))
                    .into_element()
            },
        )
    };

    let x_labels = {
        let label_style = label_style.clone();
        // Same split as the y gutter above, and for the same reason —
        // see that comment. The outer key stays `gutters` whole because
        // the x labels are offset by the Y gutter to get back into
        // plot-local x, so this box depends on BOTH reserved sizes; both
        // are spec-derived and stable.
        switch(
            move || gutters.get(),
            move |(y_w, h): &(f32, f32)| {
                let (y_w, h) = (*y_w, *h);
                let label_style = label_style.clone();
                let inner = switch(
                    move || {
                        output
                            .get()
                            .scene
                            .labels
                            .iter()
                            .filter(|l| matches!(l.role, LabelRole::AxisX | LabelRole::AxisTitleX))
                            .cloned()
                            .collect::<Vec<_>>()
                    },
                    move |labels: &Vec<LabelPlacement>| {
                        view(
                            labels
                                .iter()
                                .map(|l| label_element(l, label_style.clone(), y_w))
                                .collect(),
                        )
                        .with_style(sheet(StyleRules {
                            position: Some(Position::Absolute),
                            left: px(0.0),
                            top: px(0.0),
                            right: px(0.0),
                            bottom: px(0.0),
                            ..Default::default()
                        }))
                        .into_element()
                    },
                );
                view(vec![inner])
                    .with_style(sheet(StyleRules {
                        height: px(h),
                        flex_shrink: Some(Tokenized::Literal(0.0)),
                        position: Some(Position::Relative),
                        ..Default::default()
                    }))
                    .into_element()
            },
        )
    };

    // Legend: a real flex row of swatch + name, rebuilt when the visible
    // series change. Keyed on (name, color, visible) so recoloring or
    // toggling a series updates it and nothing else does.
    let legend = {
        let spec_for_legend = spec.clone();
        let label_style = label_style.clone();
        switch(
            move || {
                let s = spec_for_legend.get();
                let rows = if s.legend {
                    s.series
                        .iter()
                        // A heatmap row has a ramp, not a color, so a
                        // one-swatch entry for it would misstate how to read
                        // the chart. Matches what the core omits.
                        .filter(|se| !matches!(se.kind, charts_core::SeriesKind::Heatmap(_)))
                        .map(|se| (se.name.clone(), se.color, se.visible))
                        .collect::<Vec<(String, MarkColor, bool)>>()
                } else {
                    Vec::new()
                };
                (gutters.get().0, rows)
            },
            move |(y_w, entries): &(f32, Vec<(String, MarkColor, bool)>)| {
                // No entries means no row at all — an empty box still costs
                // its padding, which is exactly the space a sparkline is
                // trying to reclaim.
                if entries.iter().all(|(_, _, visible)| !visible) {
                    return view(Vec::new()).into_element();
                }
                view(entries
                    .iter()
                    .filter(|(_, _, visible)| *visible)
                    .map(|(name, color, _)| legend_entry(name, *color, label_style.clone()))
                    .collect())
                    .with_style(sheet(StyleRules {
                        flex_direction: Some(FlexDirection::Row),
                        align_items: Some(AlignItems::Center),
                        column_gap: px(14.0),
                        flex_shrink: Some(Tokenized::Literal(0.0)),
                        padding_bottom: px(6.0),
                        padding_left: px(*y_w),
                        ..Default::default()
                    }))
                    .into_element()
            },
        )
    };

    let plot_row = view(vec![y_labels, plot])
        .with_style(sheet(StyleRules {
            flex_direction: Some(FlexDirection::Row),
            flex_grow: Some(Tokenized::Literal(1.0)),
            // Sized by grow alone, not by content — see `root_rules`.
            flex_basis: px(0.0),
            min_height: px(0.0),
            ..Default::default()
        }))
        .into_element();

    let root_style = props.style.clone().unwrap_or_else(|| sheet(root_rules()));
    view(vec![legend, plot_row, x_labels]).with_style(root_style).into_element()
}

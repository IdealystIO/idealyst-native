//! The `Chart` component — `charts-core` bound to idealyst.
//!
//! # The split
//!
//! Marks go through a `Canvas`; axis labels and the tooltip are ordinary
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
//!   ticks does not churn the text nodes;
//! - the tooltip is a `when` over the hover signal.
//!
//! The memo matters: without it the painter and the label layer would each
//! re-run a full render on every change.

use std::cell::RefCell;
use std::rc::Rc;

use canvas_core::{Canvas, CanvasProps, Scene};
use charts_core::{
    render_tween, render_with, ChartOutput, ChartSpec, Color as MarkColor, DatumRef, Gutters,
    HAlign, HitResult, LabelPlacement, LabelRole, Rect as IrRect,
};
use runtime_core::{
    after_animation_frame, component, memo, on_scope_drop, signal, switch, view, when, AlignItems,
    Element, FlexDirection, IdealystSchema, IntoElement, LayoutSubscription, Length, Overflow,
    PointerEvents, Position, Reactive, Ref, StyleRules, StyleSheet, TextAlign, Tokenized,
    AnchorableHandle, StyleApplication, Transform, ViewHandle,
};
use runtime_core::Color as StyleColor;
use runtime_core::{Signal, TouchEvent, TouchPhase, TouchResponse};

/// What the pointer is currently over.
#[derive(Clone, PartialEq, Debug)]
pub struct ChartHover {
    /// Pointer position in plot-local pixels.
    pub x: f32,
    pub y: f32,
    /// Every series' datum in the hovered column, ordered by series. This
    /// is a column rather than a single nearest datum because that is what
    /// a multi-series tooltip shows — see `HitIndex::column_at`.
    pub entries: Vec<HitResult>,
}

/// Renders the tooltip body for a hovered column.
pub type TooltipRenderer = Rc<dyn Fn(&ChartHover) -> Element>;
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
    /// Show the built-in tooltip on hover. Default `true`. Turn it off to
    /// render your own from [`ChartProps::on_hover`].
    pub tooltip: bool,
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
    /// Animate data changes over this many milliseconds. `0` disables it.
    ///
    /// Values and the axis domain glide; tick labels switch to the new ones
    /// immediately so they do not churn through intermediate numbers. A
    /// change that alters the chart's SHAPE — a series added or removed, a
    /// series' length changed, a kind swapped — snaps, because pairing
    /// unrelated points animates a bar toward a value it has nothing to do
    /// with, which reads worse than not animating.
    pub transition_ms: u32,
    /// Points to render as selected. Reactive, so a host can drive selection
    /// from its own signal without rebuilding the whole spec.
    ///
    /// `#[prop(reactive)]` because `#[props]` treats a bare `Vec` as children
    /// and would otherwise leave it non-reactive.
    #[prop(reactive)]
    #[schema(constraint = "reactive: Vec<DatumRef> of selected points")]
    pub selected: Vec<DatumRef>,
    /// Render the tooltip body yourself. Receives the hovered column;
    /// return any element. `None` uses the built-in list.
    ///
    /// `#[prop(static)]` is REQUIRED here and is not redundant: `#[props]`
    /// recognises handlers by matching the literal path segments
    /// `Rc`/`Arc`/`Box` followed by `dyn Fn`, so a type alias hides the
    /// handler from it and the field would be wrapped as
    /// `Reactive<Option<TooltipRenderer>>` — which then rejects a plain
    /// `Some(..)`/`None` at every call site.
    #[prop(static)]
    #[schema(constraint = "optional Fn(&ChartHover) -> Element")]
    pub tooltip_content: Option<TooltipRenderer>,
    /// Called whenever the hovered column changes, and with `None` when the
    /// pointer leaves. Fires regardless of whether the built-in tooltip is
    /// on, so an app can drive a legend, a readout, or a linked chart.
    /// `#[prop(static)]` for the same aliasing reason as `tooltip_content`.
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
            tooltip: Reactive::Static(true),
            highlight_on_hover: Reactive::Static(true),
            dim_others: Reactive::Static(false),
            transition_ms: Reactive::Static(0),
            selected: Reactive::Static(Vec::new()),
            tooltip_content: None,
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
            .field("tooltip_content", &self.tooltip_content.is_some())
            .field("on_hover", &self.on_hover.is_some())
            .field("label_style", &self.label_style.is_some())
            .field("style", &self.style.is_some())
            .finish_non_exhaustive()
    }
}

fn sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

fn px(v: f32) -> Option<Tokenized<Length>> {
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

/// The tooltip bubble.
///
/// The width bounds are load-bearing, not cosmetic. An absolutely-positioned
/// box with `left`/`top` set and no width is shrink-to-fit, and the two
/// layout engines disagree about what that means: the DOM gives it
/// max-content, while Taffy (macOS/iOS/Android) resolves it toward
/// MIN-content — which for a text run is the widest single character, so the
/// label wraps one letter per line into a tall vertical ribbon. A definite
/// `min_width` gives Taffy a floor to lay out against, and `max_width` keeps
/// a long series name from stretching the bubble across the plot on the DOM
/// side. Same bubble on every backend, no per-platform branch.
fn tooltip_rules() -> StyleRules {
    StyleRules {
        position: Some(Position::Absolute),
        min_width: px(104.0),
        max_width: px(260.0),
        padding_top: px(6.0),
        padding_bottom: px(6.0),
        padding_left: px(8.0),
        padding_right: px(8.0),
        border_top_left_radius: px(6.0),
        border_top_right_radius: px(6.0),
        border_bottom_left_radius: px(6.0),
        border_bottom_right_radius: px(6.0),
        flex_direction: Some(FlexDirection::Column),
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
    t: f32,
) -> Option<ChartSpec> {
    match (origin, target) {
        (Some(o), Some(tg)) if t < 1.0 => {
            Some(charts_core::lerp_data(o, tg, t).unwrap_or_else(|| tg.clone()))
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
pub fn hover_at(out: &ChartOutput, x: f32, y: f32) -> Option<ChartHover> {
    let entries = out.hit.column_at(charts_core::pt(x, y));
    if entries.is_empty() {
        None
    } else {
        Some(ChartHover { x, y, entries })
    }
}

/// One legend entry: a color swatch and the series name.
///
/// Built from the SPEC, not from the `LabelRole::Legend` placements the core
/// emits. Those placements carry a naive fixed spacing because the core
/// cannot measure text — which is exactly the case the core documents as
/// "the host lays these out". Here the host is a layout engine, so a flex
/// row does it properly and the placements go unused.
fn legend_entry(name: &str, color: MarkColor, label_style: Rc<StyleSheet>) -> Element {
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

fn default_tooltip_body(h: &ChartHover, spec: &ChartSpec) -> Vec<Element> {
    let mut rows: Vec<Element> = Vec::with_capacity(h.entries.len());
    for e in &h.entries {
        let name = spec
            .series
            .get(e.series)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let line = format!("{name}: {}", e.datum.y);
        rows.push(
            runtime_core::text(line)
                .with_style(sheet(StyleRules { font_size: px(12.0), ..Default::default() }))
                .into_element(),
        );
    }
    rows
}

/// Renders a chart: marks on a canvas, labels and tooltip as real elements.
#[component]
pub fn Chart(props: &ChartProps) -> Element {
    let spec = props.spec.clone();
    let y_axis_width = props.y_axis_width.get();
    let x_axis_height = props.x_axis_height.get();
    let show_tooltip = props.tooltip.get();
    let highlight_on_hover = props.highlight_on_hover.get();
    let dim_others = props.dim_others.get();
    let transition_ms = props.transition_ms.get();
    let selected = props.selected.clone();
    let label_style = props.label_style.clone().unwrap_or_else(|| sheet(label_rules()));

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
    let progress = signal(1.0_f32);
    let anim: Rc<RefCell<Option<runtime_core::scheduling::RafLoop>>> = Rc::new(RefCell::new(None));
    // Holds the one-shot that tears the frame loop down. It cannot be
    // dropped from inside the loop's own callback (that would re-enter the
    // RefCell the callback is running under), so the stop is deferred by a
    // frame and parked here.
    let anim_stop: Rc<RefCell<Option<runtime_core::ScheduledTask>>> = Rc::new(RefCell::new(None));

    if transition_ms > 0 {
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
                visual_state(origin.get().as_ref(), target.get().as_ref(), progress.get())
            });

            match visual {
                // First spec ever: nothing to animate from.
                None => {
                    origin.set(Some(next.clone()));
                    target.set(Some(next));
                    progress.set(1.0);
                }
                Some(v) if v == next => {
                    // Same values — keep the target current (colors or
                    // highlight may still have changed) but do not animate.
                    target.set(Some(next));
                }
                Some(v) => {
                    origin.set(Some(v));
                    target.set(Some(next));
                    progress.set(0.0);
                    stop_slot.borrow_mut().take();

                    // Drive `progress` from wall-clock elapsed rather than a
                    // per-frame increment, so the duration is honest on a
                    // slow frame and cannot run long on a fast display.
                    let start = runtime_core::time::now_micros();
                    let dur = (transition_ms as u64).max(1) * 1000;
                    let inner = anim_slot.clone();
                    let stop_slot = stop_slot.clone();
                    *anim_slot.borrow_mut() =
                        Some(runtime_core::scheduling::raf_loop(move || {
                            let elapsed =
                                runtime_core::time::now_micros().saturating_sub(start);
                            let t = (elapsed as f32 / dur as f32).min(1.0);
                            progress.set(t);
                            if t >= 1.0 && stop_slot.borrow().is_none() {
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
            let t = progress.get();
            match origin.get() {
                Some(from) if transition_ms > 0 && t < 1.0 => {
                    render_tween(&from, &s, t, rect, &Gutters::None)
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
            if r.width > 0.5 {
                plot_w.set(r.width);
            }
            if r.height > 0.5 {
                plot_h.set(r.height);
            }
            h.on_layout(move |w, hgt| {
                // Guard the write: layout fires on every pass, and an
                // unconditional set would re-run the memo, the painter and
                // the label switch on every unrelated relayout.
                if (plot_w.get() - w).abs() > 0.5 {
                    plot_w.set(w);
                }
                if (plot_h.get() - hgt).abs() > 0.5 {
                    plot_h.set(hgt);
                }
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
                    let next = hover_at(&out, ev.position.x, ev.position.y);
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

    // --- tooltip -----------------------------------------------------------
    // Two reactive layers, on purpose.
    //
    // `when` keys on its predicate's BOOLEAN (that is its documented dedup:
    // a predicate reading extra signals must not rebuild on those extras).
    // So a tooltip built entirely inside `when(hover.is_some())` mounts once
    // and then NEVER updates — it displays whichever datum you hovered first
    // while the readout beside it moves on. That was a real, visible bug.
    //
    // So: `when` only decides existence. Position rides a reactive STYLE
    // closure, which re-resolves without rebuilding anything. Content rides
    // a `switch` keyed on the hovered data, which rebuilds only when the
    // pointer crosses into a different column — not on every pixel of
    // movement.
    let tooltip_content = props.tooltip_content.clone();
    let spec_for_tip = spec.clone();
    let tooltip = when(
        move || show_tooltip && hover.get().is_some(),
        move || {
            let tooltip_content = tooltip_content.clone();
            let spec_for_tip = spec_for_tip.clone();
            let rows = switch(
                move || {
                    hover
                        .get()
                        .map(|h| h.entries.iter().map(|e| (e.series, e.datum.y)).collect::<Vec<_>>())
                        .unwrap_or_default()
                },
                move |_key: &Vec<(usize, f64)>| {
                    let Some(h) = hover.get() else {
                        return view(Vec::new()).into_element();
                    };
                    let body = match &tooltip_content {
                        Some(f) => vec![f(&h)],
                        None => default_tooltip_body(&h, &spec_for_tip.get()),
                    };
                    view(body)
                        .with_style(sheet(StyleRules {
                            flex_direction: Some(FlexDirection::Column),
                            ..Default::default()
                        }))
                        .into_element()
                },
            );
            view(vec![rows])
                .with_style(move || {
                    let (x, y) = hover.get().map(|h| (h.x, h.y)).unwrap_or((0.0, 0.0));
                    let mut rules = tooltip_rules();
                    // Flip to the left of the cursor past the midpoint so
                    // the bubble stays inside the plot instead of being
                    // clipped by its `overflow: hidden`.
                    if x > plot_w.get() * 0.6 {
                        rules.right = px((plot_w.get() - x + 12.0).max(0.0));
                    } else {
                        rules.left = px(x + 12.0);
                    }
                    rules.top = px((y - 12.0).max(0.0));
                    StyleApplication::new(sheet(rules))
                })
                .into_element()
        },
        || {
            view(Vec::new())
                .with_style(sheet(StyleRules {
                    position: Some(Position::Absolute),
                    ..Default::default()
                }))
                .into_element()
        },
    );

    // --- plot area ---------------------------------------------------------
    let plot = view(vec![canvas, tooltip])
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
        switch(
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
                view(labels.iter().map(|l| label_element(l, label_style.clone(), y_axis_width)).collect())
                    .with_style(sheet(StyleRules {
                        width: px(y_axis_width),
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
        switch(
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
                view(labels.iter().map(|l| label_element(l, label_style.clone(), y_axis_width)).collect())
                    .with_style(sheet(StyleRules {
                        height: px(x_axis_height),
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
                if !s.legend {
                    return Vec::new();
                }
                s.series
                    .iter()
                    .map(|se| (se.name.clone(), se.color, se.visible))
                    .collect::<Vec<(String, MarkColor, bool)>>()
            },
            move |entries: &Vec<(String, MarkColor, bool)>| {
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
                        padding_left: px(y_axis_width),
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

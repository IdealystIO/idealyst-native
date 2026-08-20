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
    render_with, ChartOutput, ChartSpec, Gutters, HAlign, HitResult, LabelPlacement, LabelRole,
    Rect as IrRect,
};
use runtime_core::{
    after_animation_frame, component, memo, on_scope_drop, signal, switch, view, when, Element,
    FlexDirection, IdealystSchema, IntoElement, LayoutSubscription, Length, Overflow,
    PointerEvents, Position, Reactive, Ref, StyleRules, StyleSheet, TextAlign, Tokenized,
    ViewHandle,
};
use runtime_core::{TouchEvent, TouchPhase, TouchResponse};

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
fn root_rules() -> StyleRules {
    StyleRules {
        flex_direction: Some(FlexDirection::Column),
        flex_grow: Some(Tokenized::Literal(1.0)),
        width: Some(Tokenized::Literal(Length::pct(100.0))),
        height: Some(Tokenized::Literal(Length::pct(100.0))),
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

fn tooltip_rules() -> StyleRules {
    StyleRules {
        position: Some(Position::Absolute),
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

/// One tick label, absolutely placed against the gutter it lives in.
fn tick_label(l: &LabelPlacement, style: Rc<StyleSheet>, y_axis_width: f32) -> Element {
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
        _ => {
            // x labels live in the gutter below the plot, which starts at
            // the chart's left edge — so shift by the y gutter to get back
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
    let label_style = props.label_style.clone().unwrap_or_else(|| sheet(label_rules()));

    // Measured plot size. Starts at zero: nothing draws until the first
    // layout callback arrives, which is one frame after mount. Drawing
    // against a guessed size instead would paint a visibly wrong chart and
    // then jump.
    let plot_w = signal(0.0_f32);
    let plot_h = signal(0.0_f32);
    let plot_ref: Ref<ViewHandle> = Ref::new();

    let hover = signal(None::<ChartHover>);

    // The single render. Everything else reads this.
    let output = {
        let spec = spec.clone();
        memo(move || {
            let (w, h) = (plot_w.get(), plot_h.get());
            render_with(&spec.get(), IrRect::new(0.0, 0.0, w, h), &Gutters::None)
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
    let tooltip_content = props.tooltip_content.clone();
    let spec_for_tip = spec.clone();
    let tooltip = when(
        move || show_tooltip && hover.get().is_some(),
        move || {
            let Some(h) = hover.get() else {
                return view(Vec::new()).into_element();
            };
            let body = match &tooltip_content {
                Some(f) => vec![f(&h)],
                None => default_tooltip_body(&h, &spec_for_tip.get()),
            };
            let mut rules = tooltip_rules();
            // Flip to the left of the cursor near the right edge so the
            // bubble stays inside the plot instead of being clipped.
            let flip = h.x > plot_w.get() * 0.6;
            if flip {
                rules.right = px((plot_w.get() - h.x + 12.0).max(0.0));
            } else {
                rules.left = px(h.x + 12.0);
            }
            rules.top = px((h.y - 12.0).max(0.0));
            view(body).with_style(sheet(rules)).into_element()
        },
        || view(Vec::new()).with_style(sheet(StyleRules {
            position: Some(Position::Absolute),
            ..Default::default()
        })).into_element(),
    );

    // --- plot area ---------------------------------------------------------
    let plot = view(vec![canvas, tooltip])
        .bind(plot_ref)
        .on_touch(move |ev| touch(ev))
        .on_hover(move |entering| {
            if !entering && hover.get().is_some() {
                hover.set(None);
                if let Some(cb) = &leave_cb {
                    cb(None);
                }
            }
        })
        .with_style(sheet(StyleRules {
            flex_grow: Some(Tokenized::Literal(1.0)),
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
                    .filter(|l| l.role == LabelRole::AxisY)
                    .cloned()
                    .collect::<Vec<_>>()
            },
            move |labels: &Vec<LabelPlacement>| {
                view(labels.iter().map(|l| tick_label(l, label_style.clone(), y_axis_width)).collect())
                    .with_style(sheet(StyleRules {
                        width: px(y_axis_width),
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
                    .filter(|l| l.role != LabelRole::AxisY)
                    .cloned()
                    .collect::<Vec<_>>()
            },
            move |labels: &Vec<LabelPlacement>| {
                view(labels.iter().map(|l| tick_label(l, label_style.clone(), y_axis_width)).collect())
                    .with_style(sheet(StyleRules {
                        height: px(x_axis_height),
                        position: Some(Position::Relative),
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
            ..Default::default()
        }))
        .into_element();

    let root_style = props.style.clone().unwrap_or_else(|| sheet(root_rules()));
    view(vec![plot_row, x_labels]).with_style(root_style).into_element()
}

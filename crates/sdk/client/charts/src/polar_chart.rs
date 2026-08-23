//! The `PieChart` and `RadialChart` components.
//!
//! Same split as [`Chart`](crate::Chart): marks go through a `Canvas`, text
//! is real `text` primitives. Polar charts lean on that harder than
//! cartesian ones do — a donut's center readout and its slice labels are the
//! chart's whole point, and drawn into a canvas they would miss the app's
//! fonts, its theme colors, and platform text scaling.
//!
//! # Why the two components share a body
//!
//! A pie and a radial gauge differ only in their spec type and their
//! renderer. Everything else — measuring the plot, driving a transition,
//! materializing labels, hit-testing a wedge, reporting hover — is identical, so
//! it lives once in [`polar_body`], parameterized by four function pointers.
//! Two full copies would drift, and the drift would be invisible until one
//! of them stopped animating.

use std::cell::RefCell;
use std::rc::Rc;

use canvas_core::{Canvas, CanvasProps, Scene};
use charts_core::{
    lerp_pie, lerp_radial, render_pie, render_pie_tween, render_radial, render_radial_tween,
    Color as MarkColor, HitResult, LabelPlacement, LabelRole, PieSpec, PolarOutput, RadialSpec,
    Rect as IrRect, SliceHighlight, TweenAt,
};
use runtime_core::{
    after_animation_frame, component, memo, on_scope_drop, signal, switch, view, AlignItems,
    AnchorableHandle, Element, FlexDirection, IdealystSchema, IntoElement, LayoutSubscription,
    Length, Overflow, PointerEvents, Position, Reactive, Ref, Signal, StyleRules,
    StyleSheet, Tokenized, TouchEvent, TouchPhase, TouchResponse, Transition, ViewHandle,
};

use crate::chart::{
    label_sheet, overlay_label, pointer_frame, px, sheet, tween_at, transition_span_ms,
    PointerFrame,
};

/// What the pointer is over in a polar chart.
///
/// One slice, not a column: a radial chart has no shared x, so the "every
/// series at this position" question a cartesian readout answers does not
/// arise. `None` from a hit means the pointer is in the hole or outside the
/// ring, which is genuinely nothing rather than a near miss.
#[derive(Clone, PartialEq, Debug)]
pub struct PolarHover {
    /// Where the pointer is, in every space. See [`PointerFrame`].
    pub at: PointerFrame,
    /// Index into the spec's `slices` / `bars`.
    pub index: usize,
    /// The slice's own label, so a caller needs no second lookup.
    pub label: String,
    pub value: f64,
    /// The underlying hit — its `position` is the wedge's centroid and its
    /// `bounds` the wedge itself, so a caller can place a surface along the
    /// slice's own bisector at whatever radius it likes.
    pub hit: HitResult,
}
/// Notified whenever the hovered slice changes; `None` on leave.
pub type PolarHoverCallback = Rc<dyn Fn(Option<PolarHover>)>;


/// Resolve a pointer position into a hovered slice.
///
/// Pure, so the pixel-to-slice mapping is testable without a backend or a
/// synthetic event stream. Containment, not proximity: a wedge has area, and
/// its centroid is nowhere near most of it (see `HitIndex::contains`).
pub fn polar_hover_at(
    out: &PolarOutput,
    labels: &[String],
    at: PointerFrame,
) -> Option<PolarHover> {
    let hit = out.hit.contains(charts_core::pt(at.local.x, at.local.y))?;
    Some(PolarHover {
        at,
        index: hit.index,
        label: labels.get(hit.index).cloned().unwrap_or_default(),
        value: hit.datum.y,
        hit,
    })
}

fn legend_row(entries: &[LegendEntry], label_style: Rc<StyleSheet>) -> Element {
    view(
        entries
            .iter()
            .filter(|(_, _, visible)| *visible)
            .map(|(name, color, _)| crate::chart::legend_entry(name, *color, label_style.clone()))
            .collect(),
    )
    .with_style(sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        column_gap: px(14.0),
        flex_shrink: Some(Tokenized::Literal(0.0)),
        padding_bottom: px(6.0),
        ..Default::default()
    }))
    .into_element()
}

/// One legend row: name, swatch color, and whether the entry is visible.
type LegendEntry = (String, MarkColor, bool);

/// Everything both polar components do, parameterized by the four things
/// that actually differ between them.
struct PolarOps<S: 'static> {
    render: fn(&S, IrRect) -> PolarOutput,
    lerp: fn(&S, &S, TweenAt) -> Option<S>,
    /// The TWEEN render, not `lerp` + `render`.
    ///
    /// A `SliceStyleFn` has to be resolved at BOTH ends and the two colours
    /// interpolated, and only the tween entry point does that — going
    /// through `lerp` and rendering the result would resolve the callback
    /// once, against the tweened value, and a threshold recolor would flip
    /// mid-transition instead of fading.
    render_tween: fn(&S, &S, TweenAt, IrRect) -> PolarOutput,
    /// Apply hover/selection emphasis to a copy of the spec.
    emphasise: fn(&mut S, SliceHighlight),
    /// `(label, color, visible)` per entry, and whether a legend was asked
    /// for. Used for the legend row and to label a hit.
    entries: fn(&S) -> (Vec<LegendEntry>, bool),
}

struct PolarConfig<S: 'static> {
    spec: Reactive<S>,
    value_transition: Option<Transition>,
    color_transition: Option<Transition>,
    highlight_on_hover: bool,
    dim_others: bool,
    selected: Reactive<Vec<usize>>,
    on_hover: Option<PolarHoverCallback>,
    label_style: Option<Rc<StyleSheet>>,
    style: Option<Rc<StyleSheet>>,
}

/// The root box: a column of [legend] over [plot].
///
/// Same height-chain requirement as the cartesian root — see
/// [`crate::chart`]'s `root_rules` for the full story. A polar chart is
/// square-ish by nature but still has to be told a definite height, or the
/// canvas's `height: 100%` resolves against nothing.
fn polar_root_rules() -> StyleRules {
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

fn polar_body<S: Clone + PartialEq + 'static>(cfg: PolarConfig<S>, ops: PolarOps<S>) -> Element {
    let PolarConfig {
        spec,
        value_transition,
        color_transition,
        highlight_on_hover,
        dim_others,
        selected,
        on_hover: on_hover_cb,
        label_style,
        style,
    } = cfg;
    let label_style = label_style.unwrap_or_else(label_sheet);

    let plot_w = signal(0.0_f32);
    let plot_h = signal(0.0_f32);
    let plot_ref: Ref<ViewHandle> = Ref::new();

    let hover = signal(None::<PolarHover>);
    // The hovered INDEX, separate from `hover` for the same reason the
    // cartesian chart keeps `hover_col` separate: `hover` carries pixel
    // coordinates and changes on every pointer move, and the render memo
    // must not re-run per pixel.
    let hover_idx = signal(None::<usize>);

    // --- transition state (mirrors `Chart`; see its comments) --------------
    let origin: Signal<Option<S>> = signal(None);
    let target: Signal<Option<S>> = signal(None);
    let elapsed = signal(f32::INFINITY);
    let span_ms = transition_span_ms(value_transition, color_transition);
    let anim: Rc<RefCell<Option<runtime_core::scheduling::RafLoop>>> = Rc::new(RefCell::new(None));
    let anim_stop: Rc<RefCell<Option<runtime_core::ScheduledTask>>> = Rc::new(RefCell::new(None));

    if span_ms > 0 {
        let spec_for_anim = spec.clone();
        let anim_slot = anim.clone();
        let stop_slot = anim_stop.clone();
        let lerp = ops.lerp;
        runtime_core::effect!({
            let next = spec_for_anim.get();
            let visual = runtime_core::untrack(|| {
                let (o, tg) = (origin.get(), target.get());
                let at = tween_at(value_transition, color_transition, elapsed.get());
                match (o, tg) {
                    (Some(o), Some(tg)) if at.value < 1.0 || at.color < 1.0 => {
                        Some(lerp(&o, &tg, at).unwrap_or(tg))
                    }
                    (_, tg) => tg,
                }
            });
            match visual {
                None => {
                    origin.set(Some(next.clone()));
                    target.set(Some(next));
                    elapsed.set(f32::INFINITY);
                }
                Some(v) if v == next => target.set(Some(next)),
                Some(v) => {
                    origin.set(Some(v));
                    target.set(Some(next));
                    elapsed.set(0.0);
                    stop_slot.borrow_mut().take();
                    let start = runtime_core::time::now_micros();
                    let span_us = (span_ms as u64).max(1) * 1000;
                    let inner = anim_slot.clone();
                    let stop_slot = stop_slot.clone();
                    *anim_slot.borrow_mut() =
                        Some(runtime_core::scheduling::raf_loop(move || {
                            let us = runtime_core::time::now_micros().saturating_sub(start);
                            elapsed.set(us as f32 / 1000.0);
                            if us >= span_us && stop_slot.borrow().is_none() {
                                let slot = inner.clone();
                                let task = after_animation_frame(move || {
                                    slot.borrow_mut().take();
                                });
                                *stop_slot.borrow_mut() = Some(task);
                            }
                        }));
                }
            }
        });
    }

    {
        let anim = anim.clone();
        let anim_stop = anim_stop.clone();
        on_scope_drop(move || {
            anim.borrow_mut().take();
            anim_stop.borrow_mut().take();
        });
    }

    let output = {
        let spec = spec.clone();
        let selected = selected.clone();
        let (render, render_tween, emphasise) =
            (ops.render, ops.render_tween, ops.emphasise);
        memo(move || {
            let (w, h) = (plot_w.get(), plot_h.get());
            let mut s = spec.get();

            let mut hl = SliceHighlight::default();
            if highlight_on_hover {
                hl.hovered = hover_idx.get();
            }
            hl.selected = selected.get();
            hl.dim_others = dim_others;
            if hl.dim_others && hl.dim_opacity <= 0.0 {
                hl.dim_opacity = 0.35;
            }
            emphasise(&mut s, hl);

            let rect = IrRect::new(0.0, 0.0, w, h);
            let at = tween_at(value_transition, color_transition, elapsed.get());
            match origin.get() {
                Some(from) if span_ms > 0 && (at.value < 1.0 || at.color < 1.0) => {
                    // Emphasis is applied to the DESTINATION only, and the
                    // tween render falls back to it when the pair cannot be
                    // matched — so the hovered slice never loses its
                    // highlight mid-flight.
                    render_tween(&from, &s, at, rect)
                }
                _ => render(&s, rect),
            }
        })
    };

    // --- plot size subscription (see `Chart` for why it seeds first) -------
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

    let canvas = Canvas(CanvasProps {
        draw: canvas_core::draw(move |s: &mut Scene| {
            let out = output.get();
            crate::adapt::marks_into_scene(&out.scene.marks, s, 0.0, 0.0);
        }),
        ..Default::default()
    })
    .into_element();

    // --- pointer -----------------------------------------------------------
    let labels_of = {
        let spec = spec.clone();
        let entries = ops.entries;
        move || entries(&spec.get()).0.into_iter().map(|(n, _, _)| n).collect::<Vec<String>>()
    };
    let touch = {
        let on_hover_cb = on_hover_cb.clone();
        let labels_of = labels_of.clone();
        move |ev: &TouchEvent| -> TouchResponse {
            match ev.phase {
                TouchPhase::Hovered | TouchPhase::Moved | TouchPhase::Began => {
                    let out = output.get();
                    let next =
                        polar_hover_at(&out, &labels_of(), pointer_frame(plot_ref, ev));
                    let idx = next.as_ref().map(|h| h.index);
                    if hover_idx.get() != idx {
                        hover_idx.set(idx);
                    }
                    if hover.get() != next {
                        hover.set(next.clone());
                        if let Some(cb) = &on_hover_cb {
                            cb(next);
                        }
                    }
                    TouchResponse::IGNORED
                }
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    if hover_idx.get().is_some() {
                        hover_idx.set(None);
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

    // --- labels ------------------------------------------------------------
    // Keyed on the placements themselves, so text nodes are rebuilt only
    // when a label's text or position actually changes — a hover that only
    // recolors a wedge does not churn them.
    let labels = {
        let label_style = label_style.clone();
        switch(
            move || {
                output
                    .get()
                    .scene
                    .labels
                    .iter()
                    .filter(|l| l.role != LabelRole::Legend)
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

    let plot = view(vec![canvas, labels])
        .bind(plot_ref)
        .on_touch(move |ev| touch(ev))
        .on_hover(move |entering| {
            if !entering && hover_idx.get().is_some() {
                hover_idx.set(None);
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
            min_height: px(0.0),
            position: Some(Position::Relative),
            overflow: Some(Overflow::Hidden),
            ..Default::default()
        }))
        .into_element();

    let legend = {
        let spec = spec.clone();
        let entries = ops.entries;
        let label_style = label_style.clone();
        switch(
            move || {
                let (rows, on) = entries(&spec.get());
                if on {
                    rows
                } else {
                    Vec::new()
                }
            },
            move |rows: &Vec<LegendEntry>| legend_row(rows, label_style.clone()),
        )
    };

    let root_style = style.unwrap_or_else(|| sheet(polar_root_rules()));
    view(vec![legend, plot]).with_style(root_style).into_element()
}

// ---------------------------------------------------------------------------
// PieChart
// ---------------------------------------------------------------------------

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct PieChartProps {
    /// The pie or donut to draw.
    #[schema(constraint = "reactive: static PieSpec or Signal/rx!")]
    pub spec: PieSpec,
    /// Let hovering drive the spec's highlight — the hovered slice grows and
    /// pulls out per `hover_grow` / `hover_explode`. Default `true`.
    pub highlight_on_hover: bool,
    /// Fade the slices that are neither hovered nor selected. Default
    /// `false`.
    pub dim_others: bool,
    /// Animate values. `None` (the default) snaps. A change in the
    /// number of slices snaps too.
    ///
    /// Same [`Transition`] the style system uses — one vocabulary. See
    /// [`ChartProps::value_transition`](crate::ChartProps::value_transition).
    #[prop(static)]
    #[schema(constraint = "optional Transition { duration_ms, easing }")]
    pub value_transition: Option<Transition>,
    /// Animate slices colours, including a `SliceStyleFn`'s answer. `None`
    /// (the default) snaps. See
    /// [`ChartProps::color_transition`](crate::ChartProps::color_transition).
    #[prop(static)]
    #[schema(constraint = "optional Transition { duration_ms, easing }")]
    pub color_transition: Option<Transition>,
    /// Slices to render as selected, by index. `#[prop(reactive)]` because
    /// `#[props]` treats a bare `Vec` as children.
    #[prop(reactive)]
    #[schema(constraint = "reactive: Vec<usize> of selected slice indices")]
    pub selected: Vec<usize>,
    /// Called whenever the hovered slice changes, and with `None` on leave.
    #[prop(static)]
    #[schema(constraint = "optional Fn(Option<PolarHover>)")]
    pub on_hover: Option<PolarHoverCallback>,
    /// Style for slice and legend labels.
    #[prop(static)]
    pub label_style: Option<Rc<StyleSheet>>,
    /// Style for the chart's root box.
    #[prop(static)]
    pub style: Option<Rc<StyleSheet>>,
}

impl Default for PieChartProps {
    fn default() -> Self {
        Self {
            spec: Reactive::Static(PieSpec::default()),
            highlight_on_hover: Reactive::Static(true),
            dim_others: Reactive::Static(false),
            value_transition: None,
            color_transition: None,
            selected: Reactive::Static(Vec::new()),
            on_hover: None,
            label_style: None,
            style: None,
        }
    }
}

impl std::fmt::Debug for PieChartProps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PieChartProps")
            .field("on_hover", &self.on_hover.is_some())
            .field("label_style", &self.label_style.is_some())
            .field("style", &self.style.is_some())
            .finish_non_exhaustive()
    }
}

/// A pie or donut chart.
#[component]
pub fn PieChart(props: &PieChartProps) -> Element {
    polar_body(
        PolarConfig {
            spec: props.spec.clone(),
            value_transition: props.value_transition,
            color_transition: props.color_transition,
            highlight_on_hover: props.highlight_on_hover.get(),
            dim_others: props.dim_others.get(),
            selected: props.selected.clone(),
            on_hover: props.on_hover.clone(),
            label_style: props.label_style.clone(),
            style: props.style.clone(),
        },
        PolarOps {
            render: render_pie,
            lerp: lerp_pie,
            render_tween: render_pie_tween,
            emphasise: |s: &mut PieSpec, hl| s.highlight = hl,
            entries: |s: &PieSpec| {
                (
                    s.slices.iter().map(|x| (x.label.clone(), x.color, x.visible)).collect(),
                    s.legend,
                )
            },
        },
    )
}

// ---------------------------------------------------------------------------
// RadialChart
// ---------------------------------------------------------------------------

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct RadialChartProps {
    /// The radial bar chart or gauge to draw.
    #[schema(constraint = "reactive: static RadialSpec or Signal/rx!")]
    pub spec: RadialSpec,
    /// Let hovering thicken the ring under the pointer. Default `true`.
    pub highlight_on_hover: bool,
    /// Fade the rings that are neither hovered nor selected. Default
    /// `false`.
    pub dim_others: bool,
    /// Animate values and the range. `None` (the default) snaps. A change in the
    /// number of rings snaps too.
    ///
    /// Same [`Transition`] the style system uses — one vocabulary. See
    /// [`ChartProps::value_transition`](crate::ChartProps::value_transition).
    #[prop(static)]
    #[schema(constraint = "optional Transition { duration_ms, easing }")]
    pub value_transition: Option<Transition>,
    /// Animate rings colours, including a `SliceStyleFn`'s answer. `None`
    /// (the default) snaps. See
    /// [`ChartProps::color_transition`](crate::ChartProps::color_transition).
    #[prop(static)]
    #[schema(constraint = "optional Transition { duration_ms, easing }")]
    pub color_transition: Option<Transition>,
    /// Rings to render as selected, by index.
    #[prop(reactive)]
    #[schema(constraint = "reactive: Vec<usize> of selected ring indices")]
    pub selected: Vec<usize>,
    /// Called whenever the hovered ring changes, and with `None` on leave.
    #[prop(static)]
    #[schema(constraint = "optional Fn(Option<PolarHover>)")]
    pub on_hover: Option<PolarHoverCallback>,
    /// Style for ring and legend labels.
    #[prop(static)]
    pub label_style: Option<Rc<StyleSheet>>,
    /// Style for the chart's root box.
    #[prop(static)]
    pub style: Option<Rc<StyleSheet>>,
}

impl Default for RadialChartProps {
    fn default() -> Self {
        Self {
            spec: Reactive::Static(RadialSpec::default()),
            highlight_on_hover: Reactive::Static(true),
            dim_others: Reactive::Static(false),
            value_transition: None,
            color_transition: None,
            selected: Reactive::Static(Vec::new()),
            on_hover: None,
            label_style: None,
            style: None,
        }
    }
}

impl std::fmt::Debug for RadialChartProps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RadialChartProps")
            .field("on_hover", &self.on_hover.is_some())
            .field("label_style", &self.label_style.is_some())
            .field("style", &self.style.is_some())
            .finish_non_exhaustive()
    }
}

/// A radial bar chart, or a gauge.
#[component]
pub fn RadialChart(props: &RadialChartProps) -> Element {
    polar_body(
        PolarConfig {
            spec: props.spec.clone(),
            value_transition: props.value_transition,
            color_transition: props.color_transition,
            highlight_on_hover: props.highlight_on_hover.get(),
            dim_others: props.dim_others.get(),
            selected: props.selected.clone(),
            on_hover: props.on_hover.clone(),
            label_style: props.label_style.clone(),
            style: props.style.clone(),
        },
        PolarOps {
            render: render_radial,
            lerp: lerp_radial,
            render_tween: render_radial_tween,
            emphasise: |s: &mut RadialSpec, hl| s.highlight = hl,
            entries: |s: &RadialSpec| {
                (
                    s.bars.iter().map(|b| (b.label.clone(), b.color, b.visible)).collect(),
                    s.legend,
                )
            },
        },
    )
}

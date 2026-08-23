+++
title = "Charts"
order = 75
tags = ["charts", "chart", "graph", "plot", "line", "bar", "pie", "donut", "gauge", "heatmap", "sparkline", "canvas", "sdk", "dataviz"]
+++

# Charts

Two crates. **`charts-core`** turns a spec plus a rectangle into a flat list of
vector marks, a list of text *placements*, and a hit index — it draws nothing
and knows about no UI toolkit. **`charts`** binds that to idealyst: marks go
onto a `Canvas`, labels become real `text` primitives, and hover is reported
through a callback.

```rust
use charts::prelude::*;

ui! {
    view(style = MyChartBox()) {
        Chart(spec = rx!(ChartSpec::new(vec![Series::new(
            "revenue",
            SeriesKind::smooth_line(),
            Color::rgb(0x4c, 0x8d, 0xff),
            data.get(),
        )])))
    }
}
```

**Requirement on the caller**: the chart's container must have a resolvable
height — a fixed height, or `flex_grow` inside a parent that has one. A chart
in a purely auto-height column has no height to take, and renders blank.

You must also install a canvas renderer at boot, exactly as any other canvas
consumer does — `canvas_native::register` (Canvas2D / CoreGraphics /
android.graphics) or `canvas_vello::register` (wgpu). A chart is a canvas
*author*; it installs nothing of its own.

---

## Why labels are not drawn into the canvas

Text rasterized into a canvas has to be shaped by whoever draws it, and would
ignore the app's fonts, its theme colors, and platform accessibility text
scaling. So `charts-core` emits label **placements**, and `charts`
materializes them as `text` primitives inside flex-laid-out gutters. The
framework's own layout sizes the gutter to the widest label, which is why this
crate never implements `LabelMetrics` at all.

---

## The three components

| Component | Spec | For |
| --- | --- | --- |
| `Chart` | `ChartSpec` | Cartesian: line, area, bar, scatter, heatmap |
| `PieChart` | `PieSpec` | Pie and donut |
| `RadialChart` | `RadialSpec` | Radial bars and gauges |

They are separate because the specs are genuinely different types.
`ChartSpec` is cartesian to the bone — two axes, a domain each, tick selection,
gridlines, gutters, bar-slot math, column hit-testing. A pie inherits none of
it, and folding polar in as a `Coord::Polar` variant would leave half the
struct meaningless in half its states.

What the families share is everything *below* the spec: the mark IR, the hit
index, the label protocol — and therefore every renderer and every host.

---

## Cartesian: `ChartSpec`

### Series kinds

```rust
SeriesKind::line()                    // straight segments
SeriesKind::smooth_line()             // monotone cubic — never overshoots
SeriesKind::step_line(StepAt::After)  // holds each value to the next sample
SeriesKind::area()
SeriesKind::bar()
SeriesKind::scatter()
SeriesKind::heatmap(ramp)
```

Interpolation is an enum, not a `smooth: bool`:

```rust
LineStyle::new(2.0).interpolate(Interpolation::Step(StepAt::After))
```

`StepAt::After` holds a reading until the next sample's x (the one a
state-over-time series wants — a straight segment between two readings of a
discrete state asserts a transition that never happened). `Before` jumps at the
leading sample; `Mid` changes halfway.

### Axes

```rust
Axis::linear().include_zero(true).title("USD")
Axis::category(["Jan", "Feb", "Mar"])   // x is the index
Axis::time()                            // x is ms since the epoch
Axis::log()                             // non-positive data is dropped
```

`Domain::Auto` fits the data, rounded outward to tick boundaries.
`Domain::Fixed { min, max }` is what a pan/zoom addon writes — `Domain::translate`
and `Domain::zoom` produce one, and `ChartOutput::x.domain()` reads the current
window back.

### Annotations — reference lines and bands

```rust
spec.annotate(Annotation::y_line(300.0, RED).dashed([6.0, 4.0]).label("SLO"))
    .annotate(Annotation::y_band(0.0, 200.0, GREEN_WASH).label("healthy"))
    .annotate(Annotation::x_line(release_ms, PINK).label("deploy"))
```

Positioned in **data** coordinates, so they stay on their value through a
resize, a pan, a zoom and a transition. They deliberately do **not** extend the
domain — a target at 500 on a chart whose data tops out at 90 would flatten
every real value into the floor. A marker outside the visible window emits
nothing.

Rules paint above the series, bands behind them; `.above(bool)` flips it.

### Heatmaps

Each series is one **row**: the series name is the row label, `Datum::x` is the
column, `Datum::y` is the row index, and `Datum::w` carries the value the ramp
is sampled with. Use the `cell(column, row, value)` constructor.

```rust
let ramp = ColorRamp::two(COOL, HOT);
Series::new(
    "monday",
    SeriesKind::Heatmap(HeatmapStyle::new(ramp).domain(0.0, 100.0)),
    BLUE,                                   // unused; the ramp supplies color
    (0..24).map(|c| cell(c as f64, 0.0, load[c])).collect(),
)
```

Both axes want to be `Axis::category` — its half-slot padding is what makes the
first and last cells whole instead of sliced by the plot edge. **Row 0 is at
the bottom**, because the y axis points up here exactly as it does for every
other kind; a top-down heatmap is one `.rev()` on the category list.

Pin `HeatmapStyle::domain` whenever two heatmaps are shown together — auto-fitting
each independently makes the same color mean a different number in each.

Heatmap rows are excluded from the legend: a row has a ramp, not a color.

### Sparkline mode

```rust
ChartSpec::new(series).sparkline()   // no grid, no tick labels, no titles, no legend
```

Grid and labels also toggle independently (`Axis::grid(false)`,
`Axis::labels(false)`, or `Axis::bare()`). Turning labels off gives the gutter
space back, which is the point — "no furniture" that still cost what the
furniture cost would be useless.

---

## Polar: `PieSpec` and `RadialSpec`

Angles are **degrees, clockwise, zero at twelve o'clock** in every
author-facing field.

```rust
PieSpec::donut(slices, 0.62)      // 0.0 is a pie; 0.62 is a donut
    .pad_angle(2.0)
    .labels(PieLabels::Leader)    // None | Inside | Outside | Leader
    .center("1,284").center_sub("sessions")
    .legend(true)
```

Non-positive slice values draw nothing and do not count toward the total — a
pie asserts that its parts sum to a whole, and a negative part has no share of
one. Hidden slices keep their legend entry (that is what a toggle needs) but
leave the denominator.

`pad_angle` **insets** each slice rather than moving it, so the boundary
between two shares stays where the data puts it; a slice thinner than the
padding keeps its full sweep rather than being inset out of existence.

```rust
RadialSpec::gauge("throughput", 72.0, 100.0, MINT)   // 270° opening at the bottom
    .center("72%").center_sub("of capacity")

RadialSpec::new(bars).thickness(18.0).gap(8.0).labels(true)  // outermost first
```

`RadialSpec::min`/`max` is explicit, never inferred: rings only mean anything
if they share a scale, and an auto-fitted max would make the largest value a
full circle on every chart. Values beyond the range clamp rather than wrap
(an arc past a full turn overwrites itself, so an over-max value would read as
a *smaller* one).

A radial ring is hit over its whole **track**, not just its filled arc — the
question a pointer over a ring asks is "what is this ring's value", and it asks
it just as much from the empty part.

---

## Interaction

Hover works with no platform branch: `TouchPhase::Hovered` is pointer motion
with no button down (desktop/web), and `Began`/`Moved` give touch backends a
drag-to-scrub crosshair, which is the correct touch idiom rather than a
workaround.

```rust
Chart(
    spec = rx!(spec.get()),
    on_hover = my_callback,          // Rc<dyn Fn(Option<ChartHover>)>
    selected = rx!(selected.get()),  // Vec<DatumRef>
    dim_others = true,
)
```

A cartesian hover resolves a **column** — every series' datum at that x — which
is what a multi-series readout shows. A polar hover resolves one slice, and the
donut hole resolves to nothing rather than latching onto the nearest wedge.

`on_hover` fires on every pointer move, not only when the hovered column
changes, because a surface that follows the cursor needs the finer rate.
Dedupe on `entries` if yours snaps to a mark instead.

### There is no built-in tooltip

The SDK renders **no hover surface**. `on_hover` is the whole mechanism: the
chart draws marks, labels and legend, and the app renders any tooltip itself,
outside the chart's tree.

This is deliberate. A tooltip is composable from the callback plus a surface,
so it belongs in an app or a wrapper. Owning one in the SDK forced three bad
positions at once — the bubble lived inside the plot's `overflow: hidden` and
was clipped by it; it had to either hardcode colors (wrong in half the themes
it lands in) or render unbacked text over the marks; and its placement was a
fixed cursor-follow, when real charts variously snap to the hovered mark,
track x while pinning y, or park in a corner.

What the SDK owes you instead is enough information to place a surface
anywhere. That is `PointerFrame`, on every hover:

```rust
pub struct PointerFrame {
    pub local:  Point,         // pointer in PLOT-local px — the space HitResult uses
    pub window: Point,         // pointer in window px
    pub plot:   ViewportRect,  // the plot box in viewport space
}

frame.to_viewport(local_point)  // plot-local -> viewport space
```

`plot` is the load-bearing one. Adding its origin to any plot-local point puts
that point in the same space `window` is in — which is what makes "sit beside
the hovered bar" expressible from outside the chart. Without it, plot-local
geometry is unplaceable and cursor-following is the only implementable
behaviour.

Placement is then a pure function of the frame and the entries. Split it in
two — pick an **anchor**, then **resolve** that anchor against the bubble's
measured size — so the edge handling is written once for every mode:

```rust
// 1. The anchor: the point the bubble should sit beside, in viewport space.
match mode {
    // `window` is already viewport-space.
    Cursor => (at.window.x, at.window.y),

    // Snap beside the hovered bar, at its vertical MIDDLE. `position` is the
    // bar's outer end, so a bubble placed there rides up and down with the
    // value — this is what `bounds` is for.
    Mark => {
        let local = match hit.bounds {
            MarkBounds::Rect(r) => Point { x: r.x + r.w, y: r.y + r.h / 2.0 },
            _ => hit.position,
        };
        let v = at.to_viewport(local);
        (v.x, v.y)
    }

    // Track x, pin y to the plot's top edge. Mixing the two spaces is why
    // the frame carries both.
    TrackX => (at.window.x, at.plot.y + 24.0),
}

// 2. Resolve: keep it on screen. x FLIPS to the other side of the anchor
//    (sliding it left instead would park it on the mark it describes);
//    y CLAMPS (there is no side that reads better for a centred bubble).
fn resolve((ax, ay): (f32, f32), (bw, bh): (f32, f32), (vw, vh): (f32, f32)) -> (f32, f32) {
    let right = ax + GAP;
    let x = if right + bw + MARGIN > vw && ax - GAP - bw >= MARGIN {
        ax - GAP - bw
    } else {
        right.min((vw - bw - MARGIN).max(MARGIN))
    };
    let y = (ay - bh / 2.0).clamp(MARGIN, (vh - bh - MARGIN).max(MARGIN));
    (x, y)
}
```

The bubble's size comes from measuring it — `bind` a `Ref<ViewHandle>` and
subscribe with `on_layout` (deferred one frame with `after_animation_frame`,
since the handle is not filled until mount). The viewport comes from
`runtime_core::viewport_size()`, which is reactive, so the placement
re-resolves on window resize for free. Before the first measurement the size
is `(0, 0)` and `resolve` degrades to "no flip", which is harmless — the
layout callback lands before paint.

Mount the surface at your **app root**, not inside the chart — a surface
rendered inside the plot is clipped by the plot's own `overflow: hidden`, the
clip that keeps marks off the axis gutters. The framework has no
`Position::Fixed`, so an absolutely-positioned box in a non-scrolling root
works directly; a scrolling root subtracts its scroll offset, or portals the
bubble through `anchored_overlay`.

### Two traps when you build the surface

Both of these produce symptoms that look like chart bugs. Neither is.

**1. `when` dedups on its predicate's boolean.** Building the whole bubble
inside `when(|| hover.get().is_some())` mounts it once and then never updates
it: the predicate stays `true` as you scrub, so the branch closure does not
re-run and the bubble keeps whichever column you hovered first. It appears to
work if you test by leaving the chart between hovers, because that flips the
predicate and forces a rebuild. Split the work by how often each part changes:

```rust
when(
    move || tip.get().is_some(),        // EXISTENCE only
    move || {
        let rows = switch(               // CONTENT — per column, not per pixel
            move || tip.get().map(|(lines, _, _)| lines).unwrap_or_default(),
            move |lines: &Vec<String>| { /* text nodes */ },
        );
        view(vec![rows]).with_style(move || {   // POSITION — per move, no rebuild
            let (x, y) = tip.get().map(|(_, x, y)| (x, y)).unwrap_or((0.0, 0.0));
            StyleApplication::new(bubble_sheet(x, y))
        })
    },
    || /* closed branch — see trap 2 */,
)
```

**2. Both `when` branches must be out of flow.** The two branches occupy the
same child slot in the parent. If the open branch is `Position::Absolute` and
the closed one is a plain `view`, then in a flex parent with a `gap` the empty
view contributes a gap slot and the absolute bubble does not — so the whole
layout shifts by one gap every time the pointer enters or leaves the chart.
Give the closed branch `position: absolute` too.

`examples/charts-demo` implements all three placements on one callback, with
both traps handled, and is the reference.

### The hit index stores shapes, not points

`HitIndex` indexes what was actually drawn: a bar as its whole body, a pie
slice as a wedge, a heatmap cell as its rect, a marker as a point. A
nearest-point index silently disagrees with what the user can see — a bar
indexed by its top-centre stops responding near its base, and a slice indexed
by its centroid resolves to a neighbour over most of its own area.

```rust
out.hit.contains(p)            // what covers this point (marks with area)
out.hit.nearest_within(p, 12)  // proximity, for markers too small to aim at
out.hit.pick(p, 12)            // containment first, then proximity
out.hit.column_at(p)           // every series at that data x
```

Each `HitResult` carries both an anchor and the geometry:

```rust
hit.position  // Point — one anchor per mark type: a marker's centre, a bar's
              // OUTER END, a wedge's centroid. What a callout points AT.
hit.bounds    // MarkBounds — the mark as drawn: Point | Rect | Wedge.
              // What you place a surface BESIDE.
```

Both are plot-local; run them through `PointerFrame::to_viewport` to place
against them from outside the chart.

### Conditional formatting

```rust
// Hoist the Rc ONCE — identity is what makes two specs compare equal.
let f: StyleFn = Rc::new(|ctx: &MarkContext| {
    if ctx.datum.y > 100.0 { MarkOverride::color(RED) } else { MarkOverride::default() }
});
series.style_fn = Some(f.clone());
```

`ChartSpec` is `PartialEq` so a host can memoise on it, and closures have no
structural equality — so callbacks compare by `Rc::ptr_eq`. Building a fresh
closure inside the expression that rebuilds the spec gives every spec a new
pointer, nothing ever compares equal, and the chart re-renders on every
reactive tick. `PieSpec::styled` / `SliceStyleFn` work the same way.

---

## Transitions

Declared with the framework's **own** `Transition`, not a charting dialect —
the same type and the same `Easing` a stylesheet spells for
`background_transition`:

```rust
Chart(
    spec = ...,
    value_transition = Some(Transition::new(420, Easing::EaseInOut)),
    color_transition = Some(Transition::new(180, Easing::EaseOut)),
)
```

`None` (the default) on either channel means **snap**.

Only the *mechanism* differs from a style transition. A style transition
declares intent and the backend's native machinery interpolates (CSS
`transition`, `CATransaction`, `ObjectAnimator`); marks are painted into a
canvas, so there is no styled node to hand that to and the chart drives its own
frame loop. The declaration is identical, and the author never spells the
difference. Note also that a view's *transform* has no chart analogue — a
mark's geometry **is** its data, so what would be a transform animation
elsewhere is already the value channel here.

### Two channels, and why

| channel | covers | typical |
|---|---|---|
| `value_transition` | datum `x` / `y` / heatmap intensity, and the axis domain | 300–500 ms, `EaseInOut` |
| `color_transition` | series colour **and** whatever a `StyleFn` resolves to | 120–200 ms, `EaseOut` |

They are separate because a duration right for one is wrong for the other: 420
ms suits a bar changing height and reads as sluggish on the same bar changing
hue, where there is no distance for the eye to track. One frame loop drives
both — it runs for the longer of the two, and the shorter channel simply
clamps.

The domain rides the **value** clock rather than owning a third: an axis
settling on a different beat from the marks it measures reads as a glitch.
Tick labels switch immediately regardless, so they do not churn through
intermediate numbers.

### A `StyleFn` fades, it does not flip

A callback is a function of the datum, so resolving it once per frame against
the *interpolated* value makes a threshold recolor switch abruptly at whatever
frame the value crosses it — a hard cut in the middle of a smooth transition.
Instead the callback is resolved at **both ends** and the two answers
interpolated, which is exactly what the axis domain already did:

```rust
// y >= 10 turns a bar red. Animating 5 -> 15 now FADES blue -> red across
// `color_transition`, instead of snapping at the frame the tween passes 10.
let threshold: StyleFn = Rc::new(|ctx| if ctx.datum.y >= 10.0 {
    MarkOverride::color(RED)
} else {
    MarkOverride::default()
});
```

### What never animates

**Highlight.** A point becoming selected, or a series being hovered, lands at
once — easing into a state the user just caused feels laggy rather than
smooth. `dim_others` and `hover_color` are part of that and snap with it.

**A shape change.** A series added or removed, a length changed, a kind
swapped — the pair cannot be matched point-for-point, so the render snaps.
Pairing unrelated points animates a bar toward a value it has nothing to do
with, which reads worse than not animating.

`PieChart` and `RadialChart` take both props. A radial transition interpolates
the range too, so a gauge whose max changes does not snap its scale on frame
one while the arc glides.

### For a non-idealyst consumer

`charts-core` applies **no** easing: every `t` it takes is already eased, and
the two channels arrive as `TweenAt { value, color }`. That boundary is what
keeps the crate free of any runtime dependency — `Easing` is a runtime type it
cannot see, and owning a duplicate would fork the vocabulary in two. A host
with no runtime can use `charts_core::ease_in_out` for the old default curve.

---

## Non-idealyst use

`charts-core` is standalone by contract — no runtime crates, no toolkit. It
ships an SVG reference renderer that doubles as the worked example for a new
consumer:

```rust
let out = render_with(&spec, rect, &Gutters::Measured(&ApproxMetrics));
let svg = to_svg(&out, (480.0, 300.0), TEXT);

let polar = render_pie(&pie, rect);
let svg = scene_to_svg(&polar.scene, (360.0, 360.0), TEXT);
```

`Gutters::None` (the idealyst path) treats the rect as the data area and lets
the host place labels. `Gutters::Measured` insets it to fit measured labels,
for hosts that draw their own text.

+++
title = "Charts"
order = 75
tags = ["charts", "chart", "graph", "plot", "line", "bar", "pie", "donut", "gauge", "heatmap", "sparkline", "canvas", "sdk", "dataviz"]
+++

# Charts

Two crates. **`charts-core`** turns a spec plus a rectangle into a flat list of
vector marks, a list of text *placements*, and a hit index — it draws nothing
and knows about no UI toolkit. **`charts`** binds that to idealyst: marks go
onto a `Canvas`, labels become real `text` primitives, and a tooltip rides the
hit index.

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
    tooltip_content = my_renderer,   // Rc<dyn Fn(&ChartHover) -> Element>
    selected = rx!(selected.get()),  // Vec<DatumRef>
    dim_others = true,
)
```

A cartesian hover resolves a **column** — every series' datum at that x — which
is what a multi-series tooltip shows. A polar hover resolves one slice, and the
donut hole resolves to nothing rather than latching onto the nearest wedge.

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

```rust
Chart(spec = ..., transition_ms = 420)
```

Values and the axis domain glide; tick labels switch immediately so they do not
churn through intermediate numbers. A change that alters the chart's **shape**
— a series added or removed, a length changed, a kind swapped — snaps, because
pairing unrelated points animates a bar toward a value it has nothing to do
with.

`PieChart` and `RadialChart` take the same prop. A radial transition
interpolates the range too, so a gauge whose max changes does not snap its
scale on frame one while the arc glides.

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

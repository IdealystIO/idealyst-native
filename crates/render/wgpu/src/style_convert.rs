//! Pre-resolve `StyleRules` into render-friendly values.
//!
//! `StyleRules` is shaped for the framework's needs — `Tokenized<T>`,
//! `Length` enums, `Color` as a string. The renderer wants concrete
//! f32 px sizes and `[f32; 4]` RGBA. We cache that projection on each
//! node so the per-frame walk is cheap (just read fields).

use framework_core::{
    Color, FontFamily, FontStyle, FontWeight, Gradient, GradientKind, Length, RadialExtent,
    StyleRules, TextAlign, Tokenized, Transform,
};

/// Render-time projection of a node's style. Default = "no painted
/// background, no border, fully opaque, no rounding."
#[derive(Clone, Debug)]
pub struct RenderStyle {
    pub background: Option<[f32; 4]>,
    pub color: [f32; 4], // text color; default is black

    /// Per-corner radius in px. `[tl, tr, br, bl]`.
    pub corner_radius: [f32; 4],
    /// Per-side border width in px. `[top, right, bottom, left]`.
    pub border_width: [f32; 4],
    /// Per-side border color. Defaults to transparent if unset.
    pub border_color: [[f32; 4]; 4],

    pub font_size: f32,
    pub opacity: f32,

    /// Resolved drop shadow, if the author set `shadow: ...` on the
    /// node. The renderer emits a shadow rect instance underneath
    /// the node's main rect via the `shadow_blur > 0` path on the
    /// rounded-rect pipeline. `offset` is `(x, y)`; `blur` controls
    /// the falloff width; `color` is the shadow's RGBA in sRGB.
    pub shadow: Option<ResolvedShadow>,

    /// Resolved background gradient (linear or radial). Replaces
    /// the solid `background` fill when present — the rect shader
    /// gates on `gradient_kind` and uses the gradient stops instead.
    /// Capped at four stops; stylesheets with more truncate at
    /// resolve time (sufficient for the welcome page's
    /// 4-stop sun-glare; widen if a future design needs more).
    pub gradient: Option<ResolvedGradient>,

    /// Static transform from the stylesheet's `transform: [...]`
    /// list. Resolved into a single (translate, scale) pair —
    /// `TranslateX` / `TranslateY` accumulate in `translate`,
    /// `Scale` / `ScaleXY` multiply into `scale`. Pixel units;
    /// `Length::Percent` values are encoded as `(percent / 100,
    /// is_percent=true)` so the renderer can resolve against the
    /// box's actual size at stage time. Rotation, skew, and other
    /// transform list entries are dropped — adding them is a
    /// straightforward extension when an example needs them
    /// (welcome only uses percent translates).
    ///
    /// Composes multiplicatively with the framework's animation
    /// system: the renderer reads `static_translate` + `static_scale`
    /// from here AND `AnimatedOverrides.translate_* / scale_*` per
    /// frame, then combines (static first, then animation on top —
    /// matching iOS's `CGAffineTransform` baking order).
    pub static_translate: [TransformLength; 2],
    pub static_scale: [f32; 2],

    /// Resolved font family name. `None` keeps the renderer's
    /// fallback (cosmic-text's `Family::SansSerif`). `Some(name)`
    /// is passed to `Attrs::family(Family::Name(...))`. For
    /// [`FontFamily::Typeface`] this is the typeface's
    /// `family_name`; the framework registers the font bytes via
    /// `register_asset` before the first `apply_style` references
    /// the family, so cosmic-text resolves it by name.
    pub font_family: Option<String>,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub text_align: TextAlign,
}

/// One axis of a stylesheet-resolved static translate. Stored as
/// the raw value + an `is_percent` flag so the renderer can
/// resolve percents against the box's actual pixel size at stage
/// time (the size isn't known when `apply_style` runs — Taffy
/// hasn't laid out yet).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct TransformLength {
    pub value: f32,
    pub is_percent: bool,
}

/// Backend-resolved counterpart of [`framework_core::Gradient`].
///
/// The shader supports up to five stops; entries past
/// `stop_count` carry the last real stop's color repeated so the
/// interpolation degenerates safely without a per-fragment branch.
/// Five stops is exactly what the welcome's sun-glare needs (core
/// → warm corona → halo → faint halo → transparent edge); fewer
/// would force truncation of the transparent fade-out and leave a
/// visible warm ring at the disc's edge.
///
/// The cap also lands at WebGPU's `maxVertexAttributes = 16`
/// minimum — extending past 5 stops would require switching the
/// gradient data to a storage buffer (vertex attribute count is
/// already saturated by the existing rect / border / 5-stop
/// fields).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ResolvedGradient {
    pub kind: ResolvedGradientKind,
    /// Stop colors as sRGB `[r, g, b, a]`. Slots `stop_count..5` hold
    /// the last real stop's color (or transparent if there are zero
    /// stops, which the resolver filters out).
    pub stops: [[f32; 4]; 5],
    /// Stop offsets in `0..=1`. Slots `stop_count..5` hold `1.0` so
    /// the shader's bracket search lands on `stop[stop_count - 1]`
    /// for every `t` past the last real stop.
    pub stop_offsets: [f32; 5],
    /// Number of real stops the author declared. `1..=5`.
    pub stop_count: u8,
}

/// Render-side projection of `GradientKind`. Radial bakes in the
/// resolved `(center, radius)` so the shader doesn't need access
/// to `RadialExtent` or the multiplier; the resolver does that math
/// in rect-fraction space (0..1 across the box).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ResolvedGradientKind {
    /// Direction of the gradient axis (unit vector pointing the way
    /// stops INCREASE), in rect-fraction space. For CSS-style
    /// `angle_deg`: `(sin θ, -cos θ)` so `0deg = bottom→top`,
    /// `90deg = left→right`.
    Linear { direction: [f32; 2] },
    /// Center in rect-fraction (0..=1), elliptical radii in the
    /// same space. The stop at offset 1.0 sits on the ellipse
    /// `(p.x-cx)²/rx² + (p.y-cy)²/ry² = 1`. For `ClosestSide`
    /// the radii are `(min(cx, 1-cx), min(cy, 1-cy)) * multiplier`;
    /// `FarthestCorner` picks the max corner distance.
    Radial {
        center: [f32; 2],
        radii: [f32; 2],
    },
}

/// Backend-resolved counterpart of `framework_core::Shadow` —
/// hex strings parsed to RGBA, no `Tokenized` indirection so the
/// renderer can read it on the hot path.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ResolvedShadow {
    pub offset: [f32; 2],
    pub blur: f32,
    pub color: [f32; 4],
}

impl Default for RenderStyle {
    fn default() -> Self {
        Self {
            background: None,
            color: [0.0, 0.0, 0.0, 1.0],
            corner_radius: [0.0; 4],
            border_width: [0.0; 4],
            border_color: [[0.0, 0.0, 0.0, 0.0]; 4],
            font_size: 14.0,
            opacity: 1.0,
            shadow: None,
            gradient: None,
            static_translate: [TransformLength::default(); 2],
            static_scale: [1.0, 1.0],
            font_family: None,
            font_weight: FontWeight::Normal,
            font_style: FontStyle::Normal,
            text_align: TextAlign::Left,
        }
    }
}

impl RenderStyle {
    /// Project from the framework's `StyleRules`. Properties that
    /// the rules leave unset keep their previous render value — call
    /// sites should start from the existing `RenderStyle`, not from
    /// `default()`, so a state overlay setting only `background`
    /// preserves the base's borders and font size.
    pub fn apply(&mut self, rules: &StyleRules) {
        // `.resolve()` subscribes the enclosing apply-style Effect to
        // the per-token signal for each referenced token. Token swaps
        // re-fire only nodes that touched the changed token.
        if let Some(bg) = rules.background.as_ref() {
            self.background = Some(parse_color(&bg.resolve()));
        }
        if let Some(c) = rules.color.as_ref() {
            self.color = parse_color(&c.resolve());
        }
        if let Some(fs) = rules.font_size.as_ref() {
            if let Length::Px(px) = fs.resolve() {
                self.font_size = px;
            }
        }
        // Font family / weight / style — passed through to the text
        // shaper. `FontFamily::Typeface` resolves to the typeface's
        // family_name (the framework has called `register_asset` for
        // each face's bytes by the time this style applies, so
        // cosmic-text can resolve the name).
        if let Some(fam) = rules.font_family.as_ref() {
            self.font_family = Some(match fam {
                FontFamily::System(name) => name.clone(),
                FontFamily::Typeface(t) => t.family_name.to_string(),
            });
        }
        if let Some(w) = rules.font_weight.as_ref() {
            self.font_weight = *w;
        }
        if let Some(s) = rules.font_style.as_ref() {
            self.font_style = *s;
        }
        if let Some(a) = rules.text_align.as_ref() {
            self.text_align = *a;
        }
        if let Some(o) = rules.opacity.as_ref() {
            self.opacity = o.resolve();
        }

        // Border radius: per-corner. Percent is interpreted at draw
        // time against the rect's min(width, height) — but the MVP
        // shader only handles px, so we collapse percent to 0 for
        // now and revisit when we add percent support.
        self.corner_radius[0] = px(rules.border_top_left_radius.as_ref());
        self.corner_radius[1] = px(rules.border_top_right_radius.as_ref());
        self.corner_radius[2] = px(rules.border_bottom_right_radius.as_ref());
        self.corner_radius[3] = px(rules.border_bottom_left_radius.as_ref());

        // Border widths.
        self.border_width[0] = rules.border_top_width.as_ref().map(|t| t.resolve()).unwrap_or(self.border_width[0]);
        self.border_width[1] = rules.border_right_width.as_ref().map(|t| t.resolve()).unwrap_or(self.border_width[1]);
        self.border_width[2] = rules.border_bottom_width.as_ref().map(|t| t.resolve()).unwrap_or(self.border_width[2]);
        self.border_width[3] = rules.border_left_width.as_ref().map(|t| t.resolve()).unwrap_or(self.border_width[3]);

        if let Some(c) = rules.border_top_color.as_ref() {
            self.border_color[0] = parse_color(&c.resolve());
        }
        if let Some(c) = rules.border_right_color.as_ref() {
            self.border_color[1] = parse_color(&c.resolve());
        }
        if let Some(c) = rules.border_bottom_color.as_ref() {
            self.border_color[2] = parse_color(&c.resolve());
        }
        if let Some(c) = rules.border_left_color.as_ref() {
            self.border_color[3] = parse_color(&c.resolve());
        }

        // Drop shadow — author sets `Shadow { x, y, blur, color }`
        // on the rules; we project to RGBA + concrete f32s so the
        // renderer can stage a shadow rect instance without
        // touching the framework's `Tokenized` types on the hot
        // path. Absence collapses to `None`; once set, fields
        // without an explicit per-frame update keep their resolved
        // values (same merge-into-self pattern the rest of this
        // function uses).
        if let Some(sh) = rules.shadow.as_ref() {
            self.shadow = Some(ResolvedShadow {
                offset: [sh.x, sh.y],
                blur: sh.blur,
                color: parse_color(&sh.color),
            });
        }

        // Background gradient. Resolved into rect-fraction-space so
        // the shader's per-fragment math doesn't need the pixel
        // size — clamp / project happens in 0..1 across the box.
        if let Some(g) = rules.background_gradient.as_ref() {
            self.gradient = resolve_gradient(g);
        }

        // Static transform — `TranslateX(50%)` etc. from the
        // stylesheet. Percent values are stored with an `is_percent`
        // flag and resolved against the box's pixel size at stage
        // time (the box hasn't been laid out yet here). Translates
        // accumulate additively; `Scale` / `ScaleXY` multiply into
        // the per-axis scale. Rotate / Skew are dropped — adding
        // those is a small extension when a real design needs them.
        if let Some(transforms) = rules.transform.as_ref() {
            // Reset to identity each apply so a re-styled node
            // doesn't carry a stale transform from its previous
            // resolution.
            self.static_translate = [TransformLength::default(); 2];
            self.static_scale = [1.0, 1.0];
            for t in transforms {
                match t {
                    Transform::TranslateX(len) => {
                        let entry = match *len {
                            Length::Px(v) => TransformLength { value: v, is_percent: false },
                            Length::Percent(v) => {
                                TransformLength { value: v / 100.0, is_percent: true }
                            }
                            // `Length::Auto` has no meaning as a
                            // transform offset; treat as zero.
                            Length::Auto => TransformLength::default(),
                        };
                        // Sum existing + new on the same axis.
                        if entry.is_percent == self.static_translate[0].is_percent {
                            self.static_translate[0].value += entry.value;
                        } else {
                            // Mixed px + percent on the same axis is
                            // unusual; the welcome's transforms are
                            // single-entry so this branch never trips.
                            // Fall back to ignoring the new entry's
                            // unit when it diverges — keeping the
                            // resolver simple beats getting clever.
                            self.static_translate[0] = entry;
                        }
                    }
                    Transform::TranslateY(len) => {
                        let entry = match *len {
                            Length::Px(v) => TransformLength { value: v, is_percent: false },
                            Length::Percent(v) => {
                                TransformLength { value: v / 100.0, is_percent: true }
                            }
                            // `Length::Auto` has no meaning as a
                            // transform offset; treat as zero.
                            Length::Auto => TransformLength::default(),
                        };
                        if entry.is_percent == self.static_translate[1].is_percent {
                            self.static_translate[1].value += entry.value;
                        } else {
                            self.static_translate[1] = entry;
                        }
                    }
                    Transform::Scale(v) => {
                        self.static_scale[0] *= v;
                        self.static_scale[1] *= v;
                    }
                    Transform::ScaleXY { x, y } => {
                        self.static_scale[0] *= x;
                        self.static_scale[1] *= y;
                    }
                    // Rotation + skew not supported by the wgpu
                    // backend's static path yet — author code can
                    // still drive rotation through the animation
                    // system (`AnimProp::RotateZ`), which IS wired.
                    Transform::Rotate(_)
                    | Transform::SkewX(_)
                    | Transform::SkewY(_) => {}
                }
            }
        }
    }
}

/// Project a framework `Gradient` into the renderer's
/// [`ResolvedGradient`]. Returns `None` for degenerate inputs
/// (zero stops) — the caller should treat that as "no gradient,
/// fall back to the solid `background` color."
fn resolve_gradient(g: &Gradient) -> Option<ResolvedGradient> {
    if g.stops.is_empty() {
        return None;
    }
    // Cap at 5 stops. Truncation is silent — the shader can't bind
    // more stop slots than its layout declares. Most natural
    // gradients (linear vignettes, radial bloom) use 2-5 stops;
    // five covers the welcome's sun-glare exactly. Overflow past
    // five is an authoring concern, and the shader's vertex
    // attribute count is at WebGPU's portable minimum here.
    let n = g.stops.len().min(5);
    let mut stops = [[0.0_f32; 4]; 5];
    let mut offsets = [1.0_f32; 5];
    for (i, s) in g.stops.iter().take(n).enumerate() {
        stops[i] = parse_color(&s.color);
        offsets[i] = s.offset.clamp(0.0, 1.0);
    }
    // Pad trailing slots with the last real color. The shader uses
    // `t > offsets[i]` ladders to pick the bracket — when `t` lands
    // past the last real stop, both endpoints of the mix are the
    // same color and the result is constant. Skips a per-fragment
    // `count` branch.
    if n < 5 {
        let last = stops[n - 1];
        for slot in stops.iter_mut().skip(n) {
            *slot = last;
        }
    }
    let kind = match &g.kind {
        GradientKind::Linear { angle_deg } => {
            // CSS convention: 0deg = bottom→top. Direction vector
            // points the way stops INCREASE.
            let theta = angle_deg.to_radians();
            let dx = theta.sin();
            let dy = -theta.cos();
            ResolvedGradientKind::Linear { direction: [dx, dy] }
        }
        GradientKind::Radial { center, radius, extent } => {
            let (cx, cy) = *center;
            // Reference distance in rect-fraction space. For
            // `ClosestSide` the reference is the closest edge
            // midpoint distance from `center`; for `FarthestCorner`
            // it's the farthest corner. Both are computed independent
            // of the rect's pixel aspect, so the gradient ends up
            // elliptical on non-square boxes — matches CSS's default
            // (`radial-gradient` without explicit `ellipse` keyword
            // collapses to circle for square boxes; the welcome's
            // sun-glare wrapper carries `aspect_ratio: 1.0` so the
            // distinction doesn't matter visually there).
            let (rx_base, ry_base) = match extent {
                RadialExtent::ClosestSide => (
                    cx.min(1.0 - cx),
                    cy.min(1.0 - cy),
                ),
                RadialExtent::FarthestCorner => {
                    let dx = cx.max(1.0 - cx);
                    let dy = cy.max(1.0 - cy);
                    // Farthest corner: pythagorean distance from
                    // center to the farthest corner of the unit box,
                    // applied to both axes (gives a circle in rect-
                    // fraction space).
                    let d = (dx * dx + dy * dy).sqrt();
                    (d, d)
                }
            };
            ResolvedGradientKind::Radial {
                center: [cx, cy],
                radii: [rx_base * radius, ry_base * radius],
            }
        }
    };
    Some(ResolvedGradient {
        kind,
        stops,
        stop_offsets: offsets,
        stop_count: n as u8,
    })
}

fn px(t: Option<&Tokenized<Length>>) -> f32 {
    match t.map(|x| x.resolve()) {
        Some(Length::Px(v)) => v,
        _ => 0.0,
    }
}

/// Best-effort CSS color parse. Delegates to `framework_core::color`;
/// unknown strings render as opaque magenta so missing-color bugs are
/// visible at a glance (vs. silently rendering black like the
/// platform backends, where the surrounding CSS class still
/// produces correct output).
pub fn parse_color(c: &Color) -> [f32; 4] {
    const MAGENTA: [f32; 4] = [1.0, 0.0, 1.0, 1.0];
    framework_core::color::parse(&c.0)
        .map(|c| c.to_srgb_f32())
        .unwrap_or(MAGENTA)
}

// Identity conversion. The wgpu sim swapchain runs in a non-sRGB
// format (see `crates/host/winit/src/gpu.rs`) so the GPU blend
// equation operates on raw sRGB-numbers — same posture as
// CAGradientLayer / CSS / standard UI compositors. We keep the
// `srgb_rgba_to_linear` name (rather than removing 27 call sites)
// because the function still has SEMANTIC meaning: "convert the
// stylesheet's sRGB value into whatever colorspace the swapchain
// expects." When the swapchain is non-sRGB and we want naive
// alpha blending, that conversion is identity. If the renderer is
// ever ported to a target that mandates a linear pipeline, this
// becomes the natural single place to re-enable the gamma decode.
pub use framework_core::color::srgb_channel_to_linear;
pub fn srgb_rgba_to_linear(c: [f32; 4]) -> [f32; 4] {
    c
}

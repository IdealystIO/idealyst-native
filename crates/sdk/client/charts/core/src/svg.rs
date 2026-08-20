//! A reference renderer: [`ChartScene`] to an SVG string.
//!
//! Two jobs, neither of them "be the production renderer". First, it proves
//! the mark IR is genuinely renderer-agnostic rather than quietly shaped
//! around one target — if a mark cannot be expressed in plain SVG, the IR
//! has leaked an assumption. Second, it gives the test suite output a human
//! can open and look at, which pixel goldens do not.
//!
//! It is also the worked example for what a non-idealyst consumer writes:
//! roughly 200 lines, no dependencies, no font handling beyond picking a
//! size per label role.

use std::fmt::Write as _;

use crate::render::ChartOutput;
use crate::scene::{
    Color, FillRule, HAlign, LabelPlacement, LabelRole, Mark, Paint, Path, PathSeg, Stroke, VAlign,
};

/// Font size in pixels for each label role.
fn font_size(role: LabelRole) -> f32 {
    match role {
        LabelRole::Title => 16.0,
        LabelRole::AxisTitleX | LabelRole::AxisTitleY => 12.0,
        LabelRole::Legend => 12.0,
        _ => 11.0,
    }
}

/// A [`LabelMetrics`](crate::render::LabelMetrics) good enough to size
/// gutters without a font stack.
///
/// Approximates advance width as a fixed fraction of the font size. That is
/// wrong for any real proportional font, but it is wrong by a small and
/// *bounded* amount, which is all a gutter needs — and being deterministic,
/// it keeps golden tests stable across machines with different fonts
/// installed. Hosts that care about exact gutters supply their own.
#[derive(Clone, Copy, Debug, Default)]
pub struct ApproxMetrics;

impl crate::render::LabelMetrics for ApproxMetrics {
    fn measure(&self, text: &str, role: LabelRole) -> (f32, f32) {
        let size = font_size(role);
        (text.chars().count() as f32 * size * 0.58, size * 1.25)
    }
}

fn fmt_num(v: f32) -> String {
    // Two decimals is below the visible threshold at any realistic zoom and
    // keeps golden files from churning on float noise in the last bits.
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Opaque hex form. Alpha is NEVER folded in here — see [`paint_attrs`].
fn css_color(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

/// `fill`/`stroke` plus a matching `*-opacity`, as SVG 1.1 attributes.
///
/// Deliberately NOT `rgba(...)`. Functional-notation alpha is CSS Color 4;
/// SVG 1.1's `<color>` grammar has no alpha channel, so `stroke="rgba(...)"`
/// is invalid there and strict renderers drop the whole declaration — the
/// mark then vanishes, or falls back to black, with no error. Browsers
/// accept it, which is exactly what makes the bug easy to ship: it renders
/// fine in the one place you test. `*-opacity` is universally supported and
/// has been since SVG 1.0.
fn paint_attrs(kind: &str, c: Color) -> String {
    let mut a = format!(r#" {kind}="{}""#, css_color(c));
    if c.a != 255 {
        let _ = write!(a, r#" {kind}-opacity="{}""#, fmt_num(c.a as f32 / 255.0));
    }
    a
}

fn path_data(p: &Path) -> String {
    let mut d = String::new();
    for seg in &p.segs {
        match seg {
            PathSeg::MoveTo(a) => {
                let _ = write!(d, "M{} {} ", fmt_num(a.x), fmt_num(a.y));
            }
            PathSeg::LineTo(a) => {
                let _ = write!(d, "L{} {} ", fmt_num(a.x), fmt_num(a.y));
            }
            PathSeg::QuadTo(c, a) => {
                let _ = write!(
                    d,
                    "Q{} {} {} {} ",
                    fmt_num(c.x),
                    fmt_num(c.y),
                    fmt_num(a.x),
                    fmt_num(a.y)
                );
            }
            PathSeg::CubicTo(c1, c2, a) => {
                let _ = write!(
                    d,
                    "C{} {} {} {} {} {} ",
                    fmt_num(c1.x),
                    fmt_num(c1.y),
                    fmt_num(c2.x),
                    fmt_num(c2.y),
                    fmt_num(a.x),
                    fmt_num(a.y)
                );
            }
            PathSeg::Close => d.push_str("Z "),
        }
    }
    d.trim_end().to_string()
}

fn stroke_attrs(s: &Stroke) -> String {
    use crate::scene::{LineCap, LineJoin};
    let cap = match s.cap {
        LineCap::Butt => "butt",
        LineCap::Round => "round",
        LineCap::Square => "square",
    };
    let join = match s.join {
        LineJoin::Miter => "miter",
        LineJoin::Round => "round",
        LineJoin::Bevel => "bevel",
    };
    let mut a = format!(
        r#" stroke-width="{}" stroke-linecap="{cap}" stroke-linejoin="{join}""#,
        fmt_num(s.width)
    );
    if !s.dash.is_empty() {
        let d: Vec<String> = s.dash.iter().map(|v| fmt_num(*v)).collect();
        let _ = write!(a, r#" stroke-dasharray="{}""#, d.join(","));
    }
    a
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn label_svg(l: &LabelPlacement, text_color: Color) -> String {
    let anchor = match l.h_align {
        HAlign::Left => "start",
        HAlign::Center => "middle",
        HAlign::Right => "end",
    };
    // `dominant-baseline` is the one place SVG renderers disagree enough to
    // matter, so Baseline is expressed as the absence of the attribute
    // rather than as `auto`.
    let baseline = match l.v_align {
        VAlign::Top => r#" dominant-baseline="hanging""#,
        VAlign::Middle => r#" dominant-baseline="central""#,
        VAlign::Bottom => r#" dominant-baseline="alphabetic""#,
        VAlign::Baseline => "",
    };
    let transform = if l.rotation.abs() > f32::EPSILON {
        format!(
            r#" transform="rotate({} {} {})""#,
            fmt_num(l.rotation),
            fmt_num(l.anchor.x),
            fmt_num(l.anchor.y)
        )
    } else {
        String::new()
    };
    format!(
        r#"<text x="{}" y="{}" text-anchor="{anchor}"{baseline} font-size="{}"{}{transform}>{}</text>"#,
        fmt_num(l.anchor.x),
        fmt_num(l.anchor.y),
        fmt_num(font_size(l.role)),
        paint_attrs("fill", l.color.unwrap_or(text_color)),
        escape(&l.text)
    )
}

/// Render a chart to a standalone SVG document.
///
/// `size` is the full surface; `text_color` is used for every label that
/// does not carry its own (legend entries do).
pub fn to_svg(out: &ChartOutput, size: (f32, f32), text_color: Color) -> String {
    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}" font-family="sans-serif">"#,
        fmt_num(size.0),
        fmt_num(size.1),
        fmt_num(size.0),
        fmt_num(size.1)
    );

    // Gradients must be declared before use, so collect them in one pass.
    let mut defs = String::new();
    for (i, m) in out.scene.marks.iter().enumerate() {
        if let Mark::Fill { paint: Paint::Linear { from, to, stops }, .. } = m {
            let _ = write!(
                defs,
                r#"<linearGradient id="g{i}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}">"#,
                fmt_num(from.x),
                fmt_num(from.y),
                fmt_num(to.x),
                fmt_num(to.y)
            );
            for st in stops {
                let _ = write!(
                    defs,
                    r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
                    fmt_num(st.offset),
                    css_color(st.color),
                    fmt_num(st.color.a as f32 / 255.0)
                );
            }
            defs.push_str("</linearGradient>");
        }
    }
    if !defs.is_empty() {
        let _ = write!(s, "<defs>{defs}</defs>");
    }

    for (i, m) in out.scene.marks.iter().enumerate() {
        match m {
            Mark::Fill { path, paint, rule, .. } => {
                let fill = match paint {
                    Paint::Solid(c) => paint_attrs("fill", *c),
                    Paint::Linear { .. } => format!(r#" fill="url(#g{i})""#),
                };
                let rule = match rule {
                    FillRule::NonZero => "nonzero",
                    FillRule::EvenOdd => "evenodd",
                };
                let _ = write!(
                    s,
                    r#"<path d="{}"{fill} fill-rule="{rule}"/>"#,
                    path_data(path)
                );
            }
            Mark::Stroke { path, stroke, paint, .. } => {
                // A gradient-stroked mark is not something the renderer
                // emits today; approximate with the first stop rather than
                // dropping the mark entirely.
                let stroke_paint = match paint {
                    Paint::Solid(c) => paint_attrs("stroke", *c),
                    Paint::Linear { stops, .. } => stops
                        .first()
                        .map(|st| paint_attrs("stroke", st.color))
                        .unwrap_or_else(|| r#" stroke="none""#.into()),
                };
                let _ = write!(
                    s,
                    r#"<path d="{}" fill="none"{stroke_paint}{}/>"#,
                    path_data(path),
                    stroke_attrs(stroke)
                );
            }
            Mark::Points { instances, .. } => {
                for p in instances {
                    // Circles are the common case and read better in the
                    // output than a rounded rect with equal radii.
                    if (p.radius - p.half.x).abs() < f32::EPSILON
                        && (p.half.x - p.half.y).abs() < f32::EPSILON
                    {
                        let _ = write!(
                            s,
                            r#"<circle cx="{}" cy="{}" r="{}"{}/>"#,
                            fmt_num(p.center.x),
                            fmt_num(p.center.y),
                            fmt_num(p.radius),
                            paint_attrs("fill", p.color)
                        );
                    } else {
                        let _ = write!(
                            s,
                            r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}"{}/>"#,
                            fmt_num(p.center.x - p.half.x),
                            fmt_num(p.center.y - p.half.y),
                            fmt_num(p.half.x * 2.0),
                            fmt_num(p.half.y * 2.0),
                            fmt_num(p.radius),
                            paint_attrs("fill", p.color)
                        );
                    }
                }
            }
        }
    }

    for l in &out.scene.labels {
        s.push_str(&label_svg(l, text_color));
    }
    s.push_str("</svg>");
    s
}

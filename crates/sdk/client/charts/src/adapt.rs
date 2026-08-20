//! `charts_core` mark IR -> `canvas_core::Scene`.
//!
//! Mechanical by construction: the IR was designed against what `Scene`
//! can already express, so every arm here is a direct restatement with no
//! approximation, no flattening, and no information lost. If a future mark
//! cannot be written as a single `DrawOp`, that is a signal the IR has
//! drifted away from the canvas substrate rather than a licence to
//! pre-tessellate it here.

use canvas_core as cv;
use charts_core::scene as ir;

fn color(c: ir::Color) -> cv::Color {
    cv::Color::new(c.r, c.g, c.b, c.a)
}

fn point(p: ir::Point) -> (f32, f32) {
    (p.x, p.y)
}

fn path(p: &ir::Path) -> cv::Path {
    let mut out = cv::Path::new();
    for seg in &p.segs {
        out = match seg {
            ir::PathSeg::MoveTo(a) => {
                let (x, y) = point(*a);
                out.move_to(x, y)
            }
            ir::PathSeg::LineTo(a) => {
                let (x, y) = point(*a);
                out.line_to(x, y)
            }
            ir::PathSeg::QuadTo(c, a) => {
                let (cx, cy) = point(*c);
                let (x, y) = point(*a);
                out.quad_to(cx, cy, x, y)
            }
            ir::PathSeg::CubicTo(c1, c2, a) => {
                let (c1x, c1y) = point(*c1);
                let (c2x, c2y) = point(*c2);
                let (x, y) = point(*a);
                out.cubic_to(c1x, c1y, c2x, c2y, x, y)
            }
            ir::PathSeg::Close => out.close(),
        };
    }
    out
}

fn paint(p: &ir::Paint) -> cv::Paint {
    match p {
        ir::Paint::Solid(c) => cv::Paint::solid(color(*c)),
        ir::Paint::Linear { from, to, stops } => cv::Paint::linear(
            from.x,
            from.y,
            to.x,
            to.y,
            stops
                .iter()
                .map(|s| cv::GradientStop::new(s.offset, color(s.color)))
                .collect(),
        ),
    }
}

fn fill_rule(r: ir::FillRule) -> cv::FillRule {
    match r {
        ir::FillRule::NonZero => cv::FillRule::NonZero,
        ir::FillRule::EvenOdd => cv::FillRule::EvenOdd,
    }
}

fn stroke(s: &ir::Stroke) -> cv::Stroke {
    let cap = match s.cap {
        ir::LineCap::Butt => cv::LineCap::Butt,
        ir::LineCap::Round => cv::LineCap::Round,
        ir::LineCap::Square => cv::LineCap::Square,
    };
    let join = match s.join {
        ir::LineJoin::Miter => cv::LineJoin::Miter,
        ir::LineJoin::Round => cv::LineJoin::Round,
        ir::LineJoin::Bevel => cv::LineJoin::Bevel,
    };
    cv::Stroke::width(s.width)
        .cap(cap)
        .join(join)
        .dash(s.dash.clone(), s.dash_offset)
}

/// Append a rendered chart's marks to `scene`, offset by `(dx, dy)`.
///
/// The offset exists because `charts-core` renders into a rect whose
/// origin the caller chooses, while the canvas is its own coordinate
/// space starting at zero. Keeping it a parameter rather than assuming
/// zero lets a host place several charts on one canvas.
pub fn marks_into_scene(marks: &[ir::Mark], scene: &mut cv::Scene, dx: f32, dy: f32) {
    let shift = |p: ir::Point| ir::Point { x: p.x + dx, y: p.y + dy };
    let shift_path = |p: &ir::Path| ir::Path {
        segs: p
            .segs
            .iter()
            .map(|s| match s {
                ir::PathSeg::MoveTo(a) => ir::PathSeg::MoveTo(shift(*a)),
                ir::PathSeg::LineTo(a) => ir::PathSeg::LineTo(shift(*a)),
                ir::PathSeg::QuadTo(c, a) => ir::PathSeg::QuadTo(shift(*c), shift(*a)),
                ir::PathSeg::CubicTo(c1, c2, a) => {
                    ir::PathSeg::CubicTo(shift(*c1), shift(*c2), shift(*a))
                }
                ir::PathSeg::Close => ir::PathSeg::Close,
            })
            .collect(),
    };

    for m in marks {
        match m {
            ir::Mark::Fill { path: p, paint: pt, rule, .. } => {
                let pt = match pt {
                    // A gradient's endpoints are in the same space as the
                    // geometry, so they shift with it.
                    ir::Paint::Linear { from, to, stops } => ir::Paint::Linear {
                        from: shift(*from),
                        to: shift(*to),
                        stops: stops.clone(),
                    },
                    other => other.clone(),
                };
                scene.push_op(cv::DrawOp::Fill {
                    path: path(&shift_path(p)),
                    paint: paint(&pt),
                    fill_rule: fill_rule(*rule),
                });
            }
            ir::Mark::Stroke { path: p, stroke: s, paint: pt, .. } => {
                scene.push_op(cv::DrawOp::Stroke {
                    path: path(&shift_path(p)),
                    paint: paint(pt),
                    stroke: stroke(s),
                });
            }
            ir::Mark::Points { instances, .. } => {
                // Straight to the instanced batch rather than one fill per
                // point — this is the path a scatter of tens of thousands
                // of points depends on, and the GPU renderer draws the
                // whole batch in a single pass.
                scene.push_op(cv::DrawOp::Shapes {
                    shapes: instances
                        .iter()
                        .map(|p| {
                            cv::ShapeInstance::new(
                                p.center.x + dx,
                                p.center.y + dy,
                                p.half.x,
                                p.half.y,
                                p.radius,
                                color(p.color),
                            )
                        })
                        .collect(),
                    blend: cv::BlendMode::Normal,
                });
            }
        }
    }
}

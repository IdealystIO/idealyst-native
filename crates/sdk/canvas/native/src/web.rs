//! Web (`target_arch = "wasm32"`) renderer for the canvas SDK.
//!
//! Creates a `<canvas>` per mount and replays the author's [`Scene`]
//! into its `2d` context. Two triggers drive a repaint:
//!
//! - A reactive [`Effect`] re-runs the painter whenever a `Signal` it
//!   reads changes (same reactive-source convention as `video`/`svg`).
//! - A `ResizeObserver` re-renders (and resizes the backing store to the
//!   CSS box × `devicePixelRatio`) when layout changes the canvas size.
//!
//! Both render from one shared latest-[`Scene`] cell, so a resize never
//! re-runs author code and a content change never needs the element's
//! size to have changed.
//!
//! [`Scene`]: canvas_core::Scene

use backend_web::WebBackend;
use canvas_core::{
    paint_scene, CanvasProps, Color, DrawOp, FillRule, LineCap, LineJoin, LinearGradient, Paint,
    PaintKind, Path, PathSeg, RadialGradient, Scene,
};
use runtime_core::Effect;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{
    CanvasGradient, CanvasRenderingContext2d, CanvasWindingRule, HtmlCanvasElement, ResizeObserver,
};

/// Register the native canvas renderer against a `WebBackend`. One line
/// from the app's bootstrap; backs every `canvas::Canvas` with a
/// `<canvas>` 2D context.
pub fn register(backend: &mut WebBackend) {
    canvas_core::ensure_wire_serde();
    backend.register_external::<CanvasProps, _>(|props, _backend| build_canvas(props));
}

/// Disconnects the `ResizeObserver` and frees its `Closure` on scope
/// teardown, so a callback the browser has already queued can't fire
/// into freed wasm state after unmount (the classic web-listener UAF).
struct ObserverGuard {
    observer: ResizeObserver,
    _cb: Closure<dyn FnMut()>,
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

fn build_canvas(props: &Rc<CanvasProps>) -> web_sys::Element {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let el = document
        .create_element("canvas")
        .expect("create_element(canvas) failed");
    let _ = el.set_attribute("data-external-kind", "canvas_core::CanvasProps");

    let canvas: HtmlCanvasElement = el.clone().dyn_into().expect("canvas element cast");
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .expect("2d context unavailable")
        .dyn_into()
        .expect("2d context cast");

    // Latest painted scene — written by the content effect, read by both
    // the effect's own render and the resize observer.
    let cell: Rc<RefCell<Scene>> = Rc::new(RefCell::new(Scene::new()));

    // Size the backing store to the CSS box × dpr, then replay the cell.
    let render: Rc<dyn Fn()> = {
        let canvas = canvas.clone();
        let ctx = ctx.clone();
        let cell = cell.clone();
        Rc::new(move || render_scene(&canvas, &ctx, &cell.borrow()))
    };

    let cb = Closure::<dyn FnMut()>::new({
        let render = render.clone();
        move || render()
    });
    let observer = ResizeObserver::new(cb.as_ref().unchecked_ref()).expect("ResizeObserver::new");
    observer.observe(&el);
    let guard = ObserverGuard { observer, _cb: cb };

    // Reactive repaint. The walker runs us inside the mount scope, so this
    // Effect (and the `guard` + `render` it owns) live until unmount.
    let props = props.clone();
    let _effect = Effect::new(move || {
        // Capture the observer guard into the scope-owned effect so it is
        // dropped (→ disconnected) exactly when the canvas unmounts.
        let _keep = &guard;
        *cell.borrow_mut() = paint_scene(&props);
        render();
    });

    el
}

/// Resize the backing store and replay `scene` into `ctx`.
fn render_scene(canvas: &HtmlCanvasElement, ctx: &CanvasRenderingContext2d, scene: &Scene) {
    let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
    let css_w = canvas.client_width() as f64;
    let css_h = canvas.client_height() as f64;
    // Not laid out yet (handler runs before insertion). The ResizeObserver
    // fires once the element has a real box and we render then.
    if css_w <= 0.0 || css_h <= 0.0 {
        return;
    }
    let bw = (css_w * dpr).round().max(1.0) as u32;
    let bh = (css_h * dpr).round().max(1.0) as u32;
    // Setting width/height resets the context; only do it on a real change.
    if canvas.width() != bw {
        canvas.set_width(bw);
    }
    if canvas.height() != bh {
        canvas.set_height(bh);
    }

    let _ = ctx.reset_transform();
    ctx.clear_rect(0.0, 0.0, bw as f64, bh as f64);
    // Author coordinates are logical pixels; map them to device pixels.
    let _ = ctx.scale(dpr, dpr);
    // Protect the dpr base transform from an unbalanced author `restore`.
    ctx.save();
    for op in scene.ops() {
        apply_op(ctx, op);
    }
    ctx.restore();
}

fn apply_op(ctx: &CanvasRenderingContext2d, op: &DrawOp) {
    match op {
        DrawOp::Save => ctx.save(),
        DrawOp::Restore => ctx.restore(),
        DrawOp::Transform(t) => {
            let _ = ctx.transform(
                t.a as f64,
                t.b as f64,
                t.c as f64,
                t.d as f64,
                t.e as f64,
                t.f as f64,
            );
        }
        DrawOp::Fill { path, paint, fill_rule } => {
            build_path(ctx, path);
            apply_fill_paint(ctx, paint);
            match fill_rule {
                FillRule::NonZero => ctx.fill(),
                FillRule::EvenOdd => ctx.fill_with_canvas_winding_rule(CanvasWindingRule::Evenodd),
            }
        }
        DrawOp::Stroke { path, paint, stroke } => {
            build_path(ctx, path);
            apply_stroke_paint(ctx, paint);
            ctx.set_line_width(stroke.width as f64);
            ctx.set_line_cap(match stroke.cap {
                LineCap::Butt => "butt",
                LineCap::Round => "round",
                LineCap::Square => "square",
            });
            ctx.set_line_join(match stroke.join {
                LineJoin::Miter => "miter",
                LineJoin::Round => "round",
                LineJoin::Bevel => "bevel",
            });
            ctx.set_miter_limit(stroke.miter_limit as f64);
            ctx.stroke();
        }
        DrawOp::Clip { path, fill_rule } => {
            build_path(ctx, path);
            ctx.clip_with_canvas_winding_rule(match fill_rule {
                FillRule::NonZero => CanvasWindingRule::Nonzero,
                FillRule::EvenOdd => CanvasWindingRule::Evenodd,
            });
        }
        // `DrawOp` is `#[non_exhaustive]`; future ops no-op until wired.
        _ => {}
    }
}

fn build_path(ctx: &CanvasRenderingContext2d, path: &Path) {
    ctx.begin_path();
    for seg in &path.segs {
        match seg {
            PathSeg::MoveTo { x, y } => ctx.move_to(*x as f64, *y as f64),
            PathSeg::LineTo { x, y } => ctx.line_to(*x as f64, *y as f64),
            PathSeg::QuadTo { cx, cy, x, y } => {
                ctx.quadratic_curve_to(*cx as f64, *cy as f64, *x as f64, *y as f64)
            }
            PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y } => ctx.bezier_curve_to(
                *c1x as f64,
                *c1y as f64,
                *c2x as f64,
                *c2y as f64,
                *x as f64,
                *y as f64,
            ),
            PathSeg::Close => ctx.close_path(),
        }
    }
}

fn apply_fill_paint(ctx: &CanvasRenderingContext2d, paint: &Paint) {
    match &paint.kind {
        PaintKind::Solid(c) => ctx.set_fill_style_str(&rgba_css(*c)),
        PaintKind::Linear(g) => ctx.set_fill_style_canvas_gradient(&linear_gradient(ctx, g)),
        PaintKind::Radial(g) => {
            if let Some(grad) = radial_gradient(ctx, g) {
                ctx.set_fill_style_canvas_gradient(&grad);
            }
        }
        // `PaintKind` is `#[non_exhaustive]`; unknown paints draw nothing.
        _ => ctx.set_fill_style_str("rgba(0,0,0,0)"),
    }
}

fn apply_stroke_paint(ctx: &CanvasRenderingContext2d, paint: &Paint) {
    match &paint.kind {
        PaintKind::Solid(c) => ctx.set_stroke_style_str(&rgba_css(*c)),
        PaintKind::Linear(g) => ctx.set_stroke_style_canvas_gradient(&linear_gradient(ctx, g)),
        PaintKind::Radial(g) => {
            if let Some(grad) = radial_gradient(ctx, g) {
                ctx.set_stroke_style_canvas_gradient(&grad);
            }
        }
        _ => ctx.set_stroke_style_str("rgba(0,0,0,0)"),
    }
}

fn linear_gradient(ctx: &CanvasRenderingContext2d, g: &LinearGradient) -> CanvasGradient {
    let grad =
        ctx.create_linear_gradient(g.x0 as f64, g.y0 as f64, g.x1 as f64, g.y1 as f64);
    for s in &g.stops {
        let _ = grad.add_color_stop(s.offset, &rgba_css(s.color));
    }
    grad
}

fn radial_gradient(ctx: &CanvasRenderingContext2d, g: &RadialGradient) -> Option<CanvasGradient> {
    let grad = ctx
        .create_radial_gradient(g.cx as f64, g.cy as f64, 0.0, g.cx as f64, g.cy as f64, g.r as f64)
        .ok()?;
    for s in &g.stops {
        let _ = grad.add_color_stop(s.offset, &rgba_css(s.color));
    }
    Some(grad)
}

/// `Rgba` → CSS `rgba(r,g,b,a)` with alpha in `0..=1`.
fn rgba_css(c: Color) -> String {
    format!("rgba({},{},{},{})", c.r, c.g, c.b, c.a as f32 / 255.0)
}

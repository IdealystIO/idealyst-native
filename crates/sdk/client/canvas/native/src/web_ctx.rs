//! The browser surface the web rasterizer drives, as web-glue bindings:
//! the Canvas2D context, gradients, the `<canvas>` / `<video>` elements it
//! composites, `captureStream` and its track.
//!
//! One `import!` snippet per call, named after the web-sys method it
//! replaces, so `web.rs` reads as it did on web-sys. Canvas2D calls are the
//! per-frame hot path (a scene replays every op on every repaint); each is
//! one wasm → JS crossing, the same count web-sys made.

use web_glue::dom::{Document, HtmlCanvasElement};
use web_glue::{string, JsCast, JsValue};

web_glue::js_class! {
    /// `CanvasRenderingContext2D`.
    pub(crate) struct Ctx2d = "CanvasRenderingContext2D";
    /// `CanvasGradient`.
    pub(crate) struct Gradient = "CanvasGradient";
    /// `HTMLVideoElement` — a texture layer's hidden player.
    pub(crate) struct Video = "HTMLVideoElement";
    /// `CanvasCaptureMediaStreamTrack` — the manual-capture track.
    pub(crate) struct CaptureTrack = "CanvasCaptureMediaStreamTrack";
}

web_glue::import! {
    fn js_get_2d(c: u32) -> u32 =
        "(c) => { const x = G.get(c).getContext('2d'); return x == null ? 0 : G.add(x); }";
    fn js_save(c: u32) = "(c) => { G.get(c).save(); }";
    fn js_restore(c: u32) = "(c) => { G.get(c).restore(); }";
    fn js_transform(c: u32, a: f64, b: f64, cc: f64, d: f64, e: f64, f: f64) =
        "(c, a, b, cc, d, e, f) => { G.get(c).transform(a, b, cc, d, e, f); }";
    fn js_set_transform(c: u32, a: f64, b: f64, cc: f64, d: f64, e: f64, f: f64) =
        "(c, a, b, cc, d, e, f) => { G.get(c).setTransform(a, b, cc, d, e, f); }";
    fn js_reset_transform(c: u32) = "(c) => { G.get(c).resetTransform(); }";
    // `getTransform()` written into six f64s at `out` (8-aligned). The view
    // is taken after the call; nothing re-enters wasm in between.
    fn js_get_transform(c: u32, out: usize) =
        "(c, o) => { const m = G.get(c).getTransform(); \
           const v = new Float64Array(G.u8().buffer, o >>> 0, 6); \
           v[0] = m.a; v[1] = m.b; v[2] = m.c; v[3] = m.d; v[4] = m.e; v[5] = m.f; }";
    fn js_scale(c: u32, x: f64, y: f64) = "(c, x, y) => { G.get(c).scale(x, y); }";
    fn js_clear_rect(c: u32, x: f64, y: f64, w: f64, h: f64) =
        "(c, x, y, w, h) => { G.get(c).clearRect(x, y, w, h); }";
    fn js_begin_path(c: u32) = "(c) => { G.get(c).beginPath(); }";
    fn js_close_path(c: u32) = "(c) => { G.get(c).closePath(); }";
    fn js_move_to(c: u32, x: f64, y: f64) = "(c, x, y) => { G.get(c).moveTo(x, y); }";
    fn js_line_to(c: u32, x: f64, y: f64) = "(c, x, y) => { G.get(c).lineTo(x, y); }";
    fn js_quad_to(c: u32, cx: f64, cy: f64, x: f64, y: f64) =
        "(c, cx, cy, x, y) => { G.get(c).quadraticCurveTo(cx, cy, x, y); }";
    fn js_bezier_to(c: u32, ax: f64, ay: f64, bx: f64, by: f64, x: f64, y: f64) =
        "(c, ax, ay, bx, by, x, y) => { G.get(c).bezierCurveTo(ax, ay, bx, by, x, y); }";
    // rule: 0 nonzero (the default), 1 evenodd.
    fn js_fill(c: u32, rule: u32) = "(c, r) => { if (r) G.get(c).fill('evenodd'); else G.get(c).fill(); }";
    fn js_clip(c: u32, rule: u32) = "(c, r) => { G.get(c).clip(r ? 'evenodd' : 'nonzero'); }";
    fn js_stroke(c: u32) = "(c) => { G.get(c).stroke(); }";
    fn js_set_num(c: u32, which: u32, v: f64) =
        "(c, w, v) => { const x = G.get(c); \
           if (w === 0) x.lineWidth = v; else if (w === 1) x.miterLimit = v; \
           else if (w === 2) x.lineDashOffset = v; else x.globalAlpha = v; }";
    fn js_set_str(c: u32, which: u32, p: usize, l: usize) =
        "(c, w, p, l) => { const x = G.get(c), s = G.str(p, l); \
           if (w === 0) x.lineCap = s; else if (w === 1) x.lineJoin = s; \
           else if (w === 2) x.globalCompositeOperation = s; \
           else if (w === 3) x.fillStyle = s; else x.strokeStyle = s; }";
    // which: 0 fill, 1 stroke.
    fn js_set_style_obj(c: u32, which: u32, g: u32) =
        "(c, w, g) => { if (w) G.get(c).strokeStyle = G.get(g); else G.get(c).fillStyle = G.get(g); }";
    // `setLineDash` from `n` f64s at `p`, copied into a JS array first.
    fn js_set_line_dash(c: u32, p: usize, n: u32) =
        "(c, p, n) => { G.get(c).setLineDash(Array.from(new Float64Array(G.u8().buffer, p >>> 0, n))); }";
    fn js_linear_gradient(c: u32, x0: f64, y0: f64, x1: f64, y1: f64) -> u32 =
        "(c, x0, y0, x1, y1) => G.add(G.get(c).createLinearGradient(x0, y0, x1, y1))";
    #[catch]
    fn js_radial_gradient(c: u32, x0: f64, y0: f64, r0: f64, x1: f64, y1: f64, r1: f64) -> u32 =
        "(c, x0, y0, r0, x1, y1, r1) => G.add(G.get(c).createRadialGradient(x0, y0, r0, x1, y1, r1))";
    #[catch]
    fn js_add_color_stop(g: u32, o: f64, p: usize, l: usize) =
        "(g, o, p, l) => { G.get(g).addColorStop(o, G.str(p, l)); }";
    #[catch]
    fn js_round_rect(c: u32, x: f64, y: f64, w: f64, h: f64, r: f64) =
        "(c, x, y, w, h, r) => { G.get(c).roundRect(x, y, w, h, r); }";
    #[catch]
    fn js_draw_image(c: u32, s: u32, dx: f64, dy: f64) =
        "(c, s, dx, dy) => { G.get(c).drawImage(G.get(s), dx, dy); }";
    #[catch]
    fn js_draw_image_dwdh(c: u32, s: u32, dx: f64, dy: f64, dw: f64, dh: f64) =
        "(c, s, dx, dy, dw, dh) => { G.get(c).drawImage(G.get(s), dx, dy, dw, dh); }";
    #[catch]
    fn js_draw_image_src(c: u32, s: u32, sx: f64, sy: f64, sw: f64, sh: f64, dx: f64, dy: f64, dw: f64, dh: f64) =
        "(c, s, sx, sy, sw, sh, dx, dy, dw, dh) => { G.get(c).drawImage(G.get(s), sx, sy, sw, sh, dx, dy, dw, dh); }";
    fn js_ctx_canvas(c: u32) -> u32 = "(c) => { const e = G.get(c).canvas; return e == null ? 0 : G.add(e); }";
    // Raw RGBA `(p, l)` → `ImageData` → `putImageData` at the origin. The
    // bytes are copied (`slice`) before any other call, never viewed later.
    #[catch]
    fn js_put_rgba(c: u32, p: usize, l: usize, w: u32, h: u32) =
        "(c, p, l, w, h) => { const b = G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0)); \
           G.get(c).putImageData(new ImageData(new Uint8ClampedArray(b.buffer), w, h), 0, 0); }";

    // ---- <video> (texture layers) ----------------------------------------
    fn js_new_layer_video(d: u32) -> u32 =
        "(d) => { const v = G.get(d).createElement('video'); v.muted = true; v.autoplay = true; \
           v.setAttribute('playsinline', ''); return G.add(v); }";
    // `srcObject = s; play()`, the autoplay/interrupted rejection observed.
    fn js_video_attach(v: u32, s: u32) =
        "(v, s) => { const e = G.get(v); e.srcObject = G.get(s); const p = e.play(); if (p) p.catch(() => {}); }";
    fn js_video_width(v: u32) -> u32 = "(v) => G.get(v).videoWidth >>> 0";
    fn js_video_height(v: u32) -> u32 = "(v) => G.get(v).videoHeight >>> 0";

    // ---- captureStream ------------------------------------------------------
    // `captureStream(0)` (manual mode); 0 when unsupported or it throws.
    fn js_capture_stream(c: u32) -> u32 =
        "(c) => { const e = G.get(c); if (typeof e.captureStream !== 'function') return 0; \
           try { return G.add(e.captureStream(0)); } catch (_) { return 0; } }";
    fn js_first_video_track(s: u32) -> u32 =
        "(s) => { const t = G.get(s).getVideoTracks()[0]; return t == null ? 0 : G.add(t); }";
    fn js_request_frame(t: u32) = "(t) => { G.get(t).requestFrame(); }";
}

fn opt<T: JsCast>(idx: u32) -> Option<T> {
    // SAFETY: `idx` is a fresh handle from `G.add`; 0 (undefined) and 1
    // (null) are the permanent "nothing" slots.
    (idx > 1).then(|| T::unchecked_from_js(unsafe { JsValue::from_raw(idx) }))
}

fn h(v: &impl JsCast) -> u32 {
    v.as_ref().raw()
}

/// The `2d` context of `canvas` (`None` if the canvas is already bound to
/// another context type).
pub(crate) fn context_2d(canvas: &HtmlCanvasElement) -> Option<Ctx2d> {
    // SAFETY: a live canvas handle.
    opt(unsafe { js_get_2d(h(canvas)) })
}

/// A detached `<canvas>`, or `None` without a document.
pub(crate) fn new_canvas(document: &Document) -> Option<HtmlCanvasElement> {
    document.create_element("canvas").ok()?.dyn_into().ok()
}

// Every method below is one binding call on a live handle; the `unsafe`
// blocks only pass handles and borrowed (ptr, len) pairs.
#[allow(clippy::too_many_arguments)]
impl Ctx2d {
    pub(crate) fn save(&self) {
        unsafe { js_save(h(self)) }
    }
    pub(crate) fn restore(&self) {
        unsafe { js_restore(h(self)) }
    }
    pub(crate) fn transform(&self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
        unsafe { js_transform(h(self), a, b, c, d, e, f) }
    }
    pub(crate) fn set_transform(&self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
        unsafe { js_set_transform(h(self), a, b, c, d, e, f) }
    }
    pub(crate) fn reset_transform(&self) {
        unsafe { js_reset_transform(h(self)) }
    }
    /// `getTransform()` as `[a, b, c, d, e, f]`.
    pub(crate) fn get_transform(&self) -> [f64; 6] {
        let mut m = [0.0f64; 6];
        unsafe { js_get_transform(h(self), m.as_mut_ptr() as usize) };
        m
    }
    pub(crate) fn scale(&self, x: f64, y: f64) {
        unsafe { js_scale(h(self), x, y) }
    }
    pub(crate) fn clear_rect(&self, x: f64, y: f64, w: f64, hh: f64) {
        unsafe { js_clear_rect(h(self), x, y, w, hh) }
    }
    pub(crate) fn begin_path(&self) {
        unsafe { js_begin_path(h(self)) }
    }
    pub(crate) fn close_path(&self) {
        unsafe { js_close_path(h(self)) }
    }
    pub(crate) fn move_to(&self, x: f64, y: f64) {
        unsafe { js_move_to(h(self), x, y) }
    }
    pub(crate) fn line_to(&self, x: f64, y: f64) {
        unsafe { js_line_to(h(self), x, y) }
    }
    pub(crate) fn quadratic_curve_to(&self, cx: f64, cy: f64, x: f64, y: f64) {
        unsafe { js_quad_to(h(self), cx, cy, x, y) }
    }
    pub(crate) fn bezier_curve_to(&self, ax: f64, ay: f64, bx: f64, by: f64, x: f64, y: f64) {
        unsafe { js_bezier_to(h(self), ax, ay, bx, by, x, y) }
    }
    /// `fill()` (non-zero) or `fill("evenodd")`.
    pub(crate) fn fill(&self, even_odd: bool) {
        unsafe { js_fill(h(self), even_odd as u32) }
    }
    pub(crate) fn clip(&self, even_odd: bool) {
        unsafe { js_clip(h(self), even_odd as u32) }
    }
    pub(crate) fn stroke(&self) {
        unsafe { js_stroke(h(self)) }
    }
    pub(crate) fn set_line_width(&self, v: f64) {
        unsafe { js_set_num(h(self), 0, v) }
    }
    pub(crate) fn set_miter_limit(&self, v: f64) {
        unsafe { js_set_num(h(self), 1, v) }
    }
    pub(crate) fn set_line_dash_offset(&self, v: f64) {
        unsafe { js_set_num(h(self), 2, v) }
    }
    pub(crate) fn set_global_alpha(&self, v: f64) {
        unsafe { js_set_num(h(self), 3, v) }
    }
    fn set_str(&self, which: u32, s: &str) {
        let (p, l) = string::abi(s);
        unsafe { js_set_str(h(self), which, p, l) }
    }
    pub(crate) fn set_line_cap(&self, s: &str) {
        self.set_str(0, s)
    }
    pub(crate) fn set_line_join(&self, s: &str) {
        self.set_str(1, s)
    }
    pub(crate) fn set_global_composite_operation(&self, s: &str) {
        self.set_str(2, s)
    }
    pub(crate) fn set_fill_style_str(&self, s: &str) {
        self.set_str(3, s)
    }
    pub(crate) fn set_stroke_style_str(&self, s: &str) {
        self.set_str(4, s)
    }
    pub(crate) fn set_fill_style_gradient(&self, g: &Gradient) {
        unsafe { js_set_style_obj(h(self), 0, h(g)) }
    }
    pub(crate) fn set_stroke_style_gradient(&self, g: &Gradient) {
        unsafe { js_set_style_obj(h(self), 1, h(g)) }
    }
    /// `setLineDash(segments)`; an empty slice resets to solid.
    pub(crate) fn set_line_dash(&self, segments: &[f64]) {
        unsafe { js_set_line_dash(h(self), segments.as_ptr() as usize, segments.len() as u32) }
    }
    pub(crate) fn create_linear_gradient(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Gradient {
        Gradient::unchecked_from_js(unsafe {
            JsValue::from_raw(js_linear_gradient(h(self), x0, y0, x1, y1))
        })
    }
    /// `Err` for a negative radius (the DOM throws).
    pub(crate) fn create_radial_gradient(
        &self,
        x0: f64,
        y0: f64,
        r0: f64,
        x1: f64,
        y1: f64,
        r1: f64,
    ) -> Result<Gradient, web_glue::JsError> {
        unsafe { js_radial_gradient(h(self), x0, y0, r0, x1, y1, r1) }
            .map(|i| Gradient::unchecked_from_js(unsafe { JsValue::from_raw(i) }))
    }
    /// `roundRect` — `Err` where the engine lacks it (pre-2023 browsers).
    pub(crate) fn round_rect(&self, x: f64, y: f64, w: f64, hh: f64, r: f64) -> Result<(), web_glue::JsError> {
        unsafe { js_round_rect(h(self), x, y, w, hh, r) }
    }
    /// `drawImage(src, dx, dy)` — `src` is a canvas or video element.
    pub(crate) fn draw_image(&self, src: &JsValue, dx: f64, dy: f64) -> Result<(), web_glue::JsError> {
        unsafe { js_draw_image(h(self), src.raw(), dx, dy) }
    }
    pub(crate) fn draw_image_dw_dh(
        &self,
        src: &JsValue,
        dx: f64,
        dy: f64,
        dw: f64,
        dh: f64,
    ) -> Result<(), web_glue::JsError> {
        unsafe { js_draw_image_dwdh(h(self), src.raw(), dx, dy, dw, dh) }
    }
    pub(crate) fn draw_image_src_dst(
        &self,
        src: &JsValue,
        sx: f64,
        sy: f64,
        sw: f64,
        sh: f64,
        dx: f64,
        dy: f64,
        dw: f64,
        dh: f64,
    ) -> Result<(), web_glue::JsError> {
        unsafe { js_draw_image_src(h(self), src.raw(), sx, sy, sw, sh, dx, dy, dw, dh) }
    }
    /// The context's `<canvas>`.
    pub(crate) fn canvas(&self) -> Option<HtmlCanvasElement> {
        opt(unsafe { js_ctx_canvas(h(self)) })
    }
    /// `putImageData(new ImageData(rgba, w, h), 0, 0)`.
    pub(crate) fn put_rgba(&self, rgba: &[u8], w: u32, hh: u32) -> Result<(), web_glue::JsError> {
        unsafe { js_put_rgba(h(self), rgba.as_ptr() as usize, rgba.len(), w, hh) }
    }
}

impl Gradient {
    pub(crate) fn add_color_stop(&self, offset: f32, color: &str) -> Result<(), web_glue::JsError> {
        let (p, l) = string::abi(color);
        unsafe { js_add_color_stop(h(self), offset as f64, p, l) }
    }
}

impl Video {
    /// A detached, muted, autoplaying, `playsinline` `<video>`.
    pub(crate) fn new_layer(document: &Document) -> Video {
        Video::unchecked_from_js(unsafe { JsValue::from_raw(js_new_layer_video(h(document))) })
    }
    /// Attach `stream` as `srcObject` and start playback.
    pub(crate) fn attach(&self, stream: &JsValue) {
        unsafe { js_video_attach(h(self), stream.raw()) }
    }
    pub(crate) fn video_width(&self) -> u32 {
        unsafe { js_video_width(h(self)) }
    }
    pub(crate) fn video_height(&self) -> u32 {
        unsafe { js_video_height(h(self)) }
    }
}

/// `canvas.captureStream(0)` and its first video track as a
/// `CanvasCaptureMediaStreamTrack`; `None` when either is unavailable.
pub(crate) fn capture_stream(
    canvas: &HtmlCanvasElement,
) -> Option<(web_glue::dom::MediaStream, CaptureTrack)> {
    let stream: web_glue::dom::MediaStream = opt(unsafe { js_capture_stream(h(canvas)) })?;
    let track: JsValue = opt(unsafe { js_first_video_track(h(&stream)) })?;
    let track = track.dyn_into::<CaptureTrack>().ok()?;
    Some((stream, track))
}

impl CaptureTrack {
    pub(crate) fn request_frame(&self) {
        unsafe { js_request_frame(h(self)) }
    }
}

//! The texture-layer blit shared by every GPU layer compositor — macOS
//! (`native_capture.rs`, zero-copy IOSurface), Linux (`native_capture_linux.rs`,
//! uploaded CPU frames) and web (`web_layer.rs`, `<video>` copies). They differ
//! only in how a layer's source becomes a sampled texture; the shader and the
//! geometry live here once, so every GPU backend frames a layer identically —
//! and identically to the CPU renderers, because the geometry comes from the
//! same [`TextureLayer::source_rects`] they draw with (crop, then fit).
//!
//! # Uniform slots
//!
//! Each compositor keeps one uniform buffer with a [`LAYER_STRIDE`] slot per
//! layer, written with `queue.write_buffer` and bound with a dynamic offset.
//! Queued writes land at the NEXT queue submit — before any command buffer of
//! that submit runs — so two draws recorded into unsubmitted encoders that read
//! the same slot both see the LAST write. The slot is therefore the layer's
//! index in `CanvasProps::layers` ([`slot_offset`]), never its position in the
//! list being composited: a scene places layers one at a time
//! (`DrawOp::Texture`), possibly several into one encoder, and a per-call
//! position would put every one of them in slot 0. A layer composited twice in
//! one frame (`texture(i)` called twice) writes identical bytes to its slot, so
//! that is harmless.

use canvas_core::TextureLayer;

/// Per-layer uniform stride — ≥ the 256-byte `min_uniform_buffer_offset_alignment`
/// so each layer's uniform sits in its own dynamic-offset slot.
pub(crate) const LAYER_STRIDE: u64 = 256;
/// Max layers per canvas (sizes the uniform buffer); layers past it are skipped.
pub(crate) const MAX_LAYERS: usize = 16;
/// Bytes of one layer's uniform ([`LayerBlit::uniform`]).
pub(crate) const LAYER_UNIFORM_SIZE: u64 = 64;

/// The uniform-buffer offset of layer `index` (its index in
/// `CanvasProps::layers`), or `None` past [`MAX_LAYERS`].
pub(crate) fn slot_offset(index: usize) -> Option<u64> {
    (index < MAX_LAYERS).then_some(index as u64 * LAYER_STRIDE)
}

/// WGSL for a layer blit: a fullscreen triangle clipped to the render-pass
/// viewport (the drawn rect, clamped to the target). Everything is computed
/// from the fragment's FRAMEBUFFER position, not the viewport-relative UV, so
/// clamping the viewport to the target (a layer dragged partly off-canvas) clips
/// the layer instead of re-fitting it into the visible part.
pub(crate) const LAYER_BLIT_WGSL: &str = r#"
struct Layer {
    uv: vec4<f32>,     // framebuffer px -> texture uv: scale.xy, offset.zw
    geo: vec4<f32>,    // drawn_w_px, drawn_h_px, radius_px, opacity
    border: vec4<f32>, // border_width_px, use_src_alpha, drawn_center.xy (px)
    bcolor: vec4<f32>, // border r, g, b, a (0..1)
};
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> layer: Layer;

struct VsOut { @builtin(position) pos: vec4<f32> };
@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: VsOut;
    out.pos = vec4<f32>(p[i], 0.0, 1.0);
    return out;
}

fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    // `in.pos.xy` is the pixel centre in framebuffer pixels (top-left origin).
    let suv = in.pos.xy * layer.uv.xy + layer.uv.zw;
    let texel = textureSample(tex, samp, clamp(suv, vec2<f32>(0.0), vec2<f32>(1.0)));
    // Rounded-rect mask over the DRAWN rect (letterboxed for Contain, as the
    // CPU renderers clip), anti-aliased over ~1px.
    let size = layer.geo.xy;
    let radius = layer.geo.z;
    let opacity = layer.geo.w;
    let pp = in.pos.xy - layer.border.zw;
    let d = sd_round_box(pp, size * 0.5, radius);
    let aa = 1.0 - smoothstep(-1.0, 1.0, d);
    // Video layers are opaque (`use_src_alpha` = 0): source alpha ignored, mask
    // by corner + opacity. Image layers (watermark/logo, `use_src_alpha` = 1)
    // multiply in the texel's straight alpha so transparent PNG regions read
    // through. Straight alpha throughout; the pipeline alpha-blends over the
    // target.
    let src_a = mix(1.0, texel.a, layer.border.y);
    var rgb = texel.rgb;
    var a = aa * opacity * src_a;
    // Border ring, composited WITH the image so the frame stays locked to the
    // picture. `aa` is the outer coverage; `inner` the coverage of the rect
    // shrunk by the border width — their difference is the ring.
    let bw = layer.border.x;
    if (bw > 0.0) {
        let inner = 1.0 - smoothstep(-1.0, 1.0, d + bw);
        let bcov = clamp(aa - inner, 0.0, 1.0);
        rgb = mix(rgb, layer.bcolor.rgb, bcov);
        a = mix(a, layer.bcolor.a * opacity, bcov);
    }
    return vec4<f32>(rgb, a);
}
"#;

/// One layer's draw: the render-pass viewport and the uniform bytes for its slot.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LayerBlit {
    /// `(x, y, w, h)` in target pixels — the drawn rect clamped to the target.
    pub viewport: (f32, f32, f32, f32),
    pub uniform: [u8; LAYER_UNIFORM_SIZE as usize],
}

/// Geometry for drawing `layer` from a `tex_w × tex_h` texture into a
/// `target_w × target_h` target whose pixels are `scale` × the canvas's logical
/// units. `use_src_alpha` is 1 for image layers (straight alpha honoured), 0 for
/// opaque video. `None` when nothing would be visible (empty source, empty or
/// fully off-target rect).
///
/// The source and drawn rects come from [`TextureLayer::source_rects`] — the
/// function every CPU renderer draws with — so crop and fit match across
/// renderers. The rounded mask and border sit on the DRAWN rect (inside the
/// letterbox for `Fit::Contain`), as the CPU renderers clip, with the radius
/// clamped to half the drawn rect's shorter side.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layer_blit(
    layer: &TextureLayer,
    tex_w: u32,
    tex_h: u32,
    use_src_alpha: bool,
    scale: f32,
    target_w: u32,
    target_h: u32,
) -> Option<LayerBlit> {
    if tex_w == 0 || tex_h == 0 {
        return None;
    }
    let (tw, th) = (tex_w as f32, tex_h as f32);
    let ((sx, sy, sw, sh), (dx, dy, dw, dh)) = layer.source_rects(tw, th);
    // Logical → target pixels.
    let (dx, dy, dw, dh) = (dx * scale, dy * scale, dw * scale, dh * scale);
    if dw < 1.0 || dh < 1.0 || sw <= 0.0 || sh <= 0.0 {
        return None;
    }
    // Clamp the viewport to the target so a partly off-target rect doesn't trip
    // wgpu's "viewport out of bounds" validation. The shader works in
    // framebuffer coordinates, so this only clips.
    let vx = dx.clamp(0.0, target_w as f32);
    let vy = dy.clamp(0.0, target_h as f32);
    let vw = (dx + dw).clamp(0.0, target_w as f32) - vx;
    let vh = (dy + dh).clamp(0.0, target_h as f32) - vy;
    if vw < 1.0 || vh < 1.0 {
        return None;
    }
    // Framebuffer pixel p → source pixel sx + (p - dx) * sw/dw → uv (÷ tex size).
    let (kx, ky) = (sw / dw, sh / dh);
    let uv = [kx / tw, ky / th, (sx - dx * kx) / tw, (sy - dy * ky) / th];
    let radius = ((layer.corner_radius)() * scale).clamp(0.0, dw.min(dh) * 0.5);
    let border = (layer.border_width * scale).max(0.0);
    let bc = layer.border_color;
    let u: [f32; 16] = [
        uv[0], uv[1], uv[2], uv[3],
        dw, dh, radius, layer.opacity.clamp(0.0, 1.0),
        border, use_src_alpha as u32 as f32, dx + dw * 0.5, dy + dh * 0.5,
        bc.r as f32 / 255.0, bc.g as f32 / 255.0, bc.b as f32 / 255.0, bc.a as f32 / 255.0,
    ];
    let mut uniform = [0u8; LAYER_UNIFORM_SIZE as usize];
    for (j, f) in u.iter().enumerate() {
        uniform[j * 4..j * 4 + 4].copy_from_slice(&f.to_ne_bytes());
    }
    Some(LayerBlit { viewport: (vx, vy, vw, vh), uniform })
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{Fit, ImageSource};
    use std::rc::Rc;
    use std::sync::Arc;

    fn layer(rect: (f32, f32, f32, f32)) -> TextureLayer {
        let img = Arc::new(ImageSource::from_rgba8(1, 1, 1, vec![0, 0, 0, 255]));
        TextureLayer::image(Rc::new(move || Some(img.clone())), Rc::new(move || rect))
    }

    fn floats(b: &LayerBlit) -> [f32; 16] {
        let mut out = [0f32; 16];
        for (j, f) in out.iter_mut().enumerate() {
            *f = f32::from_ne_bytes(b.uniform[j * 4..j * 4 + 4].try_into().unwrap());
        }
        out
    }

    /// Map framebuffer pixel `(x, y)` to source pixels the way the shader does.
    fn sample_px(b: &LayerBlit, tw: f32, th: f32, x: f32, y: f32) -> (f32, f32) {
        let u = floats(b);
        ((x * u[0] + u[2]) * tw, (y * u[1] + u[3]) * th)
    }

    /// The GPU must frame a layer exactly as `source_rects` (the CPU renderers'
    /// geometry): with Contain, the drawn rect is the letterboxed one and its
    /// edges sample the source edges.
    #[test]
    fn contain_draws_into_the_letterboxed_rect_like_the_cpu_renderers() {
        let l = layer((0.0, 0.0, 100.0, 100.0)).fit(Fit::Contain);
        let b = layer_blit(&l, 200, 100, false, 2.0, 400, 400).unwrap();
        // 200×100 into 100×100 → (0, 25, 100, 50) logical → ×2 px.
        assert_eq!(b.viewport, (0.0, 50.0, 200.0, 100.0));
        assert_eq!(sample_px(&b, 200.0, 100.0, 0.0, 50.0), (0.0, 0.0));
        assert_eq!(sample_px(&b, 200.0, 100.0, 200.0, 150.0), (200.0, 100.0));
        let u = floats(&b);
        assert_eq!((u[4], u[5]), (200.0, 100.0), "mask/border on the drawn rect");
        assert_eq!((u[10], u[11]), (100.0, 100.0), "drawn-rect centre");
    }

    /// `src_crop` applies before the fit on the GPU too (it used to be folded in
    /// only on macOS and ignored on web).
    #[test]
    fn src_crop_selects_the_source_region_before_the_fit() {
        let l = layer((0.0, 0.0, 50.0, 50.0)).fit(Fit::Fill).src_crop((0.5, 0.0, 0.5, 1.0));
        let b = layer_blit(&l, 200, 100, false, 1.0, 100, 100).unwrap();
        assert_eq!(sample_px(&b, 200.0, 100.0, 0.0, 0.0), (100.0, 0.0));
        assert_eq!(sample_px(&b, 200.0, 100.0, 50.0, 50.0), (200.0, 100.0));
    }

    /// A rect hanging off the target is clipped, not re-fitted into the visible
    /// part: the mapping is the same as for the unclamped rect.
    #[test]
    fn an_off_target_rect_is_clipped_not_refitted() {
        let l = layer((-50.0, 0.0, 100.0, 100.0)).fit(Fit::Fill);
        let b = layer_blit(&l, 100, 100, false, 1.0, 100, 100).unwrap();
        assert_eq!(b.viewport, (0.0, 0.0, 50.0, 100.0));
        // Target x=0 is the middle of the source.
        assert_eq!(sample_px(&b, 100.0, 100.0, 0.0, 0.0), (50.0, 0.0));
        assert!(layer_blit(&layer((200.0, 0.0, 10.0, 10.0)), 1, 1, false, 1.0, 100, 100).is_none());
    }

    #[test]
    fn radius_is_clamped_to_half_the_drawn_rect() {
        let l = layer((0.0, 0.0, 40.0, 10.0)).corner_radius(100.0);
        let b = layer_blit(&l, 4, 1, false, 1.0, 100, 100).unwrap();
        assert_eq!(floats(&b)[6], 5.0);
    }

    #[test]
    fn slots_are_keyed_by_layer_index() {
        assert_eq!(slot_offset(0), Some(0));
        assert_eq!(slot_offset(3), Some(3 * LAYER_STRIDE));
        assert_eq!(slot_offset(MAX_LAYERS), None);
    }
}

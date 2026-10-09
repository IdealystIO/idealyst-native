//! GPU-backed checks of the present blit and overlay compositor: real
//! pipelines on a headless device, bytes read back. Each test skips (with a
//! note) when the host has no adapter at all — the software fallback adapter
//! (lavapipe / WARP) is enough where one is installed.
#![cfg(not(target_arch = "wasm32"))]

use gpu_surface::{
    headless_device, headless_device_with_limits, make_target, read_target_rgba, target_usages,
    FrameAlpha, OverlayCompositor, PresentBlit, TARGET_FORMAT,
};

const W: u32 = 4;
const H: u32 = 4;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let d = headless_device();
    if d.is_none() {
        eprintln!("skipping: no GPU adapter on this host");
    }
    d
}

/// A `W`×`H` target filled with `top` in the top half and `bottom` below.
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    top: [u8; 4],
    bottom: [u8; 4],
) -> (wgpu::Texture, wgpu::TextureView) {
    let (tex, view) = make_target(device, W, H, "test-src");
    let mut bytes = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for _ in 0..W {
            bytes.extend_from_slice(if y < H / 2 { &top } else { &bottom });
        }
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: Some(H) },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    (tex, view)
}

fn blit(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::TextureView,
    alpha: FrameAlpha,
    dst_premultiplied: bool,
    flip_y: bool,
) -> gpu_surface::RenderedImage {
    let (dst, dst_view) = make_target(device, W, H, "test-dst");
    let blit = PresentBlit::new(device, TARGET_FORMAT, alpha, dst_premultiplied, flip_y);
    let mut enc = device.create_command_encoder(&Default::default());
    blit.draw(device, &mut enc, src, &dst_view);
    queue.submit([enc.finish()]);
    read_target_rgba(device, queue, &dst, W, H)
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (*x as i16 - y as i16).abs() <= 1)
}

/// canvas-vello's frame target holds STRAIGHT alpha (vello's fine shader
/// divides by alpha on store), but surfaces are configured `PreMultiplied`.
/// Before the present blit converted, the bytes were copied through and every
/// half-transparent pixel composited too bright over dark UI. A 50%-alpha red
/// texel must reach a premultiplied surface as (128, 0, 0, 128).
#[test]
fn regression_straight_frame_is_premultiplied_for_a_premultiplied_surface() {
    let Some((device, queue)) = device() else { return };
    let (_t, src) = upload(&device, &queue, [255, 0, 0, 128], [255, 0, 0, 128]);
    let out = blit(&device, &queue, &src, FrameAlpha::Straight, true, false);
    assert!(close(out.px(1, 1), [128, 0, 0, 128]), "got {:?}", out.px(1, 1));
}

#[test]
fn straight_frame_is_copied_for_a_straight_surface() {
    let Some((device, queue)) = device() else { return };
    let (_t, src) = upload(&device, &queue, [255, 0, 0, 128], [255, 0, 0, 128]);
    let out = blit(&device, &queue, &src, FrameAlpha::Straight, false, false);
    assert_eq!(out.px(1, 1), [255, 0, 0, 128]);
}

#[test]
fn premultiplied_frame_is_unpremultiplied_for_a_straight_surface() {
    let Some((device, queue)) = device() else { return };
    let (_t, src) = upload(&device, &queue, [128, 0, 0, 128], [128, 0, 0, 128]);
    let out = blit(&device, &queue, &src, FrameAlpha::Premultiplied, false, false);
    assert!(close(out.px(1, 1), [255, 0, 0, 128]), "got {:?}", out.px(1, 1));
    // Fully transparent texels stay black, not NaN.
    let (_t, clear) = upload(&device, &queue, [0, 0, 0, 0], [0, 0, 0, 0]);
    let out = blit(&device, &queue, &clear, FrameAlpha::Premultiplied, false, false);
    assert_eq!(out.px(0, 0), [0, 0, 0, 0]);
}

/// The GL (GTK4) present writes into a bottom-left-origin framebuffer, so the
/// blit flips V: the source's top row lands in the destination's last row.
/// Colour passes through as stored — byte 200 stays 200. A linear→sRGB
/// re-encode (host-linux-desktop's blit renders into an sRGB target and has
/// to put the transfer function back; ours must not) would turn it into ~229,
/// the double-gamma wash every present path forbids.
#[test]
fn flip_y_inverts_rows() {
    let Some((device, queue)) = device() else { return };
    let red = [200, 0, 0, 255];
    let blue = [0, 0, 200, 255];
    let (_t, src) = upload(&device, &queue, red, blue);
    let straight = blit(&device, &queue, &src, FrameAlpha::Premultiplied, true, false);
    assert_eq!(straight.px(0, 0), red);
    assert_eq!(straight.px(0, H - 1), blue);
    let flipped = blit(&device, &queue, &src, FrameAlpha::Premultiplied, true, true);
    assert_eq!(flipped.px(0, 0), blue);
    assert_eq!(flipped.px(0, H - 1), red);
}

/// A straight-alpha overlay composited over a TRANSPARENT premultiplied frame
/// (a 3D view with a clear background) keeps its colour: (255,0,0,128)
/// straight → (128,0,0,128) premultiplied. Straight-over-straight blending
/// would also give (128,0,0,128) here — but read as STRAIGHT, i.e. the
/// overlay darkened to half its colour. The premultiplied destination is
/// what makes this exact.
#[test]
fn straight_overlay_over_transparent_premultiplied_frame_is_exact() {
    let Some((device, queue)) = device() else { return };
    let (_s, src) = upload(&device, &queue, [255, 0, 0, 128], [255, 0, 0, 128]);
    let (dst, dst_view) = upload(&device, &queue, [0, 0, 0, 0], [0, 0, 0, 0]);
    let comp = OverlayCompositor::new(&device, FrameAlpha::Straight, FrameAlpha::Premultiplied);
    let mut enc = device.create_command_encoder(&Default::default());
    comp.composite(&device, &mut enc, &src, &dst_view);
    queue.submit([enc.finish()]);
    let out = read_target_rgba(&device, &queue, &dst, W, H);
    assert!(close(out.px(1, 1), [128, 0, 0, 128]), "got {:?}", out.px(1, 1));
}

/// Premultiplied overlay (vello_cpu's pixmap) over an opaque premultiplied
/// frame: standard source-over.
#[test]
fn premultiplied_overlay_over_opaque_frame() {
    let Some((device, queue)) = device() else { return };
    // 50% white over opaque blue.
    let (_s, src) = upload(&device, &queue, [128, 128, 128, 128], [0, 0, 0, 0]);
    let (dst, dst_view) = upload(&device, &queue, [0, 0, 255, 255], [0, 0, 255, 255]);
    let comp = OverlayCompositor::new(&device, FrameAlpha::Premultiplied, FrameAlpha::Premultiplied);
    let mut enc = device.create_command_encoder(&Default::default());
    comp.composite(&device, &mut enc, &src, &dst_view);
    queue.submit([enc.finish()]);
    let out = read_target_rgba(&device, &queue, &dst, W, H);
    assert!(close(out.px(1, 0), [128, 128, 255, 255]), "top: {:?}", out.px(1, 0));
    // Transparent overlay leaves the frame untouched.
    assert_eq!(out.px(1, H - 1), [0, 0, 255, 255]);
}

/// WebGL2 has no storage textures; requesting `STORAGE_BINDING` there fails
/// texture creation. On a device held to WebGL2's limits the frame target
/// must omit it — and still be creatable, blittable and readable.
#[test]
fn frame_target_is_creatable_under_webgl2_limits() {
    let Some((device, queue)) =
        headless_device_with_limits(Some(wgpu::Limits::downlevel_webgl2_defaults()))
    else {
        eprintln!("skipping: no GPU adapter on this host");
        return;
    };
    assert!(!target_usages(&device).contains(wgpu::TextureUsages::STORAGE_BINDING));
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let (_t, src) = upload(&device, &queue, [10, 20, 30, 255], [10, 20, 30, 255]);
    let out = blit(&device, &queue, &src, FrameAlpha::Straight, true, false);
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "validation error under WebGL2 limits: {err:?}");
    assert_eq!(out.px(0, 0), [10, 20, 30, 255]);
}

#[test]
fn frame_target_has_storage_where_the_device_allows_it() {
    let Some((device, _queue)) = device() else { return };
    if device.limits().max_storage_textures_per_shader_stage > 0 {
        assert!(target_usages(&device).contains(wgpu::TextureUsages::STORAGE_BINDING));
    }
}

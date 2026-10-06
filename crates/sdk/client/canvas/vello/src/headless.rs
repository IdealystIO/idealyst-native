//! Surface-less GPU compositor — the headless engine the `video-compose` SDK
//! drives to transform one `MediaStream` into another.
//!
//! The on-screen [`RenderState`](crate::render) is surface-bound: it owns a
//! `wgpu::Surface`/swapchain and its frame loop presents to a window. A video
//! compositor has no window — it composites a scene + a stack of
//! [`TextureLayer`]s (the input video as the base layer, a watermark image, a
//! picture-in-picture stream, drawn graphics) into an OFFSCREEN target and hands
//! the pixels to an output `MediaStream`. This struct factors exactly that shared
//! machinery out of the display path (CLAUDE.md §7): the vello scene render, the
//! [`LayerCompositor`] (same fit/rounded/opacity/border shader the on-screen
//! canvas uses), CPU read-back, and — on macOS — the zero-copy IOSurface output
//! ring ([`NativeCapture`]).
//!
//! It owns its OWN wgpu device (a real GPU adapter, software fallback for CI), so
//! it's independent of the app's on-screen canvas device. IOSurfaces are
//! cross-device, so a camera surface produced on the app's device imports fine
//! here, and the output ring this device produces is consumed by the recorder's
//! encoder (see [[project_canvas_self_capture]]).

use crate::compose::OverlayCompositor;
use crate::encode::encode_scene;
use crate::plan::split_segments;
use crate::texture_runs::{composite_texture_runs, RunHost};
use crate::native_capture::{LayerCompositor, NativeCapture};
use crate::render::{
    headless_device, make_target, new_vello_renderer, read_target_rgba, RenderedImage,
};
use canvas_core::{place_textures, DrawOp, Scene as CanvasScene, TextureLayer};
use media_stream::FrameWriter;
use vello::kurbo::Affine;
use vello::peniko::Color;
use vello::{AaConfig, RenderParams, Renderer, Scene as VelloScene};

/// A persistent, surface-less vello compositor. Build one with [`new`](Self::new),
/// then call [`composite`](Self::composite) per output frame; read the result
/// with [`read_rgba`](Self::read_rgba) (any platform) or publish it zero-copy to
/// an output stream with [`attach_output`](Self::attach_output) +
/// [`publish_output`](Self::publish_output) (macOS/Apple).
pub struct HeadlessCompositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    scene: VelloScene,
    /// The offscreen `Rgba8Unorm` render target, (re)made on size change.
    target: Option<(wgpu::Texture, wgpu::TextureView, (u32, u32))>,
    /// Secondary target for the ON-TOP overlay scene (drawn graphics/text): vello
    /// renders it over a transparent base, then it's source-over composited above
    /// the layers so text sits on top of the video (layers composite over the base
    /// scene, so an overlay can't just be the base scene). `None` until first use.
    overlay: Option<(wgpu::Texture, wgpu::TextureView, (u32, u32))>,
    overlay_compositor: OverlayCompositor,
    /// The shared texture-layer pass (real on macOS; a no-op stub elsewhere,
    /// where the compositor isn't wired yet).
    layer_compositor: LayerCompositor,
    /// Zero-copy output ring — publishes each composited frame's IOSurface to an
    /// output `MediaStream` (macOS). `None` until [`attach_output`](Self::attach_output).
    output: Option<NativeCapture>,
}

impl HeadlessCompositor {
    /// Create a compositor with its own headless GPU device (real adapter first,
    /// software fallback). `None` if no usable adapter/device or vello init fails.
    pub fn new() -> Option<Self> {
        let (device, queue) = headless_device()?;
        // Headless: a fresh native (non-GL) device, so parallel shader init is fine.
        let renderer = new_vello_renderer(&device, false)?;
        let layer_compositor = LayerCompositor::new(&device);
        let overlay_compositor = OverlayCompositor::new(&device);
        Some(Self {
            device,
            queue,
            renderer,
            scene: VelloScene::new(),
            target: None,
            overlay: None,
            overlay_compositor,
            layer_compositor,
            output: None,
        })
    }

    /// The compositor's own wgpu device (for callers that need to build GPU
    /// resources on the same device).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    fn ensure_target(&mut self, w: u32, h: u32) {
        if let Some((_, _, size)) = &self.target {
            if *size == (w, h) {
                return;
            }
        }
        let (tex, view) = make_target(&self.device, w, h);
        self.target = Some((tex, view, (w, h)));
    }

    fn ensure_overlay(&mut self, w: u32, h: u32) {
        if let Some((_, _, size)) = &self.overlay {
            if *size == (w, h) {
                return;
            }
        }
        let (tex, view) = make_target(&self.device, w, h);
        self.overlay = Some((tex, view, (w, h)));
    }

    /// Composite `base` and `layers` into the offscreen target at `w × h`
    /// physical pixels. `base` places layers like a canvas scene does:
    /// `Scene::texture(i)` composites `layers[i]` at that point (ops after it
    /// draw on top), and a layer it never places is composited after it, in
    /// order — so a `base` without texture ops draws first with every layer on
    /// top, as before texture ops existed. `scale` maps the author's
    /// LOGICAL-coordinate scene/layer rects onto the physical target (dpr); pass
    /// `1.0` to treat the scene coordinates as physical pixels.
    ///
    /// The base (input) video is just `layers[0]` with a full-target rect; a
    /// watermark / PiP stack above it. `overlay` (drawn text/graphics) is
    /// composited LAST, on top of the layers — pass an empty scene for none.
    /// Nothing is presented — call [`read_rgba`](Self::read_rgba) or
    /// [`publish_output`](Self::publish_output) to consume the result.
    pub fn composite(
        &mut self,
        base: &CanvasScene,
        layers: &[TextureLayer],
        overlay: &CanvasScene,
        w: u32,
        h: u32,
        scale: f32,
    ) {
        let (w, h) = (w.max(1), h.max(1));
        self.ensure_target(w, h);

        let params = |w, h| RenderParams {
            base_color: Color::from_rgba8(0, 0, 0, 0),
            width: w,
            height: h,
            antialiasing_method: AaConfig::Area,
        };

        // Normalize `base` exactly as `paint_scene` does for a canvas, so the
        // segments below are self-contained (see `canvas_core::place_textures`).
        // Without layers or texture ops there is nothing to place: skip the copy.
        let placed;
        let ops: &[DrawOp] = if layers.is_empty()
            && !base.ops().iter().any(|op| matches!(op, DrawOp::Texture { .. }))
        {
            base.ops()
        } else {
            placed = place_textures(CanvasScene::from_ops(base.ops().to_vec()), layers.len());
            placed.ops()
        };
        let segments = split_segments(ops);

        // Paint the base segment into the target (transparent base). Disjoint
        // field borrows: `&mut self.renderer`/`&mut self.scene` vs `&self.device`/
        // `&self.queue`/`&self.target` are distinct fields, so this is sound.
        self.scene.reset();
        encode_scene(segments.base, &mut self.scene, Affine::scale(scale as f64));
        {
            let (_, target_view, _) = self.target.as_ref().unwrap();
            // A render failure (device lost) leaves the previous frame in the
            // target; the next composite retries. Nothing to unwind here.
            let _ = self.renderer.render_to_texture(
                &self.device,
                &self.queue,
                &self.scene,
                target_view,
                &params(w, h),
            );
        }

        // Texture runs: each run's layers, then its vector ops over them (see
        // `crate::texture_runs`, which also owns the GPU-ordering rule). The
        // layers / scale / size for this frame ride along in `HeadlessRuns`.
        if !segments.runs.is_empty() {
            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("headless-layers"),
            });
            let mut host = HeadlessRuns { c: self, layers, scale, w, h };
            composite_texture_runs(&mut host, &segments.runs, &mut enc, false);
            self.queue.submit([enc.finish()]);
        }

        // Overlay scene (drawn text/graphics) ON TOP of the layers: render it over
        // a transparent overlay texture, then source-over composite onto the
        // target. Skipped when empty so a no-graphics pipeline pays nothing.
        if !overlay.is_empty() {
            self.ensure_overlay(w, h);
            self.scene.reset();
            encode_scene(overlay.ops(), &mut self.scene, Affine::scale(scale as f64));
            {
                let (_, overlay_view, _) = self.overlay.as_ref().unwrap();
                let _ = self.renderer.render_to_texture(
                    &self.device,
                    &self.queue,
                    &self.scene,
                    overlay_view,
                    &params(w, h),
                );
            }
            let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("headless-overlay"),
            });
            {
                let (_, target_view, _) = self.target.as_ref().unwrap();
                let (_, overlay_view, _) = self.overlay.as_ref().unwrap();
                self.overlay_compositor
                    .composite(&self.device, &mut enc, overlay_view, target_view);
            }
            self.queue.submit([enc.finish()]);
        }
    }

    /// Read the current composited target back to CPU as tightly-packed top-down
    /// RGBA8. Works on every backend (the CPU output path + tests). **Blocking.**
    /// `None` before the first [`composite`](Self::composite).
    pub fn read_rgba(&self) -> Option<RenderedImage> {
        let (target, _, (w, h)) = self.target.as_ref()?;
        Some(read_target_rgba(&self.device, &self.queue, target, *w, *h))
    }

    /// Attach an output `MediaStream` sink. Each subsequent
    /// [`publish_output`](Self::publish_output) blits the composited target into
    /// the next IOSurface of a zero-copy ring and publishes it (macOS; a no-op on
    /// other targets, which use [`read_rgba`](Self::read_rgba) + `write_rgba8`).
    pub fn attach_output(&mut self, writer: FrameWriter) {
        self.output = Some(NativeCapture::new(writer));
    }

    /// True while a consumer is tapping the attached output stream's native
    /// (zero-copy) source — gates the blit so an un-consumed output costs nothing.
    pub fn output_wants_native(&self) -> bool {
        self.output.as_ref().is_some_and(|nc| nc.wants())
    }

    /// Publish the current composited target to the attached output stream's
    /// zero-copy ring (macOS). No-op if no output is attached (or off macOS, where
    /// the ring is a stub) or nothing has been composited yet. Call AFTER
    /// [`composite`](Self::composite).
    ///
    /// Published UNCONDITIONALLY (not gated on `wants_native`): a display/preview
    /// consumer reads the output stream's native surface WITHOUT registering a
    /// recorder tap, so gating on the tap would starve the preview. The blit is a
    /// single GPU copy at the driver's frame cadence; a truly unused output only
    /// pays that (the driver stops entirely when the output stream is dropped).
    pub fn publish_output(&mut self) {
        let Some((_, _, (w, h))) = self.target.as_ref().map(|(t, v, s)| (t, v, *s)) else {
            return;
        };
        if self.output.is_none() {
            return;
        }
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("headless-output-blit"),
        });
        let idx = {
            let (_, target_view, _) = self.target.as_ref().unwrap();
            self.output
                .as_mut()
                .unwrap()
                .blit_into(&self.device, &mut enc, target_view, w, h)
        };
        self.queue.submit([enc.finish()]);
        // Publish AFTER submit so the blit is in flight; the ring guarantees the
        // surface isn't reused until POOL frames later (no fence needed).
        if let Some(idx) = idx {
            self.output.as_ref().unwrap().publish(idx);
        }
    }
}

/// [`HeadlessCompositor`] as a [`RunHost`] for one `composite` call: the
/// compositor plus that frame's layers, scale and size.
struct HeadlessRuns<'a> {
    c: &'a mut HeadlessCompositor,
    layers: &'a [TextureLayer],
    scale: f32,
    w: u32,
    h: u32,
}

impl RunHost for HeadlessRuns<'_> {
    type Enc = wgpu::CommandEncoder;

    fn composite_layers(&mut self, enc: &mut wgpu::CommandEncoder, which: &[u32]) {
        let c = &mut *self.c;
        let (_, target_view, _) = c.target.as_ref().unwrap();
        c.layer_compositor.composite_layers(
            &c.device, &c.queue, enc, self.layers, which, target_view, self.scale, self.w, self.h,
        );
    }

    fn render_overlay(&mut self, ops: &[DrawOp]) -> bool {
        let c = &mut *self.c;
        c.ensure_overlay(self.w, self.h);
        c.scene.reset();
        encode_scene(ops, &mut c.scene, Affine::scale(self.scale as f64));
        let (_, overlay_view, _) = c.overlay.as_ref().unwrap();
        let params = RenderParams {
            base_color: Color::from_rgba8(0, 0, 0, 0),
            width: self.w,
            height: self.h,
            antialiasing_method: AaConfig::Area,
        };
        c.renderer.render_to_texture(&c.device, &c.queue, &c.scene, overlay_view, &params).is_ok()
    }

    fn composite_overlay(&mut self, enc: &mut wgpu::CommandEncoder) {
        let c = &*self.c;
        let (_, target_view, _) = c.target.as_ref().unwrap();
        let (_, overlay_view, _) = c.overlay.as_ref().unwrap();
        c.overlay_compositor.composite(&c.device, enc, overlay_view, target_view);
    }

    fn submit(&mut self, enc: &mut wgpu::CommandEncoder) {
        let fresh = self.c.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("headless-layers"),
        });
        self.c.queue.submit([std::mem::replace(enc, fresh).finish()]);
    }
}

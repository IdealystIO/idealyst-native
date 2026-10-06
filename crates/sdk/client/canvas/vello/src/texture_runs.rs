//! Compositing a scene's texture runs — the part of a frame after its base
//! segment (see [`split_segments`](crate::plan::split_segments)). Shared by the
//! on-screen native renderer (`render.rs`), the web renderer (`render_web.rs`)
//! and the [`HeadlessCompositor`](crate::HeadlessCompositor), so all three draw
//! textures at their op position with the same GPU ordering.
//!
//! Per run: the run's layers are composited into the target by the layer
//! compositor, then the run's vector ops are rendered by vello into the
//! renderer's `overlay` texture (transparent base) and source-over composited
//! onto the target — so ops after a `Texture` op land over that texture.
//!
//! # GPU ordering
//!
//! vello's `render_to_texture` submits its own command buffer IMMEDIATELY, while
//! the composites are recorded into an encoder the renderer submits later (with
//! its present blit / capture). Queue submissions execute in order, so if
//! `overlay` were re-rendered while an unsubmitted composite still has to READ
//! it — the base segment's Hybrid/Cached content, or the previous run — the new
//! content would land first and the earlier composite would draw the wrong
//! pixels. [`composite_texture_runs`] therefore submits (and replaces) the
//! encoder before each such reuse. That costs one extra submit per run after the
//! first, and none for the common case of layers composited after all vector
//! content (the only case before texture ops existed). The layer compositor
//! needs no fence: its per-layer uniform slots are keyed by layer index (see
//! `layer_blit`).

use crate::plan::TextureRun;
use canvas_core::DrawOp;

/// What a renderer provides to [`composite_texture_runs`]. `Enc` is the
/// renderer's pending command encoder (`wgpu::CommandEncoder`).
pub(crate) trait RunHost {
    type Enc;
    /// Record compositing layers `which` (indices into the canvas's layers, in
    /// order) into the target.
    fn composite_layers(&mut self, enc: &mut Self::Enc, which: &[u32]);
    /// Render `ops` with vello into the overlay texture over a transparent
    /// base. Submits immediately (vello's `render_to_texture`). `false` if
    /// vello failed.
    fn render_overlay(&mut self, ops: &[DrawOp]) -> bool;
    /// Record compositing the overlay texture over the target.
    fn composite_overlay(&mut self, enc: &mut Self::Enc);
    /// Submit `enc` and replace it with a fresh encoder.
    fn submit(&mut self, enc: &mut Self::Enc);
}

/// Composite `runs` into the target (see the module docs). `overlay_pending`
/// is whether `enc` already holds a composite that reads the overlay texture
/// (the base segment's Hybrid/Cached content). The caller submits `enc`
/// afterwards — before its present blit and capture read the target, so they
/// see every run. Returns `false` if vello failed (skip the frame).
pub(crate) fn composite_texture_runs<H: RunHost>(
    host: &mut H,
    runs: &[TextureRun<'_>],
    enc: &mut H::Enc,
    mut overlay_pending: bool,
) -> bool {
    for run in runs {
        host.composite_layers(enc, &run.layers);
        if run.ops.is_empty() {
            continue;
        }
        if overlay_pending {
            host.submit(enc);
        }
        if !host.render_overlay(run.ops) {
            return false;
        }
        host.composite_overlay(enc);
        overlay_pending = true;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::split_segments;
    use canvas_core::{Color, Path, Scene};

    /// A model of the GPU queue: vello renders write the overlay at once,
    /// recorded composites run only when their encoder is submitted. Each
    /// overlay composite remembers which run's content it was recorded for and
    /// checks, when it actually executes, that the overlay still holds it.
    #[derive(Default)]
    struct QueueModel {
        overlay: Option<usize>,
        renders: usize,
        /// What ended up in the target, in execution order.
        target: Vec<String>,
        bad_reads: Vec<String>,
        submits: usize,
    }

    enum Cmd {
        Layers(Vec<u32>),
        Overlay { expect: Option<usize> },
    }

    impl QueueModel {
        fn execute(&mut self, enc: &mut Vec<Cmd>) {
            for cmd in enc.drain(..) {
                match cmd {
                    Cmd::Layers(l) => self.target.push(format!("layers{l:?}")),
                    Cmd::Overlay { expect } => {
                        if self.overlay != expect {
                            self.bad_reads.push(format!("{expect:?} read as {:?}", self.overlay));
                        }
                        self.target.push(format!("ops{}", expect.unwrap()));
                    }
                }
            }
        }
    }

    impl RunHost for QueueModel {
        type Enc = Vec<Cmd>;
        fn composite_layers(&mut self, enc: &mut Vec<Cmd>, which: &[u32]) {
            enc.push(Cmd::Layers(which.to_vec()));
        }
        fn render_overlay(&mut self, _ops: &[DrawOp]) -> bool {
            self.renders += 1;
            self.overlay = Some(self.renders);
            true
        }
        fn composite_overlay(&mut self, enc: &mut Vec<Cmd>) {
            enc.push(Cmd::Overlay { expect: self.overlay });
        }
        fn submit(&mut self, enc: &mut Vec<Cmd>) {
            self.submits += 1;
            self.execute(enc);
        }
    }

    fn fill(s: &mut Scene) {
        s.fill_path(Path::rect(0.0, 0.0, 1.0, 1.0), Color::new(255, 0, 0, 255));
    }

    /// Two textures with vector ops after each: both runs render into the one
    /// overlay texture, so the first run's composite must be submitted before
    /// the second run's vello render overwrites the overlay.
    #[test]
    fn regression_a_later_run_does_not_overwrite_an_unsubmitted_overlay_composite() {
        let mut s = Scene::new();
        s.texture(0);
        fill(&mut s);
        s.texture(1);
        fill(&mut s);
        let ops = canvas_core::place_textures(s, 2).into_ops();
        let segs = split_segments(&ops);

        let mut q = QueueModel::default();
        let mut enc = Vec::new();
        assert!(composite_texture_runs(&mut q, &segs.runs, &mut enc, false));
        q.submit(&mut enc); // the caller's final submit (with the present blit)
        assert!(q.bad_reads.is_empty(), "stale overlay reads: {:?}", q.bad_reads);
        assert_eq!(q.target, ["layers[0]", "ops1", "layers[1]", "ops2"]);
    }

    /// The base segment's Hybrid/Cached composite also reads the overlay.
    #[test]
    fn regression_a_run_does_not_overwrite_the_base_segments_overlay() {
        let mut s = Scene::new();
        s.texture(0);
        fill(&mut s);
        let ops = canvas_core::place_textures(s, 1).into_ops();
        let segs = split_segments(&ops);

        let mut q = QueueModel { overlay: Some(0), ..Default::default() };
        // The base pass recorded a composite of overlay content 0.
        let mut enc = vec![Cmd::Overlay { expect: Some(0) }];
        assert!(composite_texture_runs(&mut q, &segs.runs, &mut enc, true));
        q.submit(&mut enc);
        assert!(q.bad_reads.is_empty(), "stale overlay reads: {:?}", q.bad_reads);
        assert_eq!(q.target, ["ops0", "layers[0]", "ops1"]);
    }

    /// Layers composited after all vector content — every canvas before
    /// texture ops existed — cost no extra submit and no vello pass.
    #[test]
    fn appended_layers_cost_no_extra_submit_or_vello_pass() {
        let mut s = Scene::new();
        fill(&mut s);
        let ops = canvas_core::place_textures(s, 2).into_ops();
        let segs = split_segments(&ops);

        let mut q = QueueModel::default();
        let mut enc = Vec::new();
        assert!(composite_texture_runs(&mut q, &segs.runs, &mut enc, true));
        assert_eq!((q.submits, q.renders), (0, 0));
        q.submit(&mut enc);
        assert_eq!(q.target, ["layers[0, 1]"]);
    }
}

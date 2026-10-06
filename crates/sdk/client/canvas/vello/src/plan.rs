//! Scene classification shared by the native ([`render`](crate::render)) and web
//! ([`render_web`](crate::render_web)) vello renderers: decide, from a scene's
//! LEADING ops, whether the instanced [`ShapePass`](crate::shape_pass) can draw a
//! shape backdrop and let vello composite the rest over it.

use canvas_core::{BlendMode, DrawOp, ShapeInstance, Transform};

/// A borrowed view of one [`DrawOp::LayerCached`] in a [`ScenePlan::Cached`]
/// backdrop — what the renderer needs to bake (`dirty`/`ops`) and composite
/// (`id`/`transform`/`alpha`). Blend is always `Normal` here: the classifier
/// only forms a `Cached` backdrop from Normal-blend cached layers (a non-Normal
/// one needs vello's compositor, so it routes to [`ScenePlan::Vello`]).
#[derive(Debug)]
pub(crate) struct CachedRef<'a> {
    pub id: u32,
    pub dirty: bool,
    pub transform: &'a Transform,
    pub ops: &'a [DrawOp],
    pub alpha: f32,
}

/// How a renderer will draw a scene, decided by its LEADING ops. The instanced
/// `ShapePass` can own the frame's clear and draw a batch of shapes in one call,
/// but vello can't cheaply interleave a custom pass between its own draws — so the
/// pass is only used for shapes at the *start* of the scene (a backdrop), with
/// vello compositing everything after on top. Whatever path is taken, the pixels
/// match the all-vello expansion (CLAUDE.md §7): the instanced pass is an
/// optimization, never a behavioral fork.
#[derive(Debug)]
pub(crate) enum ScenePlan<'a> {
    /// No leading shape batch — vello renders the whole scene (the historical
    /// path). Scenes that open with any non-shape op (or a non-Normal blend
    /// shape batch) land here; trailing shape batches expand to fills in
    /// `encode_scene`.
    Vello,
    /// Every op is a Normal-blend [`DrawOp::Shapes`] batch: the instanced pass
    /// owns the whole frame (clear + draw) and vello isn't run. An empty op list
    /// is `Shapes(vec![])`, which the pass renders as a transparent clear.
    Shapes(Vec<&'a [ShapeInstance]>),
    /// A leading run of Normal-blend `Shapes` batches (the `prefix` backdrop)
    /// followed by other ops (`rest`): the instanced pass draws the backdrop into
    /// the target, vello renders `rest` into a separate target over a transparent
    /// base, and [`compose`](crate::compose) lays that content over the backdrop.
    /// The backdrop is GPU-instanced while everything else stays exact vello, all
    /// in the one canvas that's displayed AND self-captured (recording unaffected).
    Hybrid { prefix: Vec<&'a [ShapeInstance]>, rest: &'a [DrawOp] },
    /// A leading run of Normal-blend [`DrawOp::LayerCached`] ops (the `layers`
    /// backdrop) followed by other ops (`rest`): the renderer bakes each dirty
    /// layer to a viewport-sized texture once, composites those textures under
    /// their camera transforms (a transformed quad each — `O(1)` per layer
    /// regardless of op count), then renders `rest` (the live ink) through vello
    /// over the top via [`compose`](crate::compose). This is the infinite
    /// pan/zoom fast path; the [`encode_scene`](crate::encode) fallback produces
    /// the same pixels by re-rasterizing (CLAUDE.md §7).
    Cached { layers: Vec<CachedRef<'a>>, rest: &'a [DrawOp] },
}

/// Split `ops` into the longest leading run of Normal-blend [`DrawOp::Shapes`]
/// batches and whatever follows, classifying the scene.
pub(crate) fn plan_scene(ops: &[DrawOp]) -> ScenePlan<'_> {
    // A scene that LEADS with a cached layer is an infinite-canvas frame: bake +
    // transform-composite the leading Normal-blend cached-layer run, vello over
    // the rest. Distinct from (and checked before) the shape-backdrop path —
    // they're mutually exclusive by first op. A scene whose first cached layer
    // uses a non-Normal blend isn't routed here (it needs vello's compositor),
    // so it falls through to the shape/Vello classification below, where the
    // `encode_scene` fallback handles it correctly.
    if matches!(ops.first(), Some(DrawOp::LayerCached { blend: BlendMode::Normal, .. })) {
        let mut layers: Vec<CachedRef<'_>> = Vec::new();
        let mut i = 0;
        while let Some(DrawOp::LayerCached { id, dirty, transform, ops: nested, alpha, blend }) =
            ops.get(i)
        {
            if *blend != BlendMode::Normal {
                break;
            }
            layers.push(CachedRef {
                id: *id,
                dirty: *dirty,
                transform,
                ops: nested,
                alpha: *alpha,
            });
            i += 1;
        }
        return ScenePlan::Cached { layers, rest: &ops[i..] };
    }

    let mut prefix: Vec<&[ShapeInstance]> = Vec::new();
    let mut i = 0;
    while let Some(DrawOp::Shapes { shapes, blend }) = ops.get(i) {
        if *blend != BlendMode::Normal {
            break;
        }
        prefix.push(shapes.as_slice());
        i += 1;
    }
    if i == ops.len() {
        // Every op (possibly none) was a Normal shape batch: the instanced pass
        // owns the whole frame, vello isn't run. An empty list is `Shapes(vec![])`
        // — a transparent clear, the same cheap path the old fast path took.
        ScenePlan::Shapes(prefix)
    } else if prefix.is_empty() {
        ScenePlan::Vello
    } else {
        ScenePlan::Hybrid { prefix, rest: &ops[i..] }
    }
}

/// One composite step after the base pass: the texture layers to composite
/// (indices into `CanvasProps::layers`, in order), then the vector ops that draw
/// ON TOP of them up to the next texture (empty when nothing is drawn there).
#[derive(Debug, PartialEq)]
pub(crate) struct TextureRun<'a> {
    pub layers: Vec<u32>,
    pub ops: &'a [DrawOp],
}

/// A scene's top-level op list split at its [`DrawOp::Texture`] ops (see
/// [`split_segments`]).
#[derive(Debug, PartialEq)]
pub(crate) struct Segments<'a> {
    /// The ops before the first texture — rendered exactly like a texture-less
    /// scene (classified by [`plan_scene`], so the `Cached`/`Hybrid`/`Shapes`
    /// fast paths apply). Empty when nothing is drawn there.
    pub base: &'a [DrawOp],
    /// One entry per run of textures, in draw order. Empty (no allocation) for a
    /// scene without textures.
    pub runs: Vec<TextureRun<'a>>,
}

/// Split a scene's top-level ops at its `Texture` ops.
///
/// vello can't draw a native camera texture inside its own scene, so a GPU
/// renderer draws a textured scene as alternating passes: vello renders a run
/// of vector ops, the layer compositor draws the texture over it, vello renders
/// the next run into a separate target that is composited on top, and so on.
/// That only works because `canvas_core::place_textures` (run by `paint_scene`)
/// leaves every top-level `Texture` op at the base state with each run between
/// textures self-contained (balanced saves, its own transform/clip) — so each
/// run can be encoded on its own.
///
/// Each segment is also trimmed:
/// - a segment that only manipulates state (`Save`/`Restore`/`Transform`/`Clip`)
///   draws nothing and becomes empty, so textures separated only by
///   `place_textures`' re-open/close bookkeeping merge into one run with no
///   vello pass between them;
/// - an outer `Save … Restore` pair wrapping the whole segment is dropped. It
///   changes nothing (the state is discarded at the segment end), but
///   `place_textures` wraps the scene in one, and a leading `Save` would hide the
///   segment's leading `LayerCached` / `Shapes` ops from [`plan_scene`] and lose
///   the fast paths whenever the canvas has a layer.
///
/// Pure slicing: a scene without `Texture` ops costs one scan of its top-level
/// ops and no allocation.
pub(crate) fn split_segments(ops: &[DrawOp]) -> Segments<'_> {
    let is_texture = |op: &DrawOp| matches!(op, DrawOp::Texture { .. });
    let Some(first) = ops.iter().position(is_texture) else {
        return Segments { base: trim_segment(ops), runs: Vec::new() };
    };
    let base = trim_segment(&ops[..first]);
    let mut runs: Vec<TextureRun<'_>> = Vec::new();
    let mut i = first;
    while i < ops.len() {
        let DrawOp::Texture { index } = ops[i] else {
            unreachable!("loop invariant: `i` is at a Texture op")
        };
        let end = ops[i + 1..].iter().position(is_texture).map_or(ops.len(), |p| i + 1 + p);
        let seg = trim_segment(&ops[i + 1..end]);
        // Textures with nothing drawn between them composite as one run.
        match runs.last_mut() {
            Some(run) if run.ops.is_empty() => run.layers.push(index),
            _ => runs.push(TextureRun { layers: vec![index], ops: &[] }),
        }
        runs.last_mut().expect("just pushed").ops = seg;
        i = end;
    }
    Segments { base, runs }
}

/// Drop what a self-contained segment doesn't need (see [`split_segments`]).
fn trim_segment(mut ops: &[DrawOp]) -> &[DrawOp] {
    let state_only = |op: &DrawOp| {
        matches!(op, DrawOp::Save | DrawOp::Restore | DrawOp::Transform(_) | DrawOp::Clip { .. })
    };
    if ops.iter().all(state_only) {
        return &[];
    }
    while ops.len() >= 2
        && matches!(ops.first(), Some(DrawOp::Save))
        && matching_restore(ops) == Some(ops.len() - 1)
    {
        ops = &ops[1..ops.len() - 1];
    }
    ops
}

/// Index of the `Restore` that closes the `Save` at `ops[0]`.
fn matching_restore(ops: &[DrawOp]) -> Option<usize> {
    let mut depth = 0usize;
    for (i, op) in ops.iter().enumerate() {
        match op {
            DrawOp::Save => depth += 1,
            DrawOp::Restore => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{Color, Paint, Path, Scene};

    fn red_fill(s: &mut Scene) {
        s.fill_path(Path::rect(0.0, 0.0, 1.0, 1.0), Color::new(255, 0, 0, 255));
    }

    fn painted(layers: usize, f: impl Fn(&mut Scene)) -> Vec<DrawOp> {
        let mut s = Scene::new();
        f(&mut s);
        canvas_core::place_textures(s, layers).into_ops()
    }

    #[test]
    fn a_scene_without_textures_is_one_base_segment() {
        let ops = painted(0, red_fill);
        let segs = split_segments(&ops);
        assert_eq!(segs.base, &ops[..]);
        assert!(segs.runs.is_empty());
    }

    /// Unplaced layers are appended by `place_textures` with only save/restore
    /// bookkeeping between them: they must composite as ONE run with no vello
    /// pass after it — the pre-texture-op cost of a layered canvas.
    #[test]
    fn appended_layers_composite_as_one_run_with_no_pass_after() {
        let ops = painted(2, red_fill);
        let segs = split_segments(&ops);
        assert_eq!(segs.base.len(), 1, "outer save/restore trimmed: {:?}", segs.base);
        assert!(matches!(segs.base[0], DrawOp::Fill { .. }));
        assert_eq!(segs.runs, vec![TextureRun { layers: vec![0, 1], ops: &[] }]);
    }

    /// The trimmed base must keep the `Cached` fast path when the canvas has a
    /// layer (`place_textures` wraps the scene in a leading `Save`).
    #[test]
    fn a_layered_canvas_keeps_the_cached_fast_path() {
        let ops = painted(1, |s| {
            s.layer_cached(1, true, Transform::IDENTITY, red_fill);
            red_fill(s);
        });
        let segs = split_segments(&ops);
        assert!(matches!(plan_scene(segs.base), ScenePlan::Cached { .. }), "{:?}", segs.base);
    }

    #[test]
    fn ops_after_a_texture_form_a_run_drawn_over_it() {
        let ops = painted(2, |s| {
            red_fill(s);
            s.texture(1);
            s.transform(Transform::translate(3.0, 0.0));
            red_fill(s);
            s.texture(0);
            red_fill(s);
        });
        let segs = split_segments(&ops);
        assert_eq!(segs.base.len(), 1);
        assert_eq!(segs.runs.len(), 2);
        assert_eq!(segs.runs[0].layers, vec![1]);
        // The run re-establishes the author's transform itself (self-contained).
        assert!(matches!(segs.runs[0].ops[0], DrawOp::Transform(_)), "{:?}", segs.runs[0].ops);
        assert!(matches!(segs.runs[0].ops.last(), Some(DrawOp::Fill { .. })));
        assert_eq!(segs.runs[1].layers, vec![0]);
        assert!(matches!(segs.runs[1].ops.last(), Some(DrawOp::Fill { .. })));
    }

    #[test]
    fn a_texture_first_scene_has_an_empty_base() {
        let ops = painted(1, |s| {
            s.texture(0);
            red_fill(s);
        });
        let segs = split_segments(&ops);
        assert!(segs.base.is_empty());
        assert_eq!(segs.runs.len(), 1);
        assert_eq!(segs.runs[0].ops.len(), 1);
    }

    /// A `Save` whose `Restore` isn't the segment's last op isn't a wrapper.
    #[test]
    fn a_non_wrapping_save_is_kept() {
        let mut s = Scene::new();
        s.save();
        red_fill(&mut s);
        s.restore();
        red_fill(&mut s);
        let ops = s.into_ops();
        assert_eq!(split_segments(&ops).base.len(), 4);
    }

    #[test]
    fn plan_scene_classifies_leading_cached_layers() {
        let red = Color::new(255, 0, 0, 255);

        // Leading Normal cached layer + trailing ink → Cached { layers:[..], rest }.
        let mut s = Scene::new();
        s.layer_cached(1, true, Transform::translate(5.0, 0.0), |l| {
            l.path().add_path(Path::rect(0.0, 0.0, 4.0, 4.0));
            l.fill(red);
        });
        s.path().add_path(Path::rect(8.0, 8.0, 2.0, 2.0));
        s.fill(Paint::solid(red));
        match plan_scene(s.ops()) {
            ScenePlan::Cached { layers, rest } => {
                assert_eq!(layers.len(), 1);
                assert_eq!(layers[0].id, 1);
                assert!(layers[0].dirty);
                assert_eq!(*layers[0].transform, Transform::translate(5.0, 0.0));
                assert_eq!(rest.len(), 1, "trailing ink is the `rest`");
            }
            other => panic!("expected Cached, got {other:?}"),
        }

        // Two leading cached layers, no rest → Cached with empty rest.
        let mut s2 = Scene::new();
        s2.layer_cached(1, true, Transform::IDENTITY, |l| {
            l.fill_path(Path::rect(0.0, 0.0, 1.0, 1.0), red);
        });
        s2.layer_cached(2, false, Transform::translate(1.0, 1.0), |_| {});
        match plan_scene(s2.ops()) {
            ScenePlan::Cached { layers, rest } => {
                assert_eq!(layers.len(), 2);
                assert!(!layers[1].dirty, "second layer reuses its raster");
                assert!(rest.is_empty());
            }
            other => panic!("expected Cached, got {other:?}"),
        }

        // A non-Normal-blend cached layer is NOT routed to the fast path (needs
        // vello's compositor) → Vello, where encode_scene handles it correctly.
        let mut s3 = Scene::new();
        s3.layer_cached_with(1, true, Transform::IDENTITY, 1.0, BlendMode::Multiply, |l| {
            l.fill_path(Path::rect(0.0, 0.0, 1.0, 1.0), red);
        });
        assert!(matches!(plan_scene(s3.ops()), ScenePlan::Vello));

        // A cached layer that does NOT lead (ink first) → Vello fallback.
        let mut s4 = Scene::new();
        s4.fill_path(Path::rect(0.0, 0.0, 1.0, 1.0), red);
        s4.layer_cached(1, true, Transform::IDENTITY, |l| {
            l.fill_path(Path::rect(0.0, 0.0, 1.0, 1.0), red);
        });
        assert!(matches!(plan_scene(s4.ops()), ScenePlan::Vello));
    }
}

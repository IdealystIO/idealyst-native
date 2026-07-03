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

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{Color, Paint, Path, Scene};

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

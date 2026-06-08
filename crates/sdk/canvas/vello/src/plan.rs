//! Scene classification shared by the native ([`render`](crate::render)) and web
//! ([`render_web`](crate::render_web)) vello renderers: decide, from a scene's
//! LEADING ops, whether the instanced [`ShapePass`](crate::shape_pass) can draw a
//! shape backdrop and let vello composite the rest over it.

use canvas_core::{BlendMode, DrawOp, ShapeInstance};

/// How a renderer will draw a scene, decided by its LEADING ops. The instanced
/// `ShapePass` can own the frame's clear and draw a batch of shapes in one call,
/// but vello can't cheaply interleave a custom pass between its own draws — so the
/// pass is only used for shapes at the *start* of the scene (a backdrop), with
/// vello compositing everything after on top. Whatever path is taken, the pixels
/// match the all-vello expansion (CLAUDE.md §7): the instanced pass is an
/// optimization, never a behavioral fork.
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
}

/// Split `ops` into the longest leading run of Normal-blend [`DrawOp::Shapes`]
/// batches and whatever follows, classifying the scene.
pub(crate) fn plan_scene(ops: &[DrawOp]) -> ScenePlan<'_> {
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

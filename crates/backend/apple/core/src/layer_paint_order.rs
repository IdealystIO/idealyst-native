//! Make a software rasterization of a CALayer tree paint like the screen.
//!
//! Core Animation's compositor (the window server on macOS, the render
//! server on iOS) sorts each layer's sublayers by `zPosition` (ties in
//! tree order) and lets a `CATransformLayer`'s sublayers sort together
//! with the transform layer's own siblings — which is how the Apple
//! backends realise `StyleRules::z_index` and the `display: contents`
//! reactive anchor. `-[CALayer renderInContext:]` does neither: it draws
//! sublayers in array order and draws a transform layer's sublayers as one
//! unit. So a snapshot taken through it (AppKit's `cacheDisplayInRect:`,
//! the macOS robot screenshot) showed a `z_index`-raised sibling UNDER the
//! later sibling it covers on screen.
//!
//! [`with_layers_in_paint_order`] closes that gap for the duration of one
//! synchronous rasterization: it rewrites each layer's `sublayers` into
//! the compositor's order — transform layers folded into their parent,
//! then a stable sort by `zPosition` ([`runtime_shared::style::z_paint_order`],
//! the order every backend shares) — runs the capture, and restores every
//! array it touched before returning. Layers whose order already matches
//! are left alone, so a tree with no `z_index` and no anchors is never
//! mutated. Implicit animations are suppressed throughout, and the whole
//! rewrite + restore happens inside one main-thread call, so the screen
//! never sees the intermediate tree.

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGRect};

use crate::implicit_animations::NoImplicitAnimations;

fn sublayers(layer: &NSObject) -> Vec<Retained<NSObject>> {
    unsafe {
        let arr: *mut NSObject = msg_send![layer, sublayers];
        if arr.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![arr, count];
        (0..count)
            .filter_map(|i| {
                let sub: *mut NSObject = msg_send![arr, objectAtIndex: i];
                Retained::retain(sub)
            })
            .collect()
    }
}

fn set_sublayers(layer: &NSObject, subs: &[Retained<NSObject>]) {
    unsafe {
        let arr: Retained<NSObject> = msg_send_id![class!(NSMutableArray), array];
        for s in subs {
            let _: () = msg_send![&arr, addObject: &**s];
        }
        let _: () = msg_send![layer, setSublayers: &*arr];
    }
}

fn z_of(layer: &NSObject) -> f32 {
    let z: CGFloat = unsafe { msg_send![layer, zPosition] };
    z as f32
}

/// A transform layer whose sublayers can be lifted into its parent without
/// moving them: frame and bounds at the origin, as the backends' anchors
/// are (`LayoutTree::frame_of` gives a contents node its ancestor's box at
/// `(0, 0)`). Any other transform layer is rasterized as a unit.
fn is_foldable_transform_layer(layer: &NSObject) -> bool {
    let is_transform: bool =
        unsafe { msg_send![layer, isKindOfClass: class!(CATransformLayer)] };
    if !is_transform {
        return false;
    }
    let frame: CGRect = unsafe { msg_send![layer, frame] };
    let bounds: CGRect = unsafe { msg_send![layer, bounds] };
    frame.origin.x == 0.0
        && frame.origin.y == 0.0
        && bounds.origin.x == 0.0
        && bounds.origin.y == 0.0
}

/// The layers that sort together directly under `layer`, in tree order,
/// with foldable transform layers replaced by their own (recursively).
/// `folded` collects the transform layers that were folded.
fn sort_group(
    layer: &NSObject,
    out: &mut Vec<Retained<NSObject>>,
    folded: &mut Vec<Retained<NSObject>>,
) {
    for sub in sublayers(layer) {
        if is_foldable_transform_layer(&sub) {
            sort_group(&sub, out, folded);
            folded.push(sub);
        } else {
            out.push(sub);
        }
    }
}

type Undo = Vec<(Retained<NSObject>, Vec<Retained<NSObject>>)>;

fn reorder(layer: &NSObject, undo: &mut Undo) {
    let current = sublayers(layer);
    let mut group = Vec::new();
    let mut folded = Vec::new();
    sort_group(layer, &mut group, &mut folded);
    let zs: Vec<f32> = group.iter().map(|l| z_of(l)).collect();
    let order = runtime_shared::style::z_paint_order(&zs);
    let identity = order.iter().enumerate().all(|(i, &j)| i == j);
    // An unsorted group needs no rewrite when nothing was folded either:
    // `renderInContext:` already draws it in this order. With folded
    // transform layers and an identity sort, drawing each transform layer
    // as a unit is ALSO this order, so only a real reorder rewrites.
    if !identity {
        for t in &folded {
            undo.push((t.clone(), sublayers(t)));
        }
        undo.push((unsafe { Retained::retain(layer as *const NSObject as *mut NSObject) }
            .expect("non-null layer"), current));
        let sorted: Vec<Retained<NSObject>> = order.iter().map(|&i| group[i].clone()).collect();
        set_sublayers(layer, &sorted);
    }
    for child in &group {
        reorder(child, undo);
    }
}

/// Run `capture` (a synchronous `renderInContext:` / `cacheDisplayInRect:`
/// over `root`) with every layer under `root` temporarily in compositor
/// paint order; see the module docs. Every rewritten `sublayers` array is
/// restored before this returns.
pub fn with_layers_in_paint_order<R>(root: &NSObject, capture: impl FnOnce() -> R) -> R {
    with_layer_trees_in_paint_order(&[root], capture)
}

/// [`with_layers_in_paint_order`] over several disjoint layer trees at
/// once — the layer-backed subtrees under a view that is not layer-backed
/// itself (AppKit's host content view).
pub fn with_layer_trees_in_paint_order<R>(roots: &[&NSObject], capture: impl FnOnce() -> R) -> R {
    let _no_anim = NoImplicitAnimations::begin();
    let mut undo: Undo = Vec::new();
    for root in roots {
        reorder(root, &mut undo);
    }
    let result = capture();
    // Reverse order: a parent is restored (re-adopting its folded
    // transform layers, emptied) before those transform layers get their
    // own sublayers back.
    for (layer, original) in undo.into_iter().rev() {
        set_sublayers(&layer, &original);
    }
    result
}

//! The CALayer half of dashed / dotted borders, shared verbatim by iOS and
//! macOS.
//!
//! `CALayer.borderWidth`/`borderColor` strokes one unbroken line and has no
//! dash pattern, so a [`BorderStyle::Dashed`] / [`BorderStyle::Dotted`] border
//! is stroked by `CAShapeLayer`s instead. Which pieces get stroked is the pure
//! [`crate::border::route_border`] decision; every length, gap and path point
//! comes from [`crate::border::loop_stroke`] / [`crate::border::side_stroke`],
//! which in turn take their geometry from `runtime_shared::border_dash` — the
//! one source of dash proportions every backend shares (Rule #7). This file
//! only replays those plans onto Core Animation.
//!
//! Like [`crate::shadow_layer`], every routine takes the view's CALayer (the
//! same class under UIKit and AppKit); the backends contribute the view → layer
//! lookup and the `Color → CGColor` conversion.
//!
//! ## Layer shape
//!
//! One plain `CALayer` "host" (named [`HOST_NAME`], held on the view's layer
//! under the KVC key [`HOST_KEY`]) with one `CAShapeLayer` child per stroked
//! piece: a single closed loop for a uniform border, or one open line per side
//! with a width for an asymmetric one. A `CAShapeLayer` has ONE stroke colour,
//! width and dash pattern, which is why the per-side case needs a child each.
//! The host's frame tracks the view's `bounds` and the children's paths are in
//! the host's (0, 0)-origin space, so nothing is recomputed per child when the
//! bounds origin moves.
//!
//! Everything a re-trace needs is stored ON the layers — the style and the
//! layer's y-direction on the host (KVC), each piece's role in its `name`, its
//! width in `lineWidth`, its colour in `strokeColor` — so [`sync`] needs no
//! Rust-side cache and the layout pass can call it blindly per view.
//!
//! ## Z-order: above the background, below the children
//!
//! The host sits directly above the view's `idealyst_gradient` sublayer when
//! there is one, else at sublayer index 0. That is CSS's paint order — a
//! border paints over the element's background (colour AND image) and under
//! its children — so an absolutely positioned child that overlaps the border
//! covers it exactly as it does on web. Index 0 of the view's own layer is
//! below every child view's layer (UIKit/AppKit append subview layers), and
//! above `backgroundColor`, which a layer paints beneath all sublayers.
//!
//! This deliberately differs from `CALayer.borderWidth`, which Core Animation
//! composites ABOVE the layer's sublayers. The solid path keeps its CALayer
//! stroke (it has no ordering knob); the difference is only visible where a
//! child overlaps the border band, which in-flow children never do (Taffy
//! insets the content box by the border width).
//!
//! The per-side SOLID bars are subviews appended on top of the children; the
//! dashed per-side lines take the CSS order above instead, for the same reason.
//!
//! [`place_host`] re-checks the position on every sync, because both toolkits
//! rewrite `sublayers` as subviews come and go, and a gradient re-applied
//! later is inserted at index 0 (below the host — correct), while a host
//! inserted before a gradient existed would otherwise end up beneath it.
//!
//! ## Hit-testing and clipping
//!
//! Sublayers never take part in `UIView.hitTest` / `NSView.hitTest:` — both
//! walk the view tree, not the layer tree — so the host cannot swallow a tap.
//! The stroke lies inside the bounds (its centreline is inset half a width), so
//! `masksToBounds` (`overflow: hidden`) clips nothing of it — the same band a
//! CALayer border paints in.
//!
//! ## Animation
//!
//! Neither Apple backend animates `border_*_color` / `border_*_width` today:
//! the style path is the only writer of `borderColor`/`borderWidth` (no
//! transition driver or `AnimProp` touches them). So there is no animated
//! write that could resurrect a solid stroke under a dashed one. The host and
//! its children are backend-created layers with no view delegate, so Core
//! Animation would implicitly ease every `path` / `strokeColor` change over
//! ~0.25 s; every mutation here runs under [`NoImplicitAnimations`] so a
//! dashed border restyles instantly, exactly like the solid CALayer stroke of
//! a view-backed layer does. If border-colour transitions are ever added, they
//! must write the pieces' `strokeColor` for a patterned border.
//!
//! [`BorderStyle::Dashed`]: runtime_shared::BorderStyle::Dashed
//! [`BorderStyle::Dotted`]: runtime_shared::BorderStyle::Dotted

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGRect, NSString};
use runtime_shared::BorderStyle;

use crate::border::{loop_stroke, side_stroke, DashStroke, PathOp};
use crate::cg::{CGColorRef, CGPathRef};
use crate::implicit_animations::NoImplicitAnimations;

/// KVC key on the view's own layer holding a strong reference to the host
/// layer. CALayer supports arbitrary KVC keys and retains the value.
pub const HOST_KEY: &str = "idealyst_border_dash";

/// `CALayer.name` of the host — for layer dumps, and for [`place_host`]'s
/// sibling walk. Lookup goes through [`HOST_KEY`].
pub const HOST_NAME: &str = "idealyst_border_dash";

/// Host KVC key: the pattern (`NSNumber`, see [`style_code`]).
const STYLE_KEY: &str = "idealyst_border_dash_style";
/// Host KVC key: whether the view's layer is y-down (`NSNumber` bool).
const Y_DOWN_KEY: &str = "idealyst_border_dash_y_down";

/// Name of the view's gradient sublayer, which the host must sit above.
const GRADIENT_NAME: &str = "idealyst_gradient";

/// `CAShapeLayer.name` for each piece. The loop is the uniform border; the
/// four sides are the asymmetric one's per-side lines.
const LOOP_NAME: &str = "idealyst_border_dash_loop";
const SIDE_NAMES: [&str; 4] = [
    "idealyst_border_dash_top",
    "idealyst_border_dash_right",
    "idealyst_border_dash_bottom",
    "idealyst_border_dash_left",
];

/// `kCALineCapRound` / `kCALineCapButt`. The constants' documented string
/// values; `lineCap` compares by string equality.
const CAP_ROUND: &str = "round";
const CAP_BUTT: &str = "butt";

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPathCreateMutable() -> CGPathRef;
    fn CGPathMoveToPoint(path: CGPathRef, m: *const std::ffi::c_void, x: CGFloat, y: CGFloat);
    fn CGPathAddLineToPoint(path: CGPathRef, m: *const std::ffi::c_void, x: CGFloat, y: CGFloat);
    fn CGPathAddArcToPoint(
        path: CGPathRef,
        m: *const std::ffi::c_void,
        x1: CGFloat,
        y1: CGFloat,
        x2: CGFloat,
        y2: CGFloat,
        radius: CGFloat,
    );
    fn CGPathCloseSubpath(path: CGPathRef);
    fn CGPathRelease(path: CGPathRef);
}

/// One stroked piece of a patterned border.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Piece {
    /// The whole uniform border as one closed loop.
    Loop,
    /// One side of an asymmetric border, `0..4` = top, right, bottom, left.
    Side(usize),
}

impl Piece {
    fn name(self) -> &'static str {
        match self {
            Piece::Loop => LOOP_NAME,
            Piece::Side(i) => SIDE_NAMES[i.min(3)],
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        if name == LOOP_NAME {
            return Some(Piece::Loop);
        }
        SIDE_NAMES.iter().position(|n| *n == name).map(Piece::Side)
    }
}

/// What introspection reads back off a patterned border: the stroke Core
/// Animation will actually draw.
pub struct Readback {
    pub width: CGFloat,
    pub color: CGColorRef,
    pub style: BorderStyle,
}

fn style_code(style: BorderStyle) -> i64 {
    match style {
        BorderStyle::Solid => 0,
        BorderStyle::Dashed => 1,
        BorderStyle::Dotted => 2,
    }
}

fn style_from_code(code: i64) -> BorderStyle {
    match code {
        1 => BorderStyle::Dashed,
        2 => BorderStyle::Dotted,
        _ => BorderStyle::Solid,
    }
}

fn kvc_get(obj: &NSObject, key: &str) -> *mut NSObject {
    let key = NSString::from_str(key);
    unsafe { msg_send![obj, valueForKey: &*key] }
}

fn kvc_set(obj: &NSObject, key: &str, value: Option<&NSObject>) {
    let key = NSString::from_str(key);
    let ptr: *const NSObject = value.map_or(std::ptr::null(), |v| v as *const NSObject);
    unsafe {
        let _: () = msg_send![obj, setValue: ptr, forKey: &*key];
    }
}

fn layer_name(layer: *mut NSObject) -> Option<String> {
    if layer.is_null() {
        return None;
    }
    let name: *mut NSString = unsafe { msg_send![layer, name] };
    (!name.is_null()).then(|| unsafe { &*name }.to_string())
}

/// The host layer for `layer`, if a patterned border is installed.
pub fn host(layer: &NSObject) -> Option<Retained<NSObject>> {
    let ptr = kvc_get(layer, HOST_KEY);
    if ptr.is_null() {
        None
    } else {
        unsafe { Retained::retain(ptr) }
    }
}

/// The host's `CAShapeLayer` children, snapshotted.
///
/// Snapshot, not a live walk: `[CALayer sublayers]` is a live `CALayerArray`
/// proxy and [`install`] removes children while deciding what to keep — the
/// mutate-while-indexing abort `gradient::remove_existing` documents.
fn pieces_of(host: &NSObject) -> Vec<(Retained<NSObject>, Option<Piece>)> {
    let subs: *mut NSObject = unsafe { msg_send![host, sublayers] };
    if subs.is_null() {
        return Vec::new();
    }
    let count: usize = unsafe { msg_send![subs, count] };
    (0..count)
        .filter_map(|i| {
            let ptr: *mut NSObject = unsafe { msg_send![subs, objectAtIndex: i] };
            let piece = layer_name(ptr).as_deref().and_then(Piece::from_name);
            unsafe { Retained::retain(ptr) }.map(|l| (l, piece))
        })
        .collect()
}

/// Install (or update in place) a patterned border on `layer`.
///
/// `pieces` is every piece to stroke with its width and colour; any piece a
/// previous apply installed that is not listed is removed, so a uniform ↔
/// asymmetric flip or a side losing its width leaves no ghost line. `y_down`
/// is the layer's y-direction: always true under UIKit, `isFlipped` on AppKit.
///
/// The caller is responsible for zeroing `borderWidth` and removing any solid
/// per-side bars — this module only owns its own layers.
pub fn install(
    layer: &NSObject,
    style: BorderStyle,
    pieces: &[(Piece, f32, CGColorRef)],
    y_down: bool,
) {
    let _no_anim = NoImplicitAnimations::begin();
    let host = match host(layer) {
        Some(h) => h,
        None => {
            let h: Retained<NSObject> = unsafe { msg_send_id![class!(CALayer), layer] };
            unsafe {
                let name = NSString::from_str(HOST_NAME);
                let _: () = msg_send![&*h, setName: &*name];
            }
            kvc_set(layer, HOST_KEY, Some(&h));
            h
        }
    };
    unsafe {
        let code: Retained<NSObject> =
            msg_send_id![class!(NSNumber), numberWithLongLong: style_code(style)];
        kvc_set(&host, STYLE_KEY, Some(&code));
        let flag: Retained<NSObject> = msg_send_id![class!(NSNumber), numberWithBool: y_down];
        kvc_set(&host, Y_DOWN_KEY, Some(&flag));
    }

    let existing = pieces_of(&host);
    for (child, piece) in &existing {
        let wanted = piece.is_some_and(|p| pieces.iter().any(|(q, _, _)| *q == p));
        if !wanted {
            let _: () = unsafe { msg_send![&**child, removeFromSuperlayer] };
        }
    }
    for &(piece, width, color) in pieces {
        let child = existing
            .iter()
            .find(|(_, p)| *p == Some(piece))
            .map(|(c, _)| c.clone())
            .unwrap_or_else(|| {
                let c: Retained<NSObject> = unsafe { msg_send_id![class!(CAShapeLayer), layer] };
                unsafe {
                    let name = NSString::from_str(piece.name());
                    let _: () = msg_send![&*c, setName: &*name];
                    // CAShapeLayer's default `fillColor` is OPAQUE BLACK: left
                    // alone, the closed loop would paint a black slab over the
                    // view's whole background.
                    let _: () = msg_send![&*c, setFillColor: CGColorRef(std::ptr::null())];
                    let _: () = msg_send![&*host, addSublayer: &*c];
                }
                c
            });
        unsafe {
            let _: () = msg_send![&*child, setLineWidth: width as CGFloat];
            let _: () = msg_send![&*child, setStrokeColor: color];
        }
    }
    drop(_no_anim);
    sync(layer);
}

/// Remove the patterned border entirely — a solid border, or none, is now
/// wanted. No-op when none is installed, so every style apply calls it.
pub fn remove(layer: &NSObject) {
    if let Some(h) = host(layer) {
        let _: () = unsafe { msg_send![&*h, removeFromSuperlayer] };
        kvc_set(layer, HOST_KEY, None);
    }
}

/// Re-trace every piece against the layer's current `bounds` and
/// `cornerRadius`, and keep the host in its paint-order slot.
///
/// Call from the post-layout hook AFTER the corner radius has been clamped
/// (the same slot as `shadow_layer::sync_own_shadow_path`), so a percent /
/// `Length::Full` radius — which only resolves once Taffy has sized the view —
/// is traced at its final value. The dash fitting depends on the perimeter, so
/// the pattern is re-fitted here too, not just the path. No-op for a view with
/// no patterned border.
pub fn sync(layer: &NSObject) {
    let Some(host) = host(layer) else { return };
    let _no_anim = NoImplicitAnimations::begin();
    place_host(layer, &host);

    let bounds: CGRect = unsafe { msg_send![layer, bounds] };
    let (w, h) = (bounds.size.width as f32, bounds.size.height as f32);
    if !(w > 0.0 && h > 0.0) {
        // Pre-layout: nothing to trace, and a stale path would paint at the
        // old size. The layout pass calls again once there is a frame.
        let _: () = unsafe { msg_send![&*host, setHidden: true] };
        return;
    }
    unsafe {
        let _: () = msg_send![&*host, setFrame: bounds];
        let _: () = msg_send![&*host, setHidden: false];
    }
    let radius: CGFloat = unsafe { msg_send![layer, cornerRadius] };
    let style = {
        let n = kvc_get(&host, STYLE_KEY);
        if n.is_null() {
            BorderStyle::Solid
        } else {
            style_from_code(unsafe { msg_send![n, longLongValue] })
        }
    };
    let y_down = {
        let n = kvc_get(&host, Y_DOWN_KEY);
        n.is_null() || unsafe { msg_send![n, boolValue] }
    };

    for (child, piece) in pieces_of(&host) {
        let Some(piece) = piece else { continue };
        let width: CGFloat = unsafe { msg_send![&*child, lineWidth] };
        let plan = match piece {
            Piece::Loop => loop_stroke(style, w, h, radius as f32, width as f32, y_down),
            Piece::Side(side) => side_stroke(style, w, h, side, width as f32, y_down),
        };
        match plan {
            Some(plan) => apply_plan(&child, &plan),
            None => unsafe {
                let _: () = msg_send![&*child, setPath: CGPathRef(std::ptr::null())];
            },
        }
    }
}

/// Write one [`DashStroke`] onto a `CAShapeLayer`.
fn apply_plan(shape: &NSObject, plan: &DashStroke) {
    // Create-rule: we own the +1; `setPath:` retains its own reference.
    let path = unsafe { CGPathCreateMutable() };
    if path.0.is_null() {
        return;
    }
    let m = std::ptr::null();
    for op in &plan.ops {
        unsafe {
            match *op {
                PathOp::MoveTo(x, y) => CGPathMoveToPoint(path, m, x as CGFloat, y as CGFloat),
                PathOp::LineTo(x, y) => CGPathAddLineToPoint(path, m, x as CGFloat, y as CGFloat),
                PathOp::ArcTo { x1, y1, x2, y2, r } => CGPathAddArcToPoint(
                    path,
                    m,
                    x1 as CGFloat,
                    y1 as CGFloat,
                    x2 as CGFloat,
                    y2 as CGFloat,
                    r as CGFloat,
                ),
                PathOp::Close => CGPathCloseSubpath(path),
            }
        }
    }
    unsafe {
        let _: () = msg_send![shape, setPath: path];
        CGPathRelease(path);

        let arr: Retained<NSObject> = msg_send_id![class!(NSMutableArray), array];
        for v in plan.dash {
            let n: Retained<NSObject> = msg_send_id![class!(NSNumber), numberWithDouble: v as f64];
            let _: () = msg_send![&*arr, addObject: &*n];
        }
        let _: () = msg_send![shape, setLineDashPattern: &*arr];
        let _: () = msg_send![shape, setLineDashPhase: plan.phase as CGFloat];
        let cap = NSString::from_str(if plan.round_cap { CAP_ROUND } else { CAP_BUTT });
        let _: () = msg_send![shape, setLineCap: &*cap];
    }
}

/// Keep the host directly above the gradient sublayer (or at index 0 when
/// there is none) — see the module's z-order section.
///
/// Read-only walk first, and only a remove + insert when the slot is wrong, so
/// the common per-layout call touches nothing.
fn place_host(layer: &NSObject, host: &NSObject) {
    let subs: *mut NSObject = unsafe { msg_send![layer, sublayers] };
    let host_ptr = host as *const NSObject as *mut NSObject;
    let mut host_idx = None;
    let mut gradient: Option<(usize, *mut NSObject)> = None;
    if !subs.is_null() {
        let count: usize = unsafe { msg_send![subs, count] };
        for i in 0..count {
            let entry: *mut NSObject = unsafe { msg_send![subs, objectAtIndex: i] };
            if entry == host_ptr {
                host_idx = Some(i);
            } else if gradient.is_none() && layer_name(entry).as_deref() == Some(GRADIENT_NAME) {
                gradient = Some((i, entry));
            }
        }
    }
    let in_place = match (host_idx, gradient) {
        (Some(h), Some((g, _))) => h == g + 1,
        (Some(h), None) => h == 0,
        (None, _) => false,
    };
    if in_place {
        return;
    }
    unsafe {
        if host_idx.is_some() {
            let _: () = msg_send![host, removeFromSuperlayer];
        }
        match gradient {
            Some((_, g)) => {
                let _: () = msg_send![layer, insertSublayer: host, above: g];
            }
            None => {
                let _: () = msg_send![layer, insertSublayer: host, atIndex: 0u32];
            }
        }
    }
}

/// The patterned stroke as Core Animation will draw it, for introspection: the
/// loop for a uniform border, else the TOP side (web's introspection reads
/// `border-top-*`, so both report the same side). `None` when no patterned
/// border is installed or that piece is absent.
///
/// The style is read back from the piece's live dash pattern and cap — a
/// zero-length round-capped dash is a dot — not from the host's stored style,
/// so a layer that lost its pattern is not reported as dashed.
pub fn read_stroke(layer: &NSObject) -> Option<Readback> {
    let host = host(layer)?;
    let pieces = pieces_of(&host);
    let pick = |want: Piece| {
        pieces
            .iter()
            .find(|(_, p)| *p == Some(want))
            .map(|(c, _)| c.clone())
    };
    let shape = pick(Piece::Loop).or_else(|| pick(Piece::Side(0)))?;
    let path: CGPathRef = unsafe { msg_send![&*shape, path] };
    if path.0.is_null() {
        return None;
    }
    let width: CGFloat = unsafe { msg_send![&*shape, lineWidth] };
    let color: CGColorRef = unsafe { msg_send![&*shape, strokeColor] };
    let pattern: *mut NSObject = unsafe { msg_send![&*shape, lineDashPattern] };
    let count: usize = if pattern.is_null() {
        0
    } else {
        unsafe { msg_send![pattern, count] }
    };
    let style = if count == 0 {
        BorderStyle::Solid
    } else {
        let first: *mut NSObject = unsafe { msg_send![pattern, objectAtIndex: 0usize] };
        let on: f64 = unsafe { msg_send![first, doubleValue] };
        let cap: *mut NSString = unsafe { msg_send![&*shape, lineCap] };
        let round = !cap.is_null() && unsafe { &*cap }.to_string() == CAP_ROUND;
        if on == 0.0 && round {
            BorderStyle::Dotted
        } else {
            BorderStyle::Dashed
        }
    };
    Some(Readback {
        width,
        color,
        style,
    })
}

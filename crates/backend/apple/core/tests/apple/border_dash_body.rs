//! Live-CALayer test for `backend_apple_core::border_dash_layer`.
//!
//! Real `CALayer`s / `CAShapeLayer`s, real `CGPath`s, and a real rasterization
//! through `-[CALayer renderInContext:]`, so it checks that the border actually
//! PAINTS dashes (gaps along the edge, background untouched inside) rather than
//! restating the properties it set. Reaching the end also proves every
//! CoreGraphics handle crosses `msg_send!` with the right type encoding — a
//! wrong one aborts the process instead of failing an assertion.
//!
//! Run: `cargo test -p backend-apple-core --test border_dash_calayer`

use backend_apple_core::border_dash_layer::{self, Piece};
use backend_apple_core::cg::{CGColorRef, CGPathRef};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, NSString};
use runtime_shared::BorderStyle;

#[link(name = "QuartzCore", kind = "framework")]
extern "C" {}

#[repr(transparent)]
#[derive(Clone, Copy)]
struct CGContextRef(*mut std::ffi::c_void);
unsafe impl objc2::encode::Encode for CGContextRef {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("CGContext", &[]));
}
#[repr(transparent)]
#[derive(Clone, Copy)]
struct CGColorSpaceRef(*mut std::ffi::c_void);

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGColorSpaceRelease(cs: CGColorSpaceRef);
    fn CGBitmapContextCreate(
        data: *mut std::ffi::c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
    ) -> CGContextRef;
    fn CGBitmapContextGetData(ctx: CGContextRef) -> *mut u8;
    fn CGContextRelease(ctx: CGContextRef);
    fn CGColorCreate(space: CGColorSpaceRef, components: *const CGFloat) -> CGColorRef;
    fn CGPathGetBoundingBox(path: CGPathRef) -> CGRect;
}

const ALPHA_PREMULTIPLIED_LAST: u32 = 1;

fn rasterize(root: &NSObject, w: usize, h: usize) -> Vec<u8> {
    unsafe {
        let cs = CGColorSpaceCreateDeviceRGB();
        let ctx = CGBitmapContextCreate(
            std::ptr::null_mut(),
            w,
            h,
            8,
            w * 4,
            cs,
            ALPHA_PREMULTIPLIED_LAST,
        );
        assert!(!ctx.0.is_null());
        let _: () = msg_send![root, renderInContext: ctx];
        let data = CGBitmapContextGetData(ctx);
        let out = std::slice::from_raw_parts(data, w * h * 4).to_vec();
        CGContextRelease(ctx);
        CGColorSpaceRelease(cs);
        out
    }
}

fn red_at(buf: &[u8], w: usize, x: usize, y: usize) -> u8 {
    buf[(y * w + x) * 4]
}

fn rgba(r: CGFloat, g: CGFloat, b: CGFloat, a: CGFloat) -> CGColorRef {
    unsafe {
        let cs = CGColorSpaceCreateDeviceRGB();
        let c = CGColorCreate(cs, [r, g, b, a].as_ptr());
        CGColorSpaceRelease(cs);
        c
    }
}

fn new_layer(class_name: &str) -> Retained<NSObject> {
    let cls = objc2::runtime::AnyClass::get(class_name).expect("class");
    unsafe { msg_send_id![cls, layer] }
}

fn rect(x: CGFloat, y: CGFloat, w: CGFloat, h: CGFloat) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize {
            width: w,
            height: h,
        },
    }
}

fn sublayers(layer: &NSObject) -> Vec<Retained<NSObject>> {
    let subs: *mut NSObject = unsafe { msg_send![layer, sublayers] };
    if subs.is_null() {
        return Vec::new();
    }
    let count: usize = unsafe { msg_send![subs, count] };
    (0..count)
        .map(|i| {
            let p: *mut NSObject = unsafe { msg_send![subs, objectAtIndex: i] };
            unsafe { Retained::retain(p) }.unwrap()
        })
        .collect()
}

fn name_of(layer: &NSObject) -> String {
    let n: *mut NSString = unsafe { msg_send![layer, name] };
    if n.is_null() {
        String::new()
    } else {
        unsafe { &*n }.to_string()
    }
}

fn names(layer: &NSObject) -> Vec<String> {
    sublayers(layer).iter().map(|l| name_of(l)).collect()
}

struct Report {
    failures: Vec<String>,
    checks: usize,
}

impl Report {
    fn check(&mut self, what: &str, ok: bool) {
        self.checks += 1;
        if ok {
            println!("  ok   {what}");
        } else {
            println!("  FAIL {what}");
            self.failures.push(what.to_string());
        }
    }
}

/// Count dark runs along a row between `x0..x1` (a "run" = a maximal stretch of
/// pixels with red < 128 on the white background).
fn dark_runs(buf: &[u8], w: usize, y: usize, x0: usize, x1: usize) -> usize {
    let mut runs = 0;
    let mut in_run = false;
    for x in x0..x1 {
        let dark = red_at(buf, w, x, y) < 128;
        if dark && !in_run {
            runs += 1;
        }
        in_run = dark;
    }
    runs
}

pub fn run() {
    let mut r = Report {
        failures: Vec::new(),
        checks: 0,
    };
    const W: usize = 100;
    const H: usize = 40;

    // A white card with a rounded corner and a 2px dashed black border.
    let card = new_layer("CALayer");
    let _: () = unsafe { msg_send![&*card, setFrame: rect(0.0, 0.0, W as f64, H as f64)] };
    let _: () = unsafe { msg_send![&*card, setBackgroundColor: rgba(1.0, 1.0, 1.0, 1.0)] };
    let _: () = unsafe { msg_send![&*card, setCornerRadius: 8.0 as CGFloat] };
    // A "child view" layer already present — the host must go BELOW it.
    let child = new_layer("CALayer");
    let _: () = unsafe { msg_send![&*card, addSublayer: &*child] };

    let black = rgba(0.0, 0.0, 0.0, 1.0);
    println!("uniform dashed loop");
    border_dash_layer::install(
        &card,
        BorderStyle::Dashed,
        &[(Piece::Loop, 2.0, black)],
        true,
    );

    let host = border_dash_layer::host(&card);
    r.check("install creates the host layer", host.is_some());
    let host = host.unwrap();
    r.check(
        "host is named for layer dumps",
        name_of(&host) == border_dash_layer::HOST_NAME,
    );
    r.check(
        "host sits at index 0 — above the background, below the child layer (CSS paint order)",
        names(&card).first().map(String::as_str) == Some(border_dash_layer::HOST_NAME),
    );
    let pieces = sublayers(&host);
    r.check("one CAShapeLayer for a uniform border", pieces.len() == 1);
    let fill: CGColorRef = unsafe { msg_send![&*pieces[0], fillColor] };
    r.check(
        "fillColor is cleared (CAShapeLayer defaults to opaque black)",
        fill.0.is_null(),
    );
    let host_frame: CGRect = unsafe { msg_send![&*host, frame] };
    r.check(
        "host frame tracks bounds",
        host_frame.size.width == W as f64 && host_frame.size.height == H as f64,
    );

    let buf = rasterize(&card, W, H);
    // Row 1 is the top (or, after the CG y-flip, bottom) edge's centreline;
    // both edges are dashed, so the flip doesn't matter here.
    let runs = dark_runs(&buf, W, 1, 12, 88);
    r.check(
        &format!("the edge paints separate dashes, not one line ({runs} runs)"),
        runs >= 5,
    );
    r.check(
        "the interior keeps the background (no filled loop)",
        red_at(&buf, W, 50, 20) > 240,
    );

    let rb = border_dash_layer::read_stroke(&card);
    r.check(
        "read_stroke reports width 2 + Dashed",
        rb.as_ref().is_some_and(|b| {
            b.width == 2.0 && b.style == BorderStyle::Dashed && !b.color.0.is_null()
        }),
    );

    println!("re-trace on resize");
    let _: () = unsafe { msg_send![&*card, setBounds: rect(0.0, 0.0, 200.0, 40.0)] };
    border_dash_layer::sync(&card);
    let path: CGPathRef = unsafe { msg_send![&*pieces[0], path] };
    let bb = unsafe { CGPathGetBoundingBox(path) };
    r.check(
        &format!(
            "sync re-traces the centreline to the new bounds (bbox w {})",
            bb.size.width
        ),
        (bb.size.width - 198.0).abs() < 0.01 && (bb.origin.x - 1.0).abs() < 0.01,
    );

    println!("z-order with a gradient");
    let grad = new_layer("CAGradientLayer");
    let gname = NSString::from_str("idealyst_gradient");
    let _: () = unsafe { msg_send![&*grad, setName: &*gname] };
    // A gradient applied AFTER the border is inserted at index 0 — the next
    // sync must lift the host back above it.
    let _: () = unsafe { msg_send![&*card, insertSublayer: &*grad, atIndex: 0u32] };
    border_dash_layer::sync(&card);
    let order = names(&card);
    r.check(
        &format!("host sits directly above the gradient ({order:?})"),
        order.len() == 3
            && order[0] == "idealyst_gradient"
            && order[1] == border_dash_layer::HOST_NAME,
    );

    println!("dotted, per side, then back to a loop");
    border_dash_layer::install(
        &card,
        BorderStyle::Dotted,
        &[(Piece::Side(0), 3.0, black), (Piece::Side(2), 1.0, black)],
        true,
    );
    let piece_names = names(&host);
    r.check(
        &format!("asymmetric → one shape layer per side, loop removed ({piece_names:?})"),
        piece_names.len() == 2
            && piece_names.contains(&"idealyst_border_dash_top".to_string())
            && piece_names.contains(&"idealyst_border_dash_bottom".to_string()),
    );
    let rb = border_dash_layer::read_stroke(&card);
    r.check(
        "read_stroke reads the top side as Dotted, width 3",
        rb.as_ref()
            .is_some_and(|b| b.width == 3.0 && b.style == BorderStyle::Dotted),
    );
    r.check(
        "the host was reused, not stacked",
        border_dash_layer::host(&card)
            .is_some_and(|h| &*h as *const NSObject == &*host as *const NSObject)
            && names(&card)
                .iter()
                .filter(|n| *n == border_dash_layer::HOST_NAME)
                .count()
                == 1,
    );
    border_dash_layer::install(
        &card,
        BorderStyle::Dashed,
        &[(Piece::Loop, 1.0, black)],
        true,
    );
    r.check(
        "sides removed when the border turns uniform",
        names(&host) == vec!["idealyst_border_dash_loop".to_string()],
    );

    println!("teardown");
    border_dash_layer::remove(&card);
    r.check(
        "remove drops the host handle",
        border_dash_layer::host(&card).is_none(),
    );
    r.check(
        "…and unparents it — no ghost stroke",
        !names(&card)
            .iter()
            .any(|n| n == border_dash_layer::HOST_NAME),
    );
    r.check(
        "read_stroke reports nothing after remove",
        border_dash_layer::read_stroke(&card).is_none(),
    );

    println!("zero-size view");
    let empty = new_layer("CALayer");
    border_dash_layer::install(
        &empty,
        BorderStyle::Dashed,
        &[(Piece::Loop, 1.0, black)],
        true,
    );
    let h: bool = unsafe { msg_send![&*border_dash_layer::host(&empty).unwrap(), isHidden] };
    r.check("a 0×0 layer hides the host until layout gives it a size", h);

    println!("{} checks, {} failures", r.checks, r.failures.len());
    if !r.failures.is_empty() {
        for f in &r.failures {
            eprintln!("FAILED: {f}");
        }
        std::process::exit(1);
    }
}

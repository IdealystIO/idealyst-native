//! Live-CALayer test for `backend_apple_core::layer_paint_order` — and for
//! the Core Animation behaviour the Apple backends' `z_index` rests on.
//!
//! Two renderers over the same real layer trees:
//! - **`CARenderer`** (macOS only) is Core Animation's own compositor, the
//!   one the window server runs: it is the ground truth for what the screen
//!   shows. These checks pin the platform semantics the backends rely on —
//!   sibling sublayers paint by `zPosition` (ties in tree order), and a
//!   `CATransformLayer` (the reactive anchor's layer) lets its sublayers
//!   sort together with its own siblings, exactly like web's
//!   `display: contents`.
//! - **`renderInContext:`** is the software path snapshots go through. Bare,
//!   it ignores both; inside `with_layers_in_paint_order` it must match the
//!   compositor, and every `sublayers` array must be restored afterwards.
//!
//! Run: `cargo test -p backend-apple-core --test layer_paint_order_calayer`

use backend_apple_core::cg::CGColorRef;
use backend_apple_core::layer_paint_order::with_layers_in_paint_order;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize};

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
}

const ALPHA_PREMULTIPLIED_LAST: u32 = 1;

fn rgba(r: CGFloat, g: CGFloat, b: CGFloat) -> CGColorRef {
    unsafe {
        let cs = CGColorSpaceCreateDeviceRGB();
        let c = CGColorCreate(cs, [r, g, b, 1.0].as_ptr());
        CGColorSpaceRelease(cs);
        c
    }
}

const W: usize = 40;
const H: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Px {
    Red,
    Green,
    Blue,
    Yellow,
    Other([u8; 3]),
}

fn classify(r: u8, g: u8, b: u8) -> Px {
    let hi = |v: u8| v > 200;
    let lo = |v: u8| v < 60;
    match (hi(r), hi(g), hi(b), lo(r), lo(g), lo(b)) {
        (true, _, _, _, true, true) => Px::Red,
        (_, true, _, true, _, true) => Px::Green,
        (_, _, true, true, true, _) => Px::Blue,
        (true, true, _, _, _, true) => Px::Yellow,
        _ => Px::Other([r, g, b]),
    }
}

fn layer(class_name: &str, x: CGFloat, w: CGFloat) -> Retained<NSObject> {
    let cls = objc2::runtime::AnyClass::get(class_name).expect("class");
    let l: Retained<NSObject> = unsafe { msg_send_id![cls, layer] };
    let frame = CGRect {
        origin: CGPoint { x, y: 0.0 },
        size: CGSize { width: w, height: H as CGFloat },
    };
    let _: () = unsafe { msg_send![&l, setFrame: frame] };
    l
}

fn fill(x: CGFloat, w: CGFloat, color: CGColorRef, z: CGFloat) -> Retained<NSObject> {
    let l = layer("CALayer", x, w);
    unsafe {
        let _: () = msg_send![&l, setBackgroundColor: color];
        let _: () = msg_send![&l, setZPosition: z];
    }
    l
}

fn add(parent: &NSObject, child: &NSObject) {
    let _: () = unsafe { msg_send![parent, addSublayer: child] };
}

fn sublayer_ptrs(l: &NSObject) -> Vec<usize> {
    unsafe {
        let arr: *mut NSObject = msg_send![l, sublayers];
        if arr.is_null() {
            return Vec::new();
        }
        let n: usize = msg_send![arr, count];
        (0..n)
            .map(|i| {
                let p: *mut NSObject = msg_send![arr, objectAtIndex: i];
                p as usize
            })
            .collect()
    }
}

/// `[red z 2, blue z 0]`, both full width: red must be on top.
fn raised_first_sibling() -> Retained<NSObject> {
    let root = layer("CALayer", 0.0, W as CGFloat);
    add(&root, &fill(0.0, W as CGFloat, rgba(1.0, 0.0, 0.0), 2.0));
    add(&root, &fill(0.0, W as CGFloat, rgba(0.0, 0.0, 1.0), 0.0));
    root
}

/// `[anchor{green z 0 left half, blue z 2 right half}, yellow z 1 full]` —
/// web (`display: contents`): yellow over green on the left, blue over
/// yellow on the right. A plain layer for the anchor would put yellow on
/// top everywhere.
fn anchor_interleave() -> (Retained<NSObject>, Retained<NSObject>) {
    let root = layer("CALayer", 0.0, W as CGFloat);
    let anchor = layer("CATransformLayer", 0.0, W as CGFloat);
    add(&anchor, &fill(0.0, (W / 2) as CGFloat, rgba(0.0, 1.0, 0.0), 0.0));
    add(&anchor, &fill((W / 2) as CGFloat, (W / 2) as CGFloat, rgba(0.0, 0.0, 1.0), 2.0));
    add(&root, &anchor);
    add(&root, &fill(0.0, W as CGFloat, rgba(1.0, 1.0, 0.0), 1.0));
    (root, anchor)
}

/// `[anchor{green z 0}, yellow z 0]` and the reverse: equal z falls back to
/// tree order with the anchor's children in the anchor's place.
fn anchor_tie(anchor_first: bool) -> Retained<NSObject> {
    let root = layer("CALayer", 0.0, W as CGFloat);
    let anchor = layer("CATransformLayer", 0.0, W as CGFloat);
    add(&anchor, &fill(0.0, W as CGFloat, rgba(0.0, 1.0, 0.0), 0.0));
    let yellow = fill(0.0, W as CGFloat, rgba(1.0, 1.0, 0.0), 0.0);
    if anchor_first {
        add(&root, &anchor);
        add(&root, &yellow);
    } else {
        add(&root, &yellow);
        add(&root, &anchor);
    }
    root
}

/// Rasterize with `renderInContext:` (RGBA) and read the pixel at `x`.
fn software(root: &NSObject, xs: &[usize]) -> Vec<Px> {
    unsafe {
        let cs = CGColorSpaceCreateDeviceRGB();
        let ctx = CGBitmapContextCreate(std::ptr::null_mut(), W, H, 8, W * 4, cs, ALPHA_PREMULTIPLIED_LAST);
        assert!(!ctx.0.is_null());
        let _: () = msg_send![root, renderInContext: ctx];
        let data = std::slice::from_raw_parts(CGBitmapContextGetData(ctx), W * H * 4).to_vec();
        CGContextRelease(ctx);
        CGColorSpaceRelease(cs);
        xs.iter()
            .map(|&x| {
                let i = ((H / 2) * W + x) * 4;
                classify(data[i], data[i + 1], data[i + 2])
            })
            .collect()
    }
}

#[cfg(target_os = "macos")]
mod compositor {
    //! `CARenderer` over a Metal texture: Core Animation's own compositor.
    use super::*;

    #[link(name = "Metal", kind = "framework")]
    extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut NSObject;
    }
    #[link(name = "QuartzCore", kind = "framework")]
    extern "C" {
        static kCARendererMetalCommandQueue: *mut NSObject;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MTLOrigin { x: usize, y: usize, z: usize }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MTLSize { w: usize, h: usize, d: usize }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MTLRegion { origin: MTLOrigin, size: MTLSize }
    unsafe impl objc2::encode::Encode for MTLRegion {
        const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
            "?",
            &[
                objc2::encode::Encoding::Struct("?", &[usize::ENCODING, usize::ENCODING, usize::ENCODING]),
                objc2::encode::Encoding::Struct("?", &[usize::ENCODING, usize::ENCODING, usize::ENCODING]),
            ],
        );
    }

    /// `const CVTimeStamp *` — passed as null; the type only has to carry
    /// the encoding objc2 checks against `beginFrameAtTime:timeStamp:`.
    #[repr(transparent)]
    #[derive(Clone, Copy)]
    struct CVTimeStampPtr(*mut std::ffi::c_void);
    unsafe impl objc2::encode::Encode for CVTimeStampPtr {
        const ENCODING: objc2::encode::Encoding = {
            use objc2::encode::Encoding as E;
            E::Pointer(&E::Struct(
                "?",
                &[
                    E::UInt,
                    E::Int,
                    E::LongLong,
                    E::ULongLong,
                    E::Double,
                    E::LongLong,
                    E::Struct(
                        "CVSMPTETime",
                        &[E::Short, E::Short, E::UInt, E::UInt, E::UInt, E::Short, E::Short, E::Short, E::Short],
                    ),
                    E::ULongLong,
                    E::ULongLong,
                ],
            ))
        };
    }

    /// `None` when the machine has no Metal device (then the ground-truth
    /// half is skipped; the software half still runs).
    pub fn render(root: &NSObject, xs: &[usize]) -> Option<Vec<Px>> {
        unsafe {
            let device = MTLCreateSystemDefaultDevice();
            if device.is_null() {
                return None;
            }
            const BGRA8_UNORM: usize = 80;
            let desc: *mut NSObject = msg_send![
                class!(MTLTextureDescriptor),
                texture2DDescriptorWithPixelFormat: BGRA8_UNORM,
                width: W,
                height: H,
                mipmapped: false
            ];
            // MTLTextureUsageShaderRead | MTLTextureUsageRenderTarget.
            let _: () = msg_send![desc, setUsage: 5usize];
            // MTLStorageModeShared.
            let _: () = msg_send![desc, setStorageMode: 0usize];
            let tex: *mut NSObject = msg_send![device, newTextureWithDescriptor: desc];
            let queue: *mut NSObject = msg_send![device, newCommandQueue];
            let options: *mut NSObject = msg_send![
                class!(NSDictionary),
                dictionaryWithObject: queue,
                forKey: kCARendererMetalCommandQueue
            ];
            let renderer: *mut NSObject =
                msg_send![class!(CARenderer), rendererWithMTLTexture: tex, options: options];
            let _: () = msg_send![renderer, setLayer: root];
            let _: () = msg_send![class!(CATransaction), flush];
            let bounds = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: W as CGFloat, height: H as CGFloat },
            };
            let _: () = msg_send![renderer, setBounds: bounds];
            let now: f64 = msg_send![class!(NSDate), timeIntervalSinceReferenceDate];
            let ts = CVTimeStampPtr(std::ptr::null_mut());
            let _: () = msg_send![renderer, beginFrameAtTime: now, timeStamp: ts];
            let _: () = msg_send![renderer, addUpdateRect: bounds];
            let _: () = msg_send![renderer, render];
            let _: () = msg_send![renderer, endFrame];
            let cb: *mut NSObject = msg_send![queue, commandBuffer];
            let _: () = msg_send![cb, commit];
            let _: () = msg_send![cb, waitUntilCompleted];
            let mut buf = vec![0u8; W * H * 4];
            let region = MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize { w: W, h: H, d: 1 },
            };
            let _: () = msg_send![
                tex,
                getBytes: buf.as_mut_ptr() as *mut std::ffi::c_void,
                bytesPerRow: W * 4,
                fromRegion: region,
                mipmapLevel: 0usize
            ];
            Some(
                xs.iter()
                    .map(|&x| {
                        let i = ((H / 2) * W + x) * 4;
                        classify(buf[i + 2], buf[i + 1], buf[i])
                    })
                    .collect(),
            )
        }
    }
}

struct Report(Vec<String>, usize);

impl Report {
    fn eq(&mut self, what: &str, got: Vec<Px>, want: &[Px]) {
        self.1 += 1;
        if got != want {
            self.0.push(format!("{what}: got {got:?}, want {want:?}"));
        }
    }
}

const LEFT: usize = 5;
const RIGHT: usize = 30;
const MID: usize = 20;

pub fn run() {
    let mut r = Report(Vec::new(), 0);

    // Ground truth: Core Animation's compositor.
    #[cfg(target_os = "macos")]
    {
        if let Some(px) = compositor::render(&raised_first_sibling(), &[MID]) {
            r.eq("compositor: zPosition 2 paints over a later sibling", px, &[Px::Red]);
            let px = compositor::render(&anchor_interleave().0, &[LEFT, RIGHT]).unwrap();
            r.eq(
                "compositor: a transform layer's children sort with its siblings",
                px,
                &[Px::Yellow, Px::Blue],
            );
            let px = compositor::render(&anchor_tie(true), &[MID]).unwrap();
            r.eq("compositor: tie, anchor first → later sibling on top", px, &[Px::Yellow]);
            let px = compositor::render(&anchor_tie(false), &[MID]).unwrap();
            r.eq("compositor: tie, anchor last → anchor's child on top", px, &[Px::Green]);
        } else {
            println!("layer_paint_order_calayer: no Metal device — compositor checks skipped");
        }
    }

    // The gap being closed: bare `renderInContext:` ignores both.
    r.eq(
        "renderInContext alone draws in array order",
        software(&raised_first_sibling(), &[MID]),
        &[Px::Blue],
    );

    // With the paint-order pass, the software path matches the compositor.
    let root = raised_first_sibling();
    let before = sublayer_ptrs(&root);
    let px = with_layers_in_paint_order(&root, || software(&root, &[MID]));
    r.eq("paint order: zPosition 2 paints over a later sibling", px, &[Px::Red]);
    r.1 += 1;
    if sublayer_ptrs(&root) != before {
        r.0.push("paint order: sibling order was not restored".into());
    }

    let (root, anchor) = anchor_interleave();
    let (root_before, anchor_before) = (sublayer_ptrs(&root), sublayer_ptrs(&anchor));
    let px = with_layers_in_paint_order(&root, || software(&root, &[LEFT, RIGHT]));
    r.eq(
        "paint order: a transform layer's children sort with its siblings",
        px,
        &[Px::Yellow, Px::Blue],
    );
    r.1 += 1;
    if sublayer_ptrs(&root) != root_before || sublayer_ptrs(&anchor) != anchor_before {
        r.0.push("paint order: the folded transform layer was not restored".into());
    }

    let root = anchor_tie(true);
    let px = with_layers_in_paint_order(&root, || software(&root, &[MID]));
    r.eq("paint order: tie, anchor first → later sibling on top", px, &[Px::Yellow]);
    let root = anchor_tie(false);
    let px = with_layers_in_paint_order(&root, || software(&root, &[MID]));
    r.eq("paint order: tie, anchor last → anchor's child on top", px, &[Px::Green]);

    if !r.0.is_empty() {
        for f in &r.0 {
            eprintln!("FAIL {f}");
        }
        panic!("layer_paint_order_calayer: {} of {} checks failed", r.0.len(), r.1);
    }
    println!("layer_paint_order_calayer: {} passed", r.1);
}

//! Generated codes must scan. Every way this crate produces a QR code is fed
//! back through its own decoder:
//!
//! 1. the [`QrMatrix`] itself, rasterized on the CPU, at every error-correction
//!    level and for text, binary and large payloads;
//! 2. the SVG from [`QrMatrix::to_svg`], its path parsed back into pixels;
//! 3. the [`QrCode`] component end to end — mounted through the scene
//!    registry, told its size the way a renderer does, painted through
//!    `CanvasPrim::paint`, rasterized on the GPU by vello's headless
//!    compositor, decoded. (Skips when the machine has no GPU adapter.)

#![cfg(not(target_arch = "wasm32"))]

use std::cell::RefCell;

use canvas::{CanvasPrim, Scene, SizeReporter};
use host_mock::Harness;
use qr::{decode_rgba8, ErrorCorrection, QrCode, QrMatrix, SvgOptions, DEFAULT_QUIET_ZONE};
use runtime_core::ui;
use runtime_scene::{MountCx, Realized};
use runtime_vocabulary::caps::DocumentOps;

/// Paint `m` with a `quiet`-module margin at `px` pixels per module, black on
/// white, as tightly-packed RGBA.
fn rasterize(m: &QrMatrix, quiet: usize, px: usize) -> (u32, Vec<u8>) {
    let side = (m.width() + 2 * quiet) * px;
    let mut rgba = [255u8; 4].repeat(side * side);
    for (x, y, len) in m.dark_runs() {
        for yy in (y + quiet) * px..(y + quiet + 1) * px {
            for xx in (x + quiet) * px..(x + quiet + len) * px {
                rgba[(yy * side + xx) * 4..(yy * side + xx) * 4 + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
    }
    (side as u32, rgba)
}

fn decoded_bytes(side: u32, rgba: &[u8]) -> Vec<Vec<u8>> {
    decode_rgba8(side, side, rgba).into_iter().map(|c| c.bytes).collect()
}

#[test]
fn every_error_correction_level_round_trips() {
    for ec in [ErrorCorrection::Low, ErrorCorrection::Medium, ErrorCorrection::Quartile, ErrorCorrection::High] {
        let m = QrMatrix::encode("round trip", ec).unwrap();
        let (side, rgba) = rasterize(&m, DEFAULT_QUIET_ZONE, 4);
        assert_eq!(decoded_bytes(side, &rgba), vec![b"round trip".to_vec()], "{ec:?}");
    }
}

#[test]
fn binary_and_large_payloads_round_trip() {
    let binary: Vec<u8> = (0..=255u8).collect();
    let large = "https://example.com/?q=".to_string() + &"z".repeat(400);
    for payload in [binary, large.into_bytes()] {
        let m = QrMatrix::encode(&payload, ErrorCorrection::Medium).unwrap();
        let (side, rgba) = rasterize(&m, DEFAULT_QUIET_ZONE, 3);
        assert_eq!(decoded_bytes(side, &rgba), vec![payload.clone()]);
    }
}

/// The exported SVG's geometry is the code: parse its path (`M x y h n v1 h-n
/// z`, in modules, quiet zone included) back into pixels and scan it.
#[test]
fn the_svg_export_round_trips() {
    let m = QrMatrix::encode("from the svg", ErrorCorrection::Medium).unwrap();
    let svg = m.to_svg(&SvgOptions::default());
    let d = svg.split(" d=\"").nth(1).unwrap().split('"').next().unwrap();
    let side_modules = m.width() + 2 * DEFAULT_QUIET_ZONE;
    let px = 4;
    let side = side_modules * px;
    let mut rgba = [255u8; 4].repeat(side * side);
    for cmd in d.split('M').filter(|c| !c.is_empty()) {
        // "x yhLENv1h-LENz"
        let (xy, rest) = cmd.split_once('h').unwrap();
        let (x, y) = xy.split_once(' ').unwrap();
        let (x, y): (usize, usize) = (x.parse().unwrap(), y.parse().unwrap());
        let len: usize = rest.split_once('v').unwrap().0.parse().unwrap();
        for yy in y * px..(y + 1) * px {
            for xx in x * px..(x + len) * px {
                rgba[(yy * side + xx) * 4..(yy * side + xx) * 4 + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
    }
    assert_eq!(decoded_bytes(side as u32, &rgba), vec![b"from the svg".to_vec()]);
}

// ---------------------------------------------------------------------------
// The component, end to end on the GPU.
// ---------------------------------------------------------------------------

thread_local! {
    static PAINTED: RefCell<Option<Scene>> = const { RefCell::new(None) };
    static REPORTER: RefCell<Option<SizeReporter>> = const { RefCell::new(None) };
}

/// A minimal canvas renderer: paint through `CanvasPrim::paint` in a
/// subtree-owned effect (as every renderer does) and keep the last scene.
fn harness() -> Harness {
    Harness::with_registry(|r| {
        r.register::<CanvasPrim, _>(|cx: &mut MountCx<'_, _>, prim, _children| {
            let node = cx.backend().borrow_mut().create_element("canvas");
            REPORTER.with(|s| *s.borrow_mut() = Some(prim.size_reporter()));
            let prim = prim.clone();
            runtime_world::effect(move || {
                let scene = prim.paint();
                PAINTED.with(|s| *s.borrow_mut() = Some(scene));
            });
            node
        });
    })
}

fn gpu_raster(scene: &Scene, w: u32, h: u32) -> Option<Vec<u8>> {
    let mut c = canvas_vello::HeadlessCompositor::new()?;
    c.composite(scene, &[], &Scene::new(), w, h, 1.0);
    Some(c.read_rgba()?.data)
}

#[test]
fn the_qr_code_component_scans_when_rendered_on_the_gpu() {
    let h = harness();
    let el = h.world.enter(|| ui! { QrCode(data = "component on the gpu".to_string()) });
    let _realized: Realized<u32> = h.mount(el);
    h.flush();

    // A renderer learns its size and reports it; the painter re-runs at it.
    // 237 is deliberately not a multiple of the module count, so the
    // whole-pixel snapping and centering are exercised.
    let (w, hgt) = (237u32, 237u32);
    REPORTER.with(|r| r.borrow().clone().unwrap()).report(w as f32, hgt as f32);
    h.flush();
    let scene = PAINTED.with(|s| s.borrow_mut().take()).expect("painted");
    assert_eq!(scene.size(), (w as f32, hgt as f32));
    assert!(!scene.is_empty(), "the code is drawn once the canvas has a size");

    let Some(rgba) = gpu_raster(&scene, w, hgt) else {
        eprintln!("no GPU adapter — skipping the GPU raster half");
        return;
    };
    assert_eq!(decoded_bytes(w, &rgba), vec![b"component on the gpu".to_vec()]);
}

/// A wide canvas: the code stays square, centered, and scannable.
#[test]
fn the_component_stays_square_and_centered_in_a_wide_canvas() {
    let h = harness();
    let el = h.world.enter(|| ui! { QrCode(data = "wide".to_string(), error_correction = ErrorCorrection::High) });
    let _realized: Realized<u32> = h.mount(el);
    h.flush();
    let (w, hgt) = (400u32, 160u32);
    REPORTER.with(|r| r.borrow().clone().unwrap()).report(w as f32, hgt as f32);
    h.flush();
    let scene = PAINTED.with(|s| s.borrow_mut().take()).expect("painted");
    let Some(rgba) = gpu_raster(&scene, w, hgt) else {
        eprintln!("no GPU adapter — skipping");
        return;
    };
    // Outside the centered 160×160 square nothing is painted.
    let px = |x: u32, y: u32| &rgba[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
    assert_eq!(px(10, 80)[3], 0, "left of the square is untouched");
    assert_eq!(px(390, 80)[3], 0, "right of the square is untouched");
    assert_eq!(px(122, 2), &[255, 255, 255, 255], "the square's quiet zone is light");
    assert_eq!(decoded_bytes_rect(&rgba, w, 120, 0, 160), vec![b"wide".to_vec()]);
}

/// Decode the `side`-px square at `(x0, y0)` of a `w`-wide RGBA image.
fn decoded_bytes_rect(rgba: &[u8], w: u32, x0: u32, y0: u32, side: u32) -> Vec<Vec<u8>> {
    let mut out = Vec::with_capacity((side * side * 4) as usize);
    for y in y0..y0 + side {
        let start = ((y * w + x0) * 4) as usize;
        out.extend_from_slice(&rgba[start..start + (side * 4) as usize]);
    }
    decoded_bytes(side, &out)
}

#[test]
fn data_that_does_not_fit_draws_only_the_background() {
    let h = harness();
    let huge = "x".repeat(5000);
    let el = h.world.enter(move || ui! { QrCode(data = huge.clone()) });
    let _realized: Realized<u32> = h.mount(el);
    h.flush();
    REPORTER.with(|r| r.borrow().clone().unwrap()).report(100.0, 100.0);
    h.flush();
    let scene = PAINTED.with(|s| s.borrow_mut().take()).expect("painted");
    assert_eq!(scene.ops().len(), 1, "background only: {:?}", scene.ops());
}

#[test]
fn nothing_is_drawn_before_the_canvas_has_a_size() {
    let h = harness();
    let el = h.world.enter(|| ui! { QrCode(data = "early".to_string()) });
    let _realized: Realized<u32> = h.mount(el);
    h.flush();
    let scene = PAINTED.with(|s| s.borrow_mut().take()).expect("painted");
    assert!(scene.is_empty(), "first paint runs before layout: {:?}", scene.ops());
}

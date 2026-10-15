//! The painter's transform composition, as the native introspection reader
//! uses it to report where a node is drawn.
//!
//! ## What this pins
//!
//! `IntrospectionOps` was an empty impl on `WindowsBackend`: `introspect_native`
//! returned `None` for every element, so a `--parity …,windows` capture came
//! back empty and Windows could not take part in parity testing at all. The
//! reader reports the box the painter draws, which is this composition — frame
//! offset, then the node's op chain, then each ancestor's scroll offset — so a
//! drift between it and `scene::paint_node` would make the reader lie.
//!
//! The crate body is `cfg(target_os = "windows")`; this compiles the pure
//! module directly (`#[path]`) so it runs on every host. The live reader is
//! covered by `introspect::tests` (Windows target / Wine).

#[path = "../src/painted_geom.rs"]
#[allow(dead_code)]
mod painted_geom;

use painted_geom::*;

fn close(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    let d = |x: f32, y: f32| (x - y).abs() < 1e-3;
    d(a.0, b.0) && d(a.1, b.1) && d(a.2, b.2) && d(a.3, b.3)
}

#[test]
fn regression_windows_reader_reports_frames_nested_in_window_space() {
    // Parent at (10, 20), child at (5, 7) inside it: the child is drawn at
    // (15, 27) in the window.
    let parent = Affine::IDENTITY.node(10.0, 20.0, &[]);
    let child = parent.node(5.0, 7.0, &[]);
    assert!(close(child.bounds(30.0, 40.0), (15.0, 27.0, 30.0, 40.0)));
}

#[test]
fn regression_windows_reader_applies_ancestor_scroll_offset() {
    // A scroll view at (0, 100) scrolled down 60 px: its child at content
    // y = 80 is drawn at window y = 100 + 80 - 60 = 120.
    let scroller = Affine::IDENTITY.node(0.0, 100.0, &[]);
    let content = scroller.then_translate(0.0, -60.0);
    let child = content.node(0.0, 80.0, &[]);
    assert!(close(child.bounds(50.0, 10.0), (0.0, 120.0, 50.0, 10.0)));
}

#[test]
fn regression_windows_reader_scales_about_the_box_center() {
    // `transform: scale(0.5)` on a 100x40 box at (0, 0): the painter wraps it
    // in T(c)·S·T(-c), so the drawn box shrinks toward its centre.
    let ops = [XOp::Translate(50.0, 20.0), XOp::Scale(0.5, 0.5), XOp::Translate(-50.0, -20.0)];
    let m = Affine::IDENTITY.node(0.0, 0.0, &ops);
    assert!(close(m.bounds(100.0, 40.0), (25.0, 10.0, 50.0, 20.0)));
}

#[test]
fn regression_windows_reader_rotation_is_clockwise_like_gdiplus() {
    // GDI+ rotates clockwise on screen. A 90° turn about the origin maps the
    // +x axis onto +y.
    let m = Affine::IDENTITY.then_op(XOp::Rotate(90.0));
    let (x, y) = m.apply(10.0, 0.0);
    assert!(x.abs() < 1e-4 && (y - 10.0).abs() < 1e-4, "got ({x}, {y})");
    // The enclosing box of a rotated 20x10 rect about its centre is 10x20.
    let ops = [XOp::Translate(10.0, 5.0), XOp::Rotate(90.0), XOp::Translate(-10.0, -5.0)];
    assert!(close(Affine::IDENTITY.node(0.0, 0.0, &ops).bounds(20.0, 10.0), (5.0, -5.0, 10.0, 20.0)));
}

#[test]
fn regression_windows_reader_reports_painted_pill_radius() {
    // A 999 px "full" radius is drawn clamped to half the shorter side.
    assert_eq!(painted_radius([999.0; 4], 120.0, 24.0), Some(12.0));
    assert_eq!(painted_radius([0.0, 6.0, 6.0, 0.0], 100.0, 100.0), Some(6.0));
    assert_eq!(painted_radius([0.0; 4], 100.0, 100.0), None);
}

#[test]
fn regression_leaf_content_box_insets_by_padding() {
    // `[top, right, bottom, left]`.
    assert_eq!(content_box(100.0, 40.0, [10.0, 4.0, 6.0, 8.0]), (8.0, 10.0, 88.0, 24.0));
    assert_eq!(content_box(100.0, 40.0, [0.0; 4]), (0.0, 0.0, 100.0, 40.0));
    // Padding wider than the box: an empty box, never a negative one.
    assert_eq!(content_box(10.0, 10.0, [8.0, 8.0, 8.0, 8.0]), (8.0, 8.0, 0.0, 0.0));
}

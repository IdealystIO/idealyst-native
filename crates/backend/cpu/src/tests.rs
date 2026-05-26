//! End-to-end smoke tests for the CPU backend.
//!
//! These build a small framework tree directly via the `Backend`
//! trait (no `runtime_core::mount` — we want to drive the trait
//! methods in isolation), render to a `MemSurface`, and assert on
//! pixel-level output.
//!
//! What these tests are checking is the rasterizer pipeline:
//! style→Taffy translation, paint order, alpha composition, and
//! the bitmap-text path. They are NOT regression tests for any of
//! the higher-level framework systems (reactivity, scope wiring,
//! navigators); for those, the per-platform smoke tests in the
//! workspace cover the contract.

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::animation::AnimProp;
use runtime_core::{
    Backend, Gradient, GradientKind, GradientStop, Length, RadialExtent, StyleRules, Tokenized,
};

use crate::{CpuBackend, ClickOutcome, MemSurface, Surface};

/// Build a style with just a few fields set — convenience for the
/// tests, since `StyleRules` has 40+ optional fields and the
/// `..Default::default()` shape is noisy.
fn style_with(mut f: impl FnMut(&mut StyleRules)) -> Rc<StyleRules> {
    let mut s = StyleRules::default();
    f(&mut s);
    Rc::new(s)
}

#[test]
fn empty_backend_renders_clear_color_only() {
    let mut backend = CpuBackend::new(16, 16);
    backend.set_clear_color([10, 20, 30, 255]);
    let mut surface = MemSurface::new(16, 16);
    backend.render(&mut surface);
    // Every pixel matches the clear color — no root means nothing
    // to paint over it.
    for y in 0..16 {
        for x in 0..16 {
            assert_eq!(
                surface.get_pixel(x, y),
                [10, 20, 30, 255],
                "pixel ({x},{y}) should be clear color"
            );
        }
    }
}

#[test]
fn solid_view_paints_its_background_color() {
    let mut backend = CpuBackend::new(40, 40);
    let a11y = AccessibilityProps::default();
    let mut root = backend.create_view(&a11y);
    // Root takes the full viewport. Give it a recognizable red.
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.background = Some(Tokenized::Literal("rgb(255, 0, 0)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(40.0)));
            s.height = Some(Tokenized::Literal(Length::Px(40.0)));
        }),
    );
    // Inner child: 20x20 blue box at top-left.
    let child = backend.create_view(&a11y);
    backend.apply_style(
        &child,
        &style_with(|s| {
            s.background = Some(Tokenized::Literal("rgb(0, 0, 255)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(20.0)));
            s.height = Some(Tokenized::Literal(Length::Px(20.0)));
        }),
    );
    backend.insert(&mut root, child);
    backend.finish(root);

    let mut surface = MemSurface::new(40, 40);
    backend.render(&mut surface);

    // Inside the child rect → blue.
    assert_eq!(surface.get_pixel(5, 5), [0, 0, 255, 255]);
    assert_eq!(surface.get_pixel(15, 15), [0, 0, 255, 255]);
    // Outside the child but inside the root → red.
    assert_eq!(surface.get_pixel(30, 30), [255, 0, 0, 255]);
    assert_eq!(surface.get_pixel(25, 25), [255, 0, 0, 255]);
}

#[test]
fn rounded_corner_omits_pixels_inside_the_arc() {
    let mut backend = CpuBackend::new(20, 20);
    let a11y = AccessibilityProps::default();
    let root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.background = Some(Tokenized::Literal("rgb(0, 255, 0)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(20.0)));
            s.height = Some(Tokenized::Literal(Length::Px(20.0)));
            // Uniform 8px corner radius — the corner square is the
            // top-left 8x8 region, with the included circle of
            // radius 8 centered at (8, 8).
            s.border_top_left_radius = Some(Tokenized::Literal(Length::Px(8.0)));
            s.border_top_right_radius = Some(Tokenized::Literal(Length::Px(8.0)));
            s.border_bottom_left_radius = Some(Tokenized::Literal(Length::Px(8.0)));
            s.border_bottom_right_radius = Some(Tokenized::Literal(Length::Px(8.0)));
        }),
    );
    backend.finish(root);

    // Clear to a recognizable non-match color so we can tell "no
    // paint" from "painted with green by accident".
    let mut surface = MemSurface::new(20, 20);
    backend.set_clear_color([5, 5, 5, 255]);
    backend.render(&mut surface);

    // Pixel (0, 0) is the extreme corner — outside the 8px arc, so
    // it should still be the clear color.
    assert_eq!(surface.get_pixel(0, 0), [5, 5, 5, 255]);
    // Pixel (8, 8) is the corner-circle center; inside the shape.
    assert_eq!(surface.get_pixel(8, 8), [0, 255, 0, 255]);
    // Pixel (10, 10) is well inside the interior.
    assert_eq!(surface.get_pixel(10, 10), [0, 255, 0, 255]);
}

#[test]
fn alpha_blending_against_clear_color() {
    let mut backend = CpuBackend::new(10, 10);
    backend.set_clear_color([200, 200, 200, 255]);
    let a11y = AccessibilityProps::default();
    let root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            // Red at half alpha — should mix toward gray.
            s.background = Some(Tokenized::Literal("rgba(255, 0, 0, 0.5)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(10.0)));
            s.height = Some(Tokenized::Literal(Length::Px(10.0)));
        }),
    );
    backend.finish(root);

    let mut surface = MemSurface::new(10, 10);
    backend.render(&mut surface);

    let p = surface.get_pixel(5, 5);
    // R ≈ 0.5 * 255 + 0.5 * 200 = 227.5 → 227 or 228.
    assert!((p[0] as i32 - 227).abs() <= 2, "got red {}", p[0]);
    // G and B ≈ 0.5 * 0 + 0.5 * 200 = 100.
    assert!((p[1] as i32 - 100).abs() <= 2, "got green {}", p[1]);
    assert!((p[2] as i32 - 100).abs() <= 2, "got blue {}", p[2]);
    assert_eq!(p[3], 255);
}

#[test]
fn text_node_paints_glyph_pixels() {
    let mut backend = CpuBackend::new(80, 16);
    backend.set_clear_color([255, 255, 255, 255]);
    let a11y = AccessibilityProps::default();
    let mut root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(80.0)));
            s.height = Some(Tokenized::Literal(Length::Px(16.0)));
        }),
    );
    let text = backend.create_text("HI", &a11y);
    backend.apply_style(
        &text,
        &style_with(|s| {
            s.color = Some(Tokenized::Literal("rgb(0, 0, 0)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(16.0)));
            s.height = Some(Tokenized::Literal(Length::Px(8.0)));
        }),
    );
    backend.insert(&mut root, text);
    backend.finish(root);

    let mut surface = MemSurface::new(80, 16);
    backend.render(&mut surface);

    // The font's glyph for 'H' has a vertical stroke on its left
    // edge — row 0, column 0 of the glyph is bit 0 of byte 0
    // (`0x33`). We just need to assert *some* pixel inside the
    // glyph bounds is black, and the area outside is still white,
    // to prove the text rasterizer fired.
    let mut painted = 0;
    for y in 0..8 {
        for x in 0..16 {
            if surface.get_pixel(x, y) == [0, 0, 0, 255] {
                painted += 1;
            }
        }
    }
    assert!(painted > 0, "expected the text to paint at least one black pixel");

    // Outside the text bounds, the clear-color white should survive.
    assert_eq!(surface.get_pixel(70, 15), [255, 255, 255, 255]);
}

#[test]
fn click_dispatch_returns_handler_for_clicked_button() {
    let mut backend = CpuBackend::new(40, 40);
    let a11y = AccessibilityProps::default();
    let mut root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(40.0)));
            s.height = Some(Tokenized::Literal(Length::Px(40.0)));
        }),
    );

    // Button via Pressable (simpler signature — `create_button`
    // requires a full `Action` with a typed payload).
    let counter = Rc::new(std::cell::Cell::new(0u32));
    let counter_inner = counter.clone();
    let press = backend.create_pressable(
        Rc::new(move || counter_inner.set(counter_inner.get() + 1)),
        &a11y,
    );
    backend.apply_style(
        &press,
        &style_with(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(20.0)));
            s.height = Some(Tokenized::Literal(Length::Px(20.0)));
        }),
    );
    backend.insert(&mut root, press);
    backend.finish(root);

    // Render once so the frame cache is populated.
    let mut surface = MemSurface::new(40, 40);
    backend.render(&mut surface);

    // Click inside the pressable (top-left 20x20).
    match backend.dispatch_click(5, 5) {
        ClickOutcome::HandlerFired(h) => h(),
        other => panic!("expected HandlerFired, got {:?}", other),
    }
    assert_eq!(counter.get(), 1);

    // Click outside the pressable.
    match backend.dispatch_click(35, 35) {
        ClickOutcome::Unhandled => {}
        other => panic!("expected Unhandled, got {:?}", other),
    }
    assert_eq!(counter.get(), 1, "outside-click should not fire the handler");
}

#[test]
fn set_animated_f32_opacity_overrides_static() {
    let mut backend = CpuBackend::new(10, 10);
    backend.set_clear_color([0, 0, 0, 255]);
    let a11y = AccessibilityProps::default();
    let root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            // Static opacity 0 — would normally suppress the paint.
            s.background = Some(Tokenized::Literal("rgb(255, 0, 0)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(10.0)));
            s.height = Some(Tokenized::Literal(Length::Px(10.0)));
            s.opacity = Some(Tokenized::Literal(0.0));
        }),
    );
    // Drive opacity back up via the animation hook. The animated
    // override REPLACES the static value (it doesn't multiply).
    backend.set_animated_f32(&root, AnimProp::Opacity, 1.0);
    backend.finish(root);

    let mut surface = MemSurface::new(10, 10);
    backend.render(&mut surface);
    // With opacity now 1.0, the red should be fully visible.
    assert_eq!(surface.get_pixel(5, 5), [255, 0, 0, 255]);
}

#[test]
fn set_animated_f32_translate_shifts_paint_position() {
    let mut backend = CpuBackend::new(40, 20);
    backend.set_clear_color([0, 0, 0, 255]);
    let a11y = AccessibilityProps::default();
    let mut root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(40.0)));
            s.height = Some(Tokenized::Literal(Length::Px(20.0)));
        }),
    );
    let child = backend.create_view(&a11y);
    backend.apply_style(
        &child,
        &style_with(|s| {
            s.background = Some(Tokenized::Literal("rgb(0, 255, 0)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(10.0)));
            s.height = Some(Tokenized::Literal(Length::Px(10.0)));
            s.position = Some(runtime_core::Position::Absolute);
            s.top = Some(Tokenized::Literal(Length::Px(0.0)));
            s.left = Some(Tokenized::Literal(Length::Px(0.0)));
        }),
    );
    backend.set_animated_f32(&child, AnimProp::TranslateX, 20.0);
    backend.insert(&mut root, child);
    backend.finish(root);

    let mut surface = MemSurface::new(40, 20);
    backend.render(&mut surface);
    // Original child position is x=0; with translateX=20 it should
    // paint at x=20..30. Sample inside and outside.
    assert_eq!(surface.get_pixel(5, 5), [0, 0, 0, 255], "pre-translate spot should NOT be green");
    assert_eq!(surface.get_pixel(25, 5), [0, 255, 0, 255], "translated spot SHOULD be green");
}

#[test]
fn set_animated_color_overrides_background() {
    let mut backend = CpuBackend::new(10, 10);
    let a11y = AccessibilityProps::default();
    let root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.background = Some(Tokenized::Literal("rgb(255, 0, 0)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(10.0)));
            s.height = Some(Tokenized::Literal(Length::Px(10.0)));
        }),
    );
    // Animate the bg to green via the color hook.
    backend.set_animated_color(&root, AnimProp::BackgroundColor, [0.0, 1.0, 0.0, 1.0]);
    backend.finish(root);

    let mut surface = MemSurface::new(10, 10);
    backend.render(&mut surface);
    // Animated override wins over the static red.
    assert_eq!(surface.get_pixel(5, 5), [0, 255, 0, 255]);
}

#[test]
fn z_index_reorders_paint_order() {
    let mut backend = CpuBackend::new(20, 20);
    let a11y = AccessibilityProps::default();
    let mut root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(20.0)));
            s.height = Some(Tokenized::Literal(Length::Px(20.0)));
            s.position = Some(runtime_core::Position::Relative);
        }),
    );
    // Two overlapping children — red on bottom, blue on top by
    // insertion order. We then push the red ABOVE the blue via
    // ZIndex and confirm red wins the pixel.
    let red = backend.create_view(&a11y);
    backend.apply_style(
        &red,
        &style_with(|s| {
            s.background = Some(Tokenized::Literal("rgb(255, 0, 0)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(20.0)));
            s.height = Some(Tokenized::Literal(Length::Px(20.0)));
            s.position = Some(runtime_core::Position::Absolute);
            s.top = Some(Tokenized::Literal(Length::Px(0.0)));
            s.left = Some(Tokenized::Literal(Length::Px(0.0)));
        }),
    );
    let blue = backend.create_view(&a11y);
    backend.apply_style(
        &blue,
        &style_with(|s| {
            s.background = Some(Tokenized::Literal("rgb(0, 0, 255)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(20.0)));
            s.height = Some(Tokenized::Literal(Length::Px(20.0)));
            s.position = Some(runtime_core::Position::Absolute);
            s.top = Some(Tokenized::Literal(Length::Px(0.0)));
            s.left = Some(Tokenized::Literal(Length::Px(0.0)));
        }),
    );
    backend.insert(&mut root, red);
    backend.insert(&mut root, blue);
    // Without z-index, blue (inserted last) would win.
    backend.set_animated_f32(&red, AnimProp::ZIndex, 10.0);
    backend.finish(root);

    let mut surface = MemSurface::new(20, 20);
    backend.render(&mut surface);
    // Red wins because its z (10) > blue's z (default 0).
    assert_eq!(surface.get_pixel(10, 10), [255, 0, 0, 255]);
}

#[test]
fn linear_gradient_interpolates_across_axis() {
    let mut backend = CpuBackend::new(100, 10);
    backend.set_clear_color([0, 0, 0, 255]);
    let a11y = AccessibilityProps::default();
    let root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(100.0)));
            s.height = Some(Tokenized::Literal(Length::Px(10.0)));
            // Left → right (angle 90) red → green.
            s.background_gradient = Some(Gradient {
                kind: GradientKind::Linear { angle_deg: 90.0 },
                stops: vec![
                    GradientStop { offset: 0.0, color: runtime_core::Color("rgb(255, 0, 0)".into()) },
                    GradientStop { offset: 1.0, color: runtime_core::Color("rgb(0, 255, 0)".into()) },
                ],
            });
        }),
    );
    backend.finish(root);

    let mut surface = MemSurface::new(100, 10);
    backend.render(&mut surface);

    // x=0 → near pure red. x=99 → near pure green. x=50 → mix.
    let left = surface.get_pixel(0, 5);
    assert!(left[0] > 230, "left pixel should be mostly red, got {left:?}");
    assert!(left[1] < 25, "left pixel green channel should be low, got {left:?}");

    let right = surface.get_pixel(99, 5);
    assert!(right[1] > 230, "right pixel should be mostly green, got {right:?}");
    assert!(right[0] < 25, "right pixel red channel should be low, got {right:?}");

    let mid = surface.get_pixel(50, 5);
    assert!(
        (mid[0] as i32 - 127).abs() < 30 && (mid[1] as i32 - 127).abs() < 30,
        "midpoint should be ~50/50 red/green, got {mid:?}"
    );
}

#[test]
fn radial_gradient_interpolates_with_distance_from_center() {
    let mut backend = CpuBackend::new(40, 40);
    backend.set_clear_color([0, 0, 0, 255]);
    let a11y = AccessibilityProps::default();
    let root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(40.0)));
            s.height = Some(Tokenized::Literal(Length::Px(40.0)));
            // White center → black edge.
            s.background_gradient = Some(Gradient {
                kind: GradientKind::Radial {
                    center: (0.5, 0.5),
                    radius: 1.0,
                    extent: RadialExtent::ClosestSide,
                },
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: runtime_core::Color("rgb(255, 255, 255)".into()),
                    },
                    GradientStop { offset: 1.0, color: runtime_core::Color("rgb(0, 0, 0)".into()) },
                ],
            });
        }),
    );
    backend.finish(root);

    let mut surface = MemSurface::new(40, 40);
    backend.render(&mut surface);

    // Center pixel should be nearly white.
    let center = surface.get_pixel(20, 20);
    assert!(center[0] > 240, "center should be near-white, got {center:?}");

    // Far corner is OUTSIDE the closest-side radius (radius = 20px;
    // corner is at distance sqrt(20^2 + 20^2) ≈ 28px). Sample stops
    // clamp to the last stop, so it'll be (near) black.
    let corner = surface.get_pixel(39, 39);
    assert!(corner[0] < 30, "corner should be near-black, got {corner:?}");
}

#[test]
fn animated_gradient_stop_color_overrides_static() {
    let mut backend = CpuBackend::new(20, 10);
    backend.set_clear_color([0, 0, 0, 255]);
    let a11y = AccessibilityProps::default();
    let root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.width = Some(Tokenized::Literal(Length::Px(20.0)));
            s.height = Some(Tokenized::Literal(Length::Px(10.0)));
            // Both stops red — uniform fill.
            s.background_gradient = Some(Gradient {
                kind: GradientKind::Linear { angle_deg: 90.0 },
                stops: vec![
                    GradientStop { offset: 0.0, color: runtime_core::Color("rgb(255, 0, 0)".into()) },
                    GradientStop { offset: 1.0, color: runtime_core::Color("rgb(255, 0, 0)".into()) },
                ],
            });
        }),
    );
    // Animate stop 1 (the right side) to blue.
    backend.set_animated_color(&root, AnimProp::GradientStopColor(1), [0.0, 0.0, 1.0, 1.0]);
    backend.finish(root);

    let mut surface = MemSurface::new(20, 10);
    backend.render(&mut surface);

    // Left edge stays red, right edge becomes blue.
    let left = surface.get_pixel(0, 5);
    let right = surface.get_pixel(19, 5);
    assert!(left[0] > 230 && left[2] < 25, "left should be red, got {left:?}");
    assert!(right[2] > 230 && right[0] < 25, "right should be blue, got {right:?}");
}

#[test]
fn opacity_zero_skips_paint() {
    let mut backend = CpuBackend::new(10, 10);
    backend.set_clear_color([0, 0, 0, 255]);
    let a11y = AccessibilityProps::default();
    let root = backend.create_view(&a11y);
    backend.apply_style(
        &root,
        &style_with(|s| {
            s.background = Some(Tokenized::Literal("rgb(255, 0, 0)".into()));
            s.width = Some(Tokenized::Literal(Length::Px(10.0)));
            s.height = Some(Tokenized::Literal(Length::Px(10.0)));
            s.opacity = Some(Tokenized::Literal(0.0));
        }),
    );
    backend.finish(root);

    let mut surface = MemSurface::new(10, 10);
    backend.render(&mut surface);
    // The red should be entirely suppressed — the framebuffer
    // remains at the clear color.
    assert_eq!(surface.get_pixel(5, 5), [0, 0, 0, 255]);
}

#[test]
fn surface_present_is_called_once_per_render() {
    // A `Surface` impl that counts present calls — proves the
    // backend invokes `present` at the right cadence (once per
    // render, no extras).
    struct CountingSurface {
        inner: MemSurface,
        present_count: u32,
    }
    impl Surface for CountingSurface {
        fn width(&self) -> u32 { self.inner.width() }
        fn height(&self) -> u32 { self.inner.height() }
        fn put_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
            self.inner.put_pixel(x, y, rgba);
        }
        fn present(&mut self) { self.present_count += 1; }
    }

    let mut backend = CpuBackend::new(4, 4);
    let mut surface = CountingSurface {
        inner: MemSurface::new(4, 4),
        present_count: 0,
    };
    backend.render(&mut surface);
    backend.render(&mut surface);
    backend.render(&mut surface);
    assert_eq!(surface.present_count, 3);
}

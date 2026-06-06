//! Unit tests for the build-time SVG → path-data conversion (build_support.rs).
//!
//! These cover the shape-element normalization that lets raw Lucide SVGs —
//! which use `<circle>`, `<rect>`, `<line>`, etc. — drop straight into
//! `assets/`. Every native backend only understands path `d` strings, so a
//! regression here renders ~1000 icons (every circle/rect/line) blank.

// Pull in the exact functions build.rs uses.
include!("../build_support.rs");

#[test]
fn path_passthrough_keeps_d_verbatim() {
    let tag = r#"<path d="M10 11v6" />"#;
    assert_eq!(convert_element("path", tag).as_deref(), Some("M10 11v6"));
}

#[test]
fn circle_becomes_two_arcs() {
    // Lucide search.svg circle.
    let tag = r#"<circle cx="11" cy="11" r="8" />"#;
    assert_eq!(
        convert_element("circle", tag).as_deref(),
        Some("M3 11a8 8 0 1 0 16 0a8 8 0 1 0 -16 0Z"),
    );
}

#[test]
fn ellipse_uses_distinct_radii() {
    // Lucide cone.svg ellipse.
    let tag = r#"<ellipse cx="12" cy="19" rx="9" ry="3" />"#;
    assert_eq!(
        convert_element("ellipse", tag).as_deref(),
        Some("M3 19a9 3 0 1 0 18 0a9 3 0 1 0 -18 0Z"),
    );
}

#[test]
fn line_becomes_move_line() {
    let tag = r#"<line x1="2" y1="4" x2="6" y2="8" />"#;
    assert_eq!(
        convert_element("line", tag).as_deref(),
        Some("M2 4L6 8"),
    );
}

#[test]
fn rect_square_corners() {
    let tag = r#"<rect width="18" height="18" x="3" y="3" />"#;
    assert_eq!(
        convert_element("rect", tag).as_deref(),
        Some("M3 3h18v18h-18Z"),
    );
}

#[test]
fn rect_rounded_corners() {
    // Lucide activity-square.svg rect (rx mirrors to ry).
    let tag = r#"<rect width="18" height="18" x="3" y="3" rx="2" />"#;
    assert_eq!(
        convert_element("rect", tag).as_deref(),
        Some("M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1 -2 2h-14a2 2 0 0 1 -2 -2v-14a2 2 0 0 1 2 -2Z"),
    );
}

#[test]
fn rect_radius_clamped_to_half_side() {
    // rx far larger than half-width must clamp, not overshoot.
    let tag = r#"<rect width="10" height="10" x="0" y="0" rx="999" />"#;
    let d = convert_element("rect", tag).unwrap();
    // Clamped rx = ry = 5: straight runs collapse to length 0.
    assert_eq!(d, "M5 0h0a5 5 0 0 1 5 5v0a5 5 0 0 1 -5 5h0a5 5 0 0 1 -5 -5v0a5 5 0 0 1 5 -5Z");
}

#[test]
fn polyline_open_polygon_closed() {
    let pl = r#"<polyline points="1,2 3,4 5,6" />"#;
    assert_eq!(convert_element("polyline", pl).as_deref(), Some("M1 2L3 4L5 6"));
    let pg = r#"<polygon points="1,2 3,4 5,6" />"#;
    assert_eq!(convert_element("polygon", pg).as_deref(), Some("M1 2L3 4L5 6Z"));
}

#[test]
fn attr_boundary_distinguishes_x_from_rx() {
    // Querying `x` must not match inside `rx="2"`.
    let tag = r#"<rect rx="2" x="7" />"#;
    assert_eq!(attr(tag, "x"), Some("7"));
    assert_eq!(attr(tag, "rx"), Some("2"));
}

#[test]
fn svg_walk_preserves_document_order_and_skips_comment() {
    // search.svg: path first, then circle — order matters for compositing.
    let svg = r#"<!-- @license lucide-static -->
<svg viewBox="0 0 24 24" stroke="currentColor">
  <path d="m21 21-4.34-4.34" />
  <circle cx="11" cy="11" r="8" />
</svg>"#;
    let paths = svg_to_paths(svg);
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0], "m21 21-4.34-4.34");
    assert_eq!(paths[1], "M3 11a8 8 0 1 0 16 0a8 8 0 1 0 -16 0Z");
}

#[test]
fn view_box_extraction() {
    assert_eq!(extract_view_box(r#"<svg viewBox="0 0 24 24">"#), Some((24, 24)));
    assert_eq!(extract_view_box("<svg>"), None);
}

#[test]
fn screaming_snake_handles_digit_prefix() {
    assert_eq!(to_screaming_snake("arrow-left"), "ARROW_LEFT");
    assert_eq!(to_screaming_snake("trash-2"), "TRASH_2");
    assert_eq!(to_screaming_snake("1st-place"), "_1ST_PLACE");
}

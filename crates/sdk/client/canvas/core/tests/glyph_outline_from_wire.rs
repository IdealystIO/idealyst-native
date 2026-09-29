//! Its own test binary — its own process — so nothing here calls
//! `FontResource::new` directly: the only way this font comes into being is
//! deserialization, as for a scene painted in another process. If that path
//! did not install the glyph outliner, a CPU renderer would draw no text.

use canvas_core::{expand_glyph_run, Color, DrawOp, FontResource, Paint, PositionedGlyph, Transform};

const INTER: &[u8] = include_bytes!("../../../../../gpu-backend/engine/assets/fonts/Inter-Regular.ttf");

#[test]
fn a_deserialized_font_outlines_glyphs() {
    let json = format!(
        r#"{{"id":7,"index":0,"data":[{}]}}"#,
        INTER.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")
    );
    let font: FontResource = serde_json::from_str(&json).unwrap();
    // Glyph 3 in Inter is a letter with an outline.
    let ops = expand_glyph_run(
        &font,
        &[PositionedGlyph::new(3, Transform::IDENTITY)],
        &Paint::solid(Color::new(0, 0, 0, 255)),
    );
    assert!(ops.iter().any(|op| matches!(op, DrawOp::Fill { .. })), "{ops:?}");
}

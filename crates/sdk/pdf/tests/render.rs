//! End-to-end: a minimal hand-built PDF → `Document::render_page` → assertions
//! on the recorded `canvas_core::Scene`. Proves the `SceneDevice` actually
//! interprets a real PDF content stream into scene ops (vectors + text), not
//! just that the types line up.

use canvas_core::{DrawOp, PaintKind};
use pdf::Document;

/// Assemble a single-page PDF with correct xref byte offsets (so `Pdf::new`
/// parses it without brute-force reconstruction). The page is 200×200 points;
/// `content` is the page's content stream.
fn build_pdf(content: &str) -> Vec<u8> {
    let mut objects: Vec<String> = Vec::new();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_string());
    objects.push("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string());
    objects.push(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
         /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
            .to_string(),
    );
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
    objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    assemble_pdf(&objects)
}

/// Serialize a list of object bodies (1-indexed) into a valid single-`%%EOF`
/// PDF with a correct xref table and `1 0 R` as the catalog root.
fn assemble_pdf(objects: &[String]) -> Vec<u8> {
    let mut pdf = String::from("%PDF-1.7\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
    }
    let xref_offset = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
    pdf.push_str("0000000000 65535 f \n");
    for off in &offsets {
        pdf.push_str(&format!("{off:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF",
        objects.len() + 1
    ));
    pdf.into_bytes()
}

#[test]
fn renders_a_filled_rectangle() {
    // A red rectangle: `1 0 0 rg` (red fill) then `re f`.
    let pdf = build_pdf("1 0 0 rg\n50 60 100 40 re\nf");
    let doc = Document::load(pdf).expect("load PDF");
    assert_eq!(doc.page_count(), 1);

    let page = doc.render_page(0).expect("render page 0");
    assert_eq!(page.width, 200.0);
    assert_eq!(page.height, 200.0);

    // Find the red fill anywhere in the scene (wrapped in Save/Transform/…).
    let red_fill = page.scene.ops().iter().find_map(|op| match op {
        DrawOp::Fill { paint, .. } => match paint.kind {
            PaintKind::Solid(c) if c.r == 255 && c.g == 0 && c.b == 0 => Some(c),
            _ => None,
        },
        _ => None,
    });
    assert!(red_fill.is_some(), "expected a red rectangle fill, scene = {:?}", page.scene.ops());
    assert_eq!(page.warnings.pattern_paints, 0);
}

#[test]
fn renders_text_as_glyphs_or_outlines() {
    // "Hi" in Helvetica at 24pt. With `embed-fonts`, the standard font resolves,
    // so text becomes either a Glyphs run (sfnt/CFF the renderer can atlas) or
    // outline Fills (Type1 fallback) — either way, visible glyph geometry.
    let content = "BT /F1 24 Tf 0 0 0 rg 40 100 Td (Hi) Tj ET";
    let pdf = build_pdf(content);
    let doc = Document::load(pdf).expect("load PDF");
    let page = doc.render_page(0).expect("render page 0");

    let mut glyph_count = 0usize;
    let mut text_fills = 0usize;
    for op in page.scene.ops() {
        match op {
            DrawOp::Glyphs { glyphs, .. } => glyph_count += glyphs.len(),
            DrawOp::Fill { .. } => text_fills += 1,
            _ => {}
        }
    }
    assert!(
        glyph_count >= 2 || text_fills >= 2,
        "expected 2 glyphs (H, i) as a Glyphs run or outline fills; got {glyph_count} glyphs, \
         {text_fills} fills. scene = {:?}",
        page.scene.ops()
    );
}

#[test]
fn soft_masked_fill_becomes_a_mask_group() {
    // A solid black rectangle drawn under an ExtGState soft mask (/SMask). The
    // masked draw must be wrapped in a `MaskGroup` carrying BOTH the fill
    // (content) and the mask's rendered ops, with `luminance: true` (the /SMask
    // is /Luminosity) — so the renderer applies the real mask, not a fade.
    //
    // `q … Q` brackets the masked draw; `/GS1 gs` sets the soft mask; a luminosity
    // mask group `/Mask` is referenced from the ExtGState.
    let content = "q /GS1 gs 0 0 0 rg 50 50 380 600 re f Q";
    let pdf = build_masked_pdf(content);
    let doc = Document::load(pdf).expect("load PDF");
    let page = doc.render_page(0).expect("render");

    let mg = page.scene.ops().iter().find_map(|op| match op {
        DrawOp::MaskGroup { content, mask, luminance, .. } => Some((content, mask, *luminance)),
        _ => None,
    });
    let (content, mask, luminance) =
        mg.unwrap_or_else(|| panic!("masked fill should be a MaskGroup, scene={:?}", page.scene.ops()));
    assert!(luminance, "the /SMask is /Luminosity");
    assert!(!content.is_empty(), "mask group carries the fill content");
    assert!(!mask.is_empty(), "mask group carries the rendered mask ops");
    // A Luminosity mask renders exactly → not counted as an approximation.
    assert_eq!(page.warnings.soft_masks, 0, "luminosity mask is exact, not approximated");
    // The black fill is inside the MaskGroup (masked), not floating at the top
    // level as a solid opaque block.
    let top_level_black = page.scene.ops().iter().any(|op| matches!(op,
        DrawOp::Fill { paint, .. } if matches!(paint.kind, PaintKind::Solid(c) if c.a == 255 && c.r == 0 && c.g == 0 && c.b == 0)));
    assert!(!top_level_black, "masked fill must not leak to the top level");
}

/// Like `build_pdf` but adds an ExtGState `/GS1` with a luminosity `/SMask`
/// (a form XObject filled mid-gray), so the page exercises the soft-mask path.
fn build_masked_pdf(content: &str) -> Vec<u8> {
    let mask_stream = "0.5 g 0 0 480 720 re f";
    let objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 480 720] \
         /Resources << /ExtGState << /GS1 6 0 R >> >> /Contents 5 0 R >>"
            .to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        // 6: ExtGState referencing the soft mask group (7).
        "<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 7 0 R >> >>".to_string(),
        // 7: the mask group — a form XObject with a transparency group.
        format!(
            "<< /Type /XObject /Subtype /Form /FormType 1 /BBox [0 0 480 720] \
             /Group << /Type /Group /S /Transparency /CS /DeviceGray >> /Length {} >>\nstream\n{mask_stream}\nendstream",
            mask_stream.len()
        ),
    ];

    let mut pdf = String::from("%PDF-1.7\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
    }
    let xref_offset = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
    for off in &offsets {
        pdf.push_str(&format!("{off:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF",
        objects.len() + 1
    ));
    pdf.into_bytes()
}

#[test]
fn axial_shading_renders_a_gradient() {
    // A shading-pattern fill: a horizontal axial gradient red(x=0)→blue(x=200).
    // The renderer samples the shading into a texture clipped to the fill path
    // (not a flat color), so the page must carry an image whose pixels gradate.
    let content = "/Pattern cs /P1 scn\n0 0 200 200 re f";
    let pattern = "<< /Type /Pattern /PatternType 2 /Shading << /ShadingType 2 \
/ColorSpace /DeviceRGB /Coords [0 0 200 0] \
/Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> \
/Extend [true true] >> >>";
    let objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
         /Resources << /Pattern << /P1 4 0 R >> >> /Contents 5 0 R >>"
            .to_string(),
        pattern.to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let pdf = assemble_pdf(&objects);
    let page = Document::load(pdf).expect("load").render_page(0).expect("render");

    let img = page
        .scene
        .ops()
        .iter()
        .find_map(|op| match op {
            DrawOp::Image { image, .. } => Some(image.clone()),
            _ => None,
        })
        .expect("shading rendered as a texture image");
    let (w, h) = (img.width, img.height);
    let at = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
    };
    let (left, right) = (at(1, h / 2), at(w - 2, h / 2));
    assert!(left[0] > 180 && left[2] < 70, "left edge red, got {left:?}");
    assert!(right[2] > 180 && right[0] < 70, "right edge blue, got {right:?}");
    assert_eq!(page.warnings.pattern_paints, 0, "shading handled, not skipped");
}

#[test]
fn missing_page_is_an_error() {
    let pdf = build_pdf("1 0 0 rg 0 0 10 10 re f");
    let doc = Document::load(pdf).expect("load PDF");
    assert!(doc.render_page(5).is_err());
}

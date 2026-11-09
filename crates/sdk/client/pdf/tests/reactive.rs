//! `PdfReactive` mounted through the scene registry: an in-place resize keeps
//! the same canvas and repaints the already-interpreted page at the new size,
//! and `on_painted` reports once per (document, box).
//!
//! The renderer is a minimal test handler on the host-mock substrate that does
//! what every real canvas renderer does: paint through `CanvasPrim::paint()` in
//! a subtree-owned effect and report the size layout gives it. (Without an
//! installed scheduler `after_ms` runs its callback synchronously on native, so
//! both the size report and the deferred `on_painted` land at the test's
//! `flush`.)

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use canvas_core::{CanvasPrim, DrawOp, Scene, SizeReporter};
use host_mock::Harness;
use pdf::{PdfPainted, PdfReactive};
use runtime_scene::{MountCx, Realized};
use runtime_vocabulary::caps::DocumentOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_world::signal;

thread_local! {
    static REPORTER: RefCell<Option<SizeReporter>> = const { RefCell::new(None) };
    static LAST_SCENE: RefCell<Option<Scene>> = const { RefCell::new(None) };
    static MOUNTS: Cell<u32> = const { Cell::new(0) };
}

fn harness() -> Harness {
    Harness::with_registry(|r| {
        r.register::<CanvasPrim, _>(|cx: &mut MountCx<'_, _>, prim, _children| {
            MOUNTS.with(|m| m.set(m.get() + 1));
            let node = cx.backend().borrow_mut().create_element("canvas");
            REPORTER.with(|s| *s.borrow_mut() = Some(prim.size_reporter()));
            let prim = prim.clone();
            runtime_world::effect(move || {
                let scene = prim.paint();
                LAST_SCENE.with(|s| *s.borrow_mut() = Some(scene));
            });
            node
        });
    })
}

fn reporter() -> SizeReporter {
    REPORTER.with(|s| s.borrow().clone()).expect("the test renderer stored a reporter")
}

fn last_scene_ops() -> Vec<DrawOp> {
    LAST_SCENE.with(|s| s.borrow().as_ref().expect("painted").ops().to_vec())
}

/// The page scale the painter applied: the second `Transform` (the first is
/// the centering translate).
fn painted_scale(ops: &[DrawOp]) -> f32 {
    let t = ops
        .iter()
        .filter_map(|op| match op {
            DrawOp::Transform(t) => Some(*t),
            _ => None,
        })
        .nth(1)
        .expect("a scale transform");
    t.a
}

fn has_page_fill(ops: &[DrawOp]) -> bool {
    ops.iter().any(|op| matches!(op, DrawOp::Fill { .. }))
}

/// A one-page 200×200 pt PDF holding a filled rectangle.
fn square_pdf() -> Rc<Vec<u8>> {
    pdf_with("1 0 0 rg\n50 60 100 40 re\nf")
}

/// A one-page 200×200 pt PDF with `content` as its content stream.
fn pdf_with(content: &str) -> Rc<Vec<u8>> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    let mut pdf = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
    }
    let xref = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1));
    for off in &offsets {
        pdf.push_str(&format!("{off:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
        objects.len() + 1
    ));
    Rc::new(pdf.into_bytes())
}

/// The bug: zooming a `PdfReactive` meant rebuilding it at the new size, which
/// mounted a fresh canvas — blank until its surface came up and the document
/// was parsed again (40–170 ms). A reactive size now resizes the SAME canvas,
/// and the page repaints at the new scale from the cached interpretation.
#[test]
fn regression_zoom_resizes_the_same_canvas_without_a_blank_frame() {
    MOUNTS.with(|m| m.set(0));
    let h = harness();
    let doc = square_pdf();
    let zoom_slot = Rc::new(Cell::new(None));
    let el = h.world.enter(|| {
        let zoom = signal(1.0f32);
        zoom_slot.set(Some(zoom));
        PdfReactive(
            move || Some(doc.clone()),
            0,
            runtime_core::rx!(200.0 * zoom.get()),
            runtime_core::rx!(200.0 * zoom.get()),
        )
        .into_element()
    });
    let _realized: Realized<u32> = h.mount(el);
    h.flush();
    reporter().report(200.0, 200.0);
    h.flush();
    let ops = last_scene_ops();
    assert!(has_page_fill(&ops), "the page painted");
    assert_eq!(painted_scale(&ops), 1.0);

    // Zoom: the style re-lays-out the same node; layout reports the new box.
    zoom_slot.get().expect("built").set(2.0);
    h.flush();
    reporter().report(400.0, 400.0);
    h.flush();

    assert_eq!(MOUNTS.with(Cell::get), 1, "a zoom must not remount the canvas");
    let ops = last_scene_ops();
    assert!(has_page_fill(&ops), "the page repainted at the new size, not blank");
    assert_eq!(painted_scale(&ops), 2.0, "fitted to the new box");
}

/// Before the renderer reports a laid-out size (the first paint runs before
/// layout), the page is fitted into the requested `width × height`.
#[test]
fn first_paint_fits_the_requested_box_before_layout() {
    let h = harness();
    let doc = square_pdf();
    let el = h.world.enter(|| PdfReactive(move || Some(doc.clone()), 0, 100.0, 300.0).into_element());
    let _realized: Realized<u32> = h.mount(el);
    h.flush();
    assert_eq!(painted_scale(&last_scene_ops()), 0.5, "contain-fit into 100×300");
}

#[test]
fn on_painted_reports_once_per_document_and_box() {
    let h = harness();
    let seen: Rc<RefCell<Vec<PdfPainted>>> = Rc::new(RefCell::new(Vec::new()));
    let doc_slot = Rc::new(Cell::new(None));
    let el = h.world.enter(|| {
        let doc = signal(Some(square_pdf()));
        doc_slot.set(Some(doc));
        let seen = seen.clone();
        PdfReactive(move || doc.get(), 0, 200.0, 200.0)
            .on_painted(move |p| seen.borrow_mut().push(p))
            .into_element()
    });
    let _realized: Realized<u32> = h.mount(el);
    h.flush();
    assert_eq!(*seen.borrow(), vec![PdfPainted { width: 200.0, height: 200.0, scale: 1.0 }]);

    // Layout confirms the same box: a repaint, but nothing new was painted.
    reporter().report(200.0, 200.0);
    h.flush();
    assert_eq!(seen.borrow().len(), 1, "no report for an unchanged box");

    reporter().report(300.0, 150.0);
    h.flush();
    assert_eq!(seen.borrow().last(), Some(&PdfPainted { width: 300.0, height: 150.0, scale: 0.75 }));

    let doc = doc_slot.get().expect("built");
    // A different document (a signal `set` of equal bytes is a no-op).
    doc.set(Some(pdf_with("0 0 1 rg\n10 10 50 50 re\nf")));
    h.flush();
    assert_eq!(seen.borrow().len(), 3, "a new document reports at the current box");

    doc.set(Some(Rc::new(b"not a pdf".to_vec())));
    h.flush();
    assert_eq!(seen.borrow().len(), 3, "a document that fails to load never reports");
}

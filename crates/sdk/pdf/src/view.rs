//! [`Pdf`] — the author-facing element. Renders a PDF page through the `canvas`
//! primitive, so it inherits the GPU pipeline (vello) where available and the
//! CPU fallback (canvas-native) where not. No new per-backend registration: a
//! PDF *is* a canvas scene (CLAUDE.md §3 — peripheral features compose from
//! primitives).

use std::cell::RefCell;
use std::rc::Rc;

use canvas_core::{draw, Canvas, CanvasProps, DrawOp, Scene, Transform};
use runtime_core::{Bound, ExternalHandle, Length, StyleRules, StyleSheet};

use crate::Document;

/// A PDF page to display. Build the element with [`Pdf`].
///
/// The page is interpreted **once** (when the element is built) into a canvas
/// scene; the element then renders that scene at `width` logical pixels, with
/// height derived from the page's aspect ratio.
pub struct PdfView {
    /// The PDF file bytes.
    pub bytes: Vec<u8>,
    /// Zero-based index of the page to display.
    pub page: usize,
    /// Display width in logical pixels. Height follows the page's aspect ratio.
    pub width: f32,
}

impl Default for PdfView {
    fn default() -> Self {
        // 612 pt = US-Letter width, a sensible default page measure.
        Self { bytes: Vec::new(), page: 0, width: 612.0 }
    }
}

/// Render a PDF page as a GPU-accelerated canvas element.
///
/// ```no_run
/// let bytes = std::fs::read("doc.pdf").unwrap();
/// let element = pdf::Pdf(pdf::PdfView { bytes, page: 0, width: 800.0 });
/// ```
///
/// On a load/parse error the element renders empty (and logs at `warn`), so a
/// bad document degrades gracefully rather than panicking.
#[allow(non_snake_case)]
pub fn Pdf(view: PdfView) -> Bound<ExternalHandle<CanvasProps>> {
    let PdfView { bytes, page, width } = view;

    // Interpret the page once. The resulting scene is in page-point coordinates;
    // we scale it to the requested display width below.
    let (ops, page_w, page_h) = match Document::load(bytes).and_then(|d| d.render_page(page)) {
        Ok(rendered) => {
            (rendered.scene.ops().to_vec(), rendered.width.max(1.0), rendered.height.max(1.0))
        }
        Err(err) => {
            log::warn!("pdf: failed to render page {page}: {err}");
            (Scene::new().ops().to_vec(), width, width)
        }
    };

    let scale = width / page_w;
    let display_h = page_h * scale;
    let ops = Rc::new(ops);

    let props = CanvasProps {
        // Replay the page's ops, scaled to the display size, each frame. A static
        // document repaints rarely, so the per-frame clone is acceptable; a
        // future optimization can bake the page into a `DrawOp::LayerCached` for
        // O(1) frames.
        draw: draw(move |s: &mut Scene| {
            s.save();
            if (scale - 1.0).abs() > f32::EPSILON {
                s.transform(Transform::scale(scale, scale));
            }
            for op in ops.iter() {
                s.push_op(op.clone());
            }
            s.restore();
        }),
        ..Default::default()
    };

    // Size the canvas to the page. An explicit `.with_style` replaces the
    // fill-parent default that `Canvas` applies.
    let mut rules = StyleRules::default();
    rules.width = Some(Length::Px(width).into());
    rules.height = Some(Length::Px(display_h).into());
    Canvas(props).with_style(Rc::new(StyleSheet::r#static(rules)))
}

/// One page rendered for [`PdfReactive`]: the scaled ops + page size.
#[derive(Default)]
struct PageCache {
    /// `Rc::ptr_eq` identity of the bytes that produced `ops` — so the page is
    /// re-interpreted only when a *different* document arrives, not every frame.
    bytes: Option<Rc<Vec<u8>>>,
    /// The interpreted page ops and its point size, if a page loaded.
    page: Option<(Vec<DrawOp>, f32, f32)>,
}

/// A PDF element whose document is supplied **reactively** — the viewer for a
/// file the user loads at runtime (e.g. via a file picker). `source` is read
/// inside the canvas's `draw` closure, so changing the signal it reads
/// re-renders the page with no remount (the same reactive-source pattern the
/// `video` SDK uses for a live stream).
///
/// The page is fit (aspect-preserved, centered) into a fixed `width × height`
/// box — the display size is known up front, but the page's own size isn't until
/// it's parsed, so a fixed box avoids a re-layout mid-load. The document is
/// re-interpreted only when `source` yields a *different* `Rc` (identity
/// compared), not on every repaint.
///
/// ```no_run
/// use std::rc::Rc;
/// # use runtime_core::{signal, Signal};
/// let bytes: Signal<Option<Rc<Vec<u8>>>> = signal!(None);
/// let viewer = pdf::PdfReactive(move || bytes.get(), 0, 520.0, 680.0);
/// // later, from a file picker: bytes.set(Some(Rc::new(file_bytes)));
/// ```
#[allow(non_snake_case)]
pub fn PdfReactive<F>(source: F, page: usize, width: f32, height: f32) -> Bound<ExternalHandle<CanvasProps>>
where
    F: Fn() -> Option<Rc<Vec<u8>>> + 'static,
{
    let cache: Rc<RefCell<PageCache>> = Rc::new(RefCell::new(PageCache::default()));

    let props = CanvasProps {
        draw: draw(move |s: &mut Scene| {
            // Reactive read: subscribes the canvas's draw scope to `source`'s
            // signal, so a new document repaints the canvas.
            let bytes = source();
            let mut c = cache.borrow_mut();

            // Re-interpret only when the document identity changed.
            let changed = match (&c.bytes, &bytes) {
                (Some(a), Some(b)) => !Rc::ptr_eq(a, b),
                (None, None) => false,
                _ => true,
            };
            if changed {
                c.bytes = bytes.clone();
                c.page = bytes.and_then(|b| {
                    match Document::load((*b).clone()).and_then(|d| d.render_page(page)) {
                        Ok(r) => Some((r.scene.ops().to_vec(), r.width.max(1.0), r.height.max(1.0))),
                        Err(err) => {
                            log::warn!("pdf: failed to render page {page}: {err}");
                            None
                        }
                    }
                });
            }

            let Some((ops, pw, ph)) = c.page.as_ref() else { return };
            // Fit (contain) the page into the box, centered.
            let scale = (width / pw).min(height / ph);
            let (ox, oy) = ((width - pw * scale) * 0.5, (height - ph * scale) * 0.5);
            s.save();
            s.transform(Transform::translate(ox, oy));
            s.transform(Transform::scale(scale, scale));
            for op in ops {
                s.push_op(op.clone());
            }
            s.restore();
        }),
        ..Default::default()
    };

    let mut rules = StyleRules::default();
    rules.width = Some(Length::Px(width).into());
    rules.height = Some(Length::Px(height).into());
    Canvas(props).with_style(Rc::new(StyleSheet::r#static(rules)))
}

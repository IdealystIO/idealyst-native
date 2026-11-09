//! [`Pdf`] — the author-facing element. Renders a PDF page through the `canvas`
//! primitive, so it inherits the GPU pipeline (vello) where available and the
//! CPU fallback (canvas-native) where not. No new per-backend registration: a
//! PDF *is* a canvas scene (CLAUDE.md §3 — peripheral features compose from
//! primitives).

use std::cell::RefCell;
use std::rc::Rc;

use canvas_core::{draw, Canvas, CanvasBound, CanvasProps, DrawOp, Scene, Transform};
use runtime_core::scheduling::{after_ms, ScheduledTask};
use runtime_core::{Element, IntoElement, Reactive, ScopeAlive};
use runtime_shared::{Length, StyleRules, StyleSheet};
use runtime_vocabulary::style_attach::{IntoStyleProp, StyleProp};

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
pub fn Pdf(view: PdfView) -> CanvasBound {
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


/// One page interpreted for [`PdfReactive`], plus what was last painted.
#[derive(Default)]
struct PageCache {
    /// `Rc::ptr_eq` identity of the bytes that produced `page` — so the page is
    /// re-interpreted only when a *different* document arrives, never on a
    /// repaint or a resize.
    bytes: Option<Rc<Vec<u8>>>,
    /// The interpreted page ops and its point size, if a page loaded.
    page: Option<(Rc<Vec<DrawOp>>, f32, f32)>,
    /// The box the current document was last painted into — `on_painted` fires
    /// once per (document, box), not on every repaint. Cleared when the document
    /// changes.
    painted: Option<(f32, f32)>,
    /// The deferred `on_painted` call. Replacing it cancels a still-pending
    /// earlier one; dropping the canvas (its props own this cache) cancels it.
    pending: Option<ScheduledTask>,
}

/// What [`PdfReactiveBound::on_painted`] reports: the box the page was fitted
/// into, in logical pixels, and the points-to-pixels scale used.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfPainted {
    /// Width of the box the page was fitted into (logical px).
    pub width: f32,
    /// Height of the box the page was fitted into (logical px).
    pub height: f32,
    /// Logical pixels per PDF point the page was drawn at.
    pub scale: f32,
}

/// Fit (contain) a `pw × ph` page into a `bw × bh` box, centered. Returns the
/// scale and the top-left offset.
fn fit_page(pw: f32, ph: f32, bw: f32, bh: f32) -> (f32, f32, f32) {
    let scale = (bw / pw).min(bh / ph);
    (scale, (bw - pw * scale) * 0.5, (bh - ph * scale) * 0.5)
}

/// A PDF element whose document is supplied **reactively** — the viewer for a
/// file the user loads at runtime (e.g. via a file picker). `source` is read
/// inside the canvas's `draw` closure, so changing the signal it reads
/// re-renders the page with no remount (the same reactive-source pattern the
/// `video` SDK uses for a live stream).
///
/// The page is fit (aspect-preserved, centered) into a `width × height` box.
/// The document is re-interpreted only when `source` yields a *different* `Rc`
/// (identity compared), never on a repaint or a resize.
///
/// **Resizing in place.** `width` and `height` take a plain `f32` or a live
/// source (a `Signal<f32>`, `rx!(…)`). Zoom by changing the signal, not by
/// rebuilding the element: the same canvas resizes, the page is not
/// re-interpreted, and the renderer keeps showing the previous frame until the
/// page is painted at the new size. A rebuilt element is a new canvas, which
/// shows nothing until its drawing surface comes up and the document is parsed
/// again. The painter fits the page into the canvas's laid-out size
/// (`Scene::size`), so `.with_style(…)` (which replaces the `width × height`
/// sizing) still fits the page to whatever box layout gives it.
///
/// ```no_run
/// use std::rc::Rc;
/// # use runtime_core::{signal, Signal};
/// let bytes: Signal<Option<Rc<Vec<u8>>>> = signal(None);
/// let zoom: Signal<f32> = signal(1.0);
/// let viewer = pdf::PdfReactive(
///     move || bytes.get(),
///     0,
///     runtime_core::rx!(520.0 * zoom.get()),
///     runtime_core::rx!(680.0 * zoom.get()),
/// )
/// .on_painted(|p: pdf::PdfPainted| log::info!("page painted at {}×{}", p.width, p.height));
/// // later, from a file picker: bytes.set(Some(Rc::new(file_bytes)));
/// ```
#[allow(non_snake_case)]
pub fn PdfReactive<F>(
    source: F,
    page: usize,
    width: impl Into<Reactive<f32>>,
    height: impl Into<Reactive<f32>>,
) -> PdfReactiveBound
where
    F: Fn() -> Option<Rc<Vec<u8>>> + 'static,
{
    PdfReactiveBound {
        source: Rc::new(source),
        page,
        width: width.into(),
        height: height.into(),
        on_painted: None,
        style: None,
    }
}

/// Builder returned by [`PdfReactive`]: `.on_painted(…)` / `.with_style(…)`,
/// then element coercion (`.into_element()`, `Element::from`, or a `ui!`
/// child splat).
pub struct PdfReactiveBound {
    source: Rc<dyn Fn() -> Option<Rc<Vec<u8>>>>,
    page: usize,
    width: Reactive<f32>,
    height: Reactive<f32>,
    on_painted: Option<Rc<dyn Fn(PdfPainted)>>,
    style: Option<StyleProp>,
}

impl PdfReactiveBound {
    /// Call `f` after the page has been painted into a new box: once when a
    /// document first draws, again after each document change, and again after
    /// each resize. A repaint at an unchanged size does not call it, and a
    /// document that fails to load never does.
    ///
    /// "Painted" means the page's scene at that size has been handed to the
    /// canvas renderer, which draws it in the same reactive pass: by the time
    /// `f` runs (deferred to the scheduler's next tick, outside that pass, so
    /// `f` may write signals) the renderer has the new pixels. The one
    /// exception is a GPU canvas's very first paint, which is held until its
    /// drawing surface is ready.
    ///
    /// Only an element built inside a reactive world reports; one painted
    /// outside a world (a wire snapshot) has no scope to defer into.
    pub fn on_painted(mut self, f: impl Fn(PdfPainted) + 'static) -> Self {
        self.on_painted = Some(Rc::new(f));
        self
    }

    /// Attach an author style. REPLACES the `width × height` sizing; the page
    /// is fit into whatever box the style lays out.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }

    fn into_canvas(self) -> CanvasBound {
        let PdfReactiveBound { source, page, width, height, on_painted, style } = self;
        let cache: Rc<RefCell<PageCache>> = Rc::new(RefCell::new(PageCache::default()));
        // The scope the deferred `on_painted` call must still be alive in.
        let alive = runtime_world::is_entered().then(ScopeAlive::current);

        let style = style.unwrap_or_else(|| sizing_style(width.clone(), height.clone()));

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
                    c.painted = None;
                    c.page = bytes.and_then(|b| {
                        match Document::load((*b).clone()).and_then(|d| d.render_page(page)) {
                            Ok(r) => Some((
                                Rc::new(r.scene.ops().to_vec()),
                                r.width.max(1.0),
                                r.height.max(1.0),
                            )),
                            Err(err) => {
                                log::warn!("pdf: failed to render page {page}: {err}");
                                None
                            }
                        }
                    });
                }

                let Some((ops, pw, ph)) = c.page.clone() else { return };
                // The laid-out box once the renderer has reported it; until then
                // (the first paint runs before layout) the requested size.
                let (bw, bh) = match s.size() {
                    (w, h) if w > 0.0 && h > 0.0 => (w, h),
                    _ => (width.get(), height.get()),
                };
                let (scale, ox, oy) = fit_page(pw, ph, bw, bh);
                s.save();
                s.transform(Transform::translate(ox, oy));
                s.transform(Transform::scale(scale, scale));
                for op in ops.iter() {
                    s.push_op(op.clone());
                }
                s.restore();

                if c.painted == Some((bw, bh)) {
                    return;
                }
                c.painted = Some((bw, bh));
                if let (Some(cb), Some(alive)) = (on_painted.clone(), alive.clone()) {
                    let report = PdfPainted { width: bw, height: bh, scale };
                    c.pending = Some(after_ms(0, move || {
                        if alive.get() {
                            cb(report);
                        }
                    }));
                }
            }),
            ..Default::default()
        };

        Canvas(props).with_style(style)
    }
}

/// The default `width × height` sizing: a static sheet when both are fixed, a
/// live one (re-laid-out in place) when either is a reactive source.
fn sizing_style(width: Reactive<f32>, height: Reactive<f32>) -> StyleProp {
    fn box_rules(width: f32, height: f32) -> StyleRules {
        StyleRules {
            width: Some(Length::Px(width).into()),
            height: Some(Length::Px(height).into()),
            ..Default::default()
        }
    }
    if width.is_static() && height.is_static() {
        Rc::new(StyleSheet::r#static(box_rules(width.get(), height.get()))).into_style_prop()
    } else {
        (move || box_rules(width.get(), height.get())).into_style_prop()
    }
}

impl IntoElement for PdfReactiveBound {
    fn into_element(self) -> Element {
        self.into_canvas().into_element()
    }
}

impl From<PdfReactiveBound> for Element {
    fn from(b: PdfReactiveBound) -> Element {
        b.into_element()
    }
}

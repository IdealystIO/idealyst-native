//! Native screen capture for the GTK4 backend — the on-device side of
//! the Robot bridge's `"screenshot"` verb.
//!
//! Every other widget backend already answers this verb (iOS/Android via
//! their layer/canvas snapshot, macOS via `cacheDisplayInRect`, web by
//! rasterizing the live DOM). GTK answered `supports_screenshot() =
//! false`, so `idealyst`'s screenshot tooling — the MCP `screenshot`
//! tool, `robot-test`'s image assertions, parity runs — had no picture of
//! a Linux app at all. That is a verification hole, not a cosmetic gap:
//! this backend's own history is a list of layout bugs that built
//! cleanly, logged nothing, and were only ever caught from a user's
//! screenshot.
//!
//! # Mechanism: re-render through GSK, don't grab the screen
//!
//! GTK4 draws everything client-side into one surface per toplevel via
//! GSK. A [`gtk4::WidgetPaintable`] over the framework root yields the
//! root's `GskRenderNode`, and the toplevel's own `GskRenderer`
//! rasterizes that node to a `GdkTexture` we save as PNG. So the capture
//! is:
//!
//! - **permission-free and in-process** — no compositor screencast
//!   portal, which on Wayland means a user permission prompt and a
//!   desktop-dependent D-Bus dance;
//! - **exactly the app content** — no window decorations, no other
//!   windows on top, no cursor. A capture taken while an unrelated window
//!   overlaps the app still shows the app;
//! - **at the surface's real scale**, because the renderer applies the
//!   widget's scale factor, so the PNG's pixel dimensions are the
//!   HiDPI-correct ones (reported back as such).
//!
//! # Why the capture is FRAME-DEFERRED, not synchronous
//!
//! This is the non-obvious part, and getting it wrong produces a verb
//! that works on some pages and not others.
//!
//! A `GtkWidgetPaintable` does not run the widget's `snapshot()` vfunc on
//! demand — it serves the render node GTK produced for that widget on its
//! last drawn frame, and refreshes off the frame clock. Create one and
//! snapshot it in the same turn and `Snapshot::to_node()` usually returns
//! `None`. Measured over the `idea-ui-docs` catalog: **21 of 49** page
//! captures produced an image, and which ones varied between runs.
//! `gtk_widget_snapshot_child` (the call our own
//! [`IdealystView`](crate::IdealystView) snapshot uses for its children)
//! is no better — it hands back the same cached node, and measured **0 of
//! 3** on the pages probed.
//!
//! What works, every time, is to attach the paintable, force a frame
//! (`queue_draw`), and read it on a later main-loop turn. So capture is
//! asynchronous and delivers through the `done` callback — which is
//! exactly why the capability is defined with a callback instead of a
//! return value (Android's is deferred for the same reason). A caller
//! that screenshots right after driving an interaction then gets a
//! picture of the state it just produced, rather than a coin flip.
//!
//! # Known gaps
//!
//! - Content that does NOT go through GSK is absent: a `graphics` /
//!   canvas leaf rendering into its own `GtkGLArea` context paints black
//!   here. Same class of gap the Apple backends document for a separate
//!   `CAMetalLayer`, and on GTK it is currently moot — the `graphics`
//!   primitive is contract-blocked on this backend and never fires
//!   `on_ready` (see `graphics.rs`).
//! - A window that has never been mapped has no renderer, so capture
//!   fails with a clear error rather than returning a blank image.

use gtk4::glib;
use gtk4::prelude::*;
use runtime_shared::Screenshot;
use std::cell::RefCell;
use std::rc::Rc;

/// How many main-loop turns to wait for the forced frame before giving
/// up. At the 16 ms tick below that is ~1 s — far more than a redraw
/// needs, and short enough that a wedged frame clock reports an error
/// instead of hanging the caller forever.
const MAX_FRAME_WAITS: u32 = 60;

/// Poll interval while waiting for the frame. Matches the host's own
/// 60 Hz pump, so a capture costs at most one extra tick in the common
/// case.
const FRAME_POLL_MS: u64 = 16;

/// Capture `widget` (the framework root) as PNG bytes, delivering the
/// result through `done`.
///
/// Runs on the GTK main thread — GTK4 requires every call on the thread
/// that ran `gtk::init`, and the Robot bridge polls there, so the caller
/// already satisfies it. `done` is invoked on that same thread: inline on
/// the error paths and when the paintable already holds a frame,
/// otherwise from a GLib timeout once GTK has drawn one.
pub(crate) fn capture(widget: &gtk4::Widget, done: Box<dyn FnOnce(Result<Screenshot, String>)>) {
    let (w, h) = (widget.width(), widget.height());
    if w <= 0 || h <= 0 {
        done(Err(format!(
            "framework root has no allocation yet ({w}x{h}) — capture after the \
             first layout pass"
        )));
        return;
    }
    // The renderer belongs to the toplevel and only exists once the window
    // is realized. `native()` walks up to it.
    let Some(renderer) = widget.native().and_then(|native| native.renderer()) else {
        done(Err(
            "no GskRenderer for this widget's toplevel — the window has not been \
             realized (present it and let one frame run first)"
                .to_string(),
        ));
        return;
    };

    let paintable = gtk4::WidgetPaintable::new(Some(widget));
    // Fast path: the paintable may already carry the last drawn frame.
    if let Some(shot) = try_render(&paintable, &renderer, w, h) {
        done(Ok(shot));
        return;
    }

    // Otherwise force a frame and read the paintable once GTK has drawn
    // it. See the module doc — this deferral is the whole reason the verb
    // is callback-shaped.
    widget.queue_draw();
    let done = Rc::new(RefCell::new(Some(done)));
    let waits = Rc::new(RefCell::new(0u32));
    let widget = widget.clone();
    glib::source::timeout_add_local(
        std::time::Duration::from_millis(FRAME_POLL_MS),
        move || {
            // `take()` guards the callback: whichever branch answers
            // consumes it exactly once, so a spurious extra tick can never
            // call it twice.
            if let Some(shot) = try_render(&paintable, &renderer, widget.width(), widget.height()) {
                if let Some(cb) = done.borrow_mut().take() {
                    cb(Ok(shot));
                }
                return glib::ControlFlow::Break;
            }
            let mut n = waits.borrow_mut();
            *n += 1;
            if *n >= MAX_FRAME_WAITS {
                if let Some(cb) = done.borrow_mut().take() {
                    cb(Err(format!(
                        "no frame produced a render node after {MAX_FRAME_WAITS} \
                         main-loop turns — the widget is mapped but nothing painted"
                    )));
                }
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        },
    );
}

/// One attempt: snapshot the paintable and rasterize. `None` means the
/// paintable has no content yet (no frame drawn since it was attached).
fn try_render(
    paintable: &gtk4::WidgetPaintable,
    renderer: &gtk4::gsk::Renderer,
    w: i32,
    h: i32,
) -> Option<Screenshot> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let snapshot = gtk4::Snapshot::new();
    gtk4::gdk::prelude::PaintableExt::snapshot(paintable, &snapshot, w as f64, h as f64);
    let node = snapshot.to_node()?;
    let texture = renderer.render_texture(&node, None);
    // `save_to_png_bytes` returns `Bytes` unconditionally, but an empty
    // buffer would still be an unusable capture.
    let png = texture.save_to_png_bytes();
    if png.is_empty() {
        return None;
    }
    Some(Screenshot {
        // The TEXTURE's dimensions, not the widget's logical size: on a
        // scaled display they differ, and the numbers have to describe the
        // PNG the caller receives.
        width: texture.width().max(0) as u32,
        height: texture.height().max(0) as u32,
        png: png.to_vec(),
    })
}


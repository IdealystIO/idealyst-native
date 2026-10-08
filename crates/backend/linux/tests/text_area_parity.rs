//! `text_area` on GTK must match every other backend in three ways it used
//! not to: it shows its placeholder, it sizes to its content within
//! `min_rows..max_rows`, and it paints the author's colours and font.
//!
//! ## The bugs this pins
//!
//! 1. **Placeholder ignored.** `GtkTextView` has no placeholder property and
//!    `create_text_area` dropped the argument, so an empty Linux text area was
//!    a blank box where web / macOS show their muted hint.
//! 2. **No intrinsic size.** The node had no Taffy measure fn, so an unsized
//!    text area laid out at ZERO height (present, focusable, invisible), and
//!    `min_rows` / `max_rows` did nothing. Every other backend autosizes the
//!    soft-wrap shape to its content, floored at `min_rows` lines and capped
//!    at `max_rows` lines (then the native widget scrolls).
//! 3. **Author colours never applied.** `apply_native_widget_css` only styled
//!    a node whose widget IS a `GtkTextView`/`GtkEntry`, but a text area's node
//!    widget is the `GtkScrolledWindow` around the view — so the desktop
//!    theme's own background and text colour showed instead of the app's.
//!
//! The row math is checked against GTK's OWN line metrics
//! (`gtk_text_view_get_line_yrange`), not against a re-derivation of the
//! backend's measure — the box must agree with what the text view actually
//! draws, or the content would clip or float in a too-tall box.
//!
//! One `#[test]` on purpose: GTK must be driven from the thread that ran
//! `gtk::init`, and cargo gives every test its own thread. The three bugs are
//! sections run in sequence; each reports independently so a failure in one
//! still shows the state of the others.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_shared::{Color, Length, StyleRules, Tokenized};
use runtime_vocabulary::builders::{text_area, text_input, view};

const WIN_W: f32 = 600.0;
const WIN_H: f32 = 500.0;
const BOX_W: f32 = 300.0;

const TEXT_COLOR: &str = "#204080";
const AREA_BG: &str = "#ff0000";
const AREA_FG: &str = "#00c000";
const AREA_FONT_PX: f32 = 20.0;

fn collect<T: IsA<gtk4::Widget>>(root: &gtk4::Widget, out: &mut Vec<T>) {
    if let Ok(w) = root.clone().downcast::<T>() {
        out.push(w);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        collect::<T>(&c, out);
        child = c.next_sibling();
    }
}

fn all<T: IsA<gtk4::Widget>>(root: &gtk4::Widget) -> Vec<T> {
    let mut v = Vec::new();
    collect(root, &mut v);
    v
}

fn lit(hex: &str) -> Option<Tokenized<Color>> {
    Some(Tokenized::Literal(Color(hex.into())))
}

fn rgba(hex: &str) -> [f32; 4] {
    let h = hex.trim_start_matches('#');
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f32 / 255.0;
    [c(0), c(2), c(4), 1.0]
}

/// Commit staged writes, run the layout pass the host's frame pump would run,
/// and give GTK real time to draw (frames need wall-clock time to elapse; a
/// non-blocking spin finishes inside one frame interval).
fn settle(ctx: &gtk4::glib::MainContext, backend: &Rc<RefCell<LinuxBackend>>) {
    newcore::flush_sync();
    for _ in 0..3 {
        for _ in 0..500 {
            ctx.iteration(false);
        }
        if let Ok(mut b) = backend.try_borrow_mut() {
            b.run_layout(WIN_W, WIN_H);
        }
        let until = Instant::now() + Duration::from_millis(60);
        while Instant::now() < until {
            ctx.iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn buffer_text(v: &gtk4::TextView) -> String {
    let b = v.buffer();
    let (s, e) = b.bounds();
    b.text(&s, &e, true).to_string()
}

/// The single label overlaid on a text view, if any.
fn overlay_label(v: &gtk4::TextView) -> Option<gtk4::Label> {
    all::<gtk4::Label>(v.upcast_ref()).into_iter().next()
}

/// Is `w` actually on screen (itself and every ancestor visible)?
fn shown(w: &impl IsA<gtk4::Widget>) -> bool {
    w.as_ref().is_visible() && w.as_ref().is_mapped()
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

/// GTK's own height for buffer line `n` (a hard-break paragraph, all its
/// wrapped rows included) — what the text view really draws.
fn line_height_of(v: &gtk4::TextView, n: i32) -> i32 {
    let it = v.buffer().iter_at_line(n).expect("line exists");
    v.line_yrange(&it).1
}

/// Total height the text view's content occupies, from GTK's own layout.
fn content_height(v: &gtk4::TextView) -> i32 {
    let b = v.buffer();
    let end = b.end_iter();
    let (y, h) = v.line_yrange(&end);
    y + h
}

fn sample(png_like: &(Vec<u8>, usize, i32, i32), x: i32, y: i32) -> [u8; 3] {
    let (bytes, stride, _, _) = png_like;
    // `GdkTexture::download` = CAIRO_FORMAT_ARGB32, native-endian → B,G,R,A
    // on little-endian.
    let o = y as usize * *stride + x as usize * 4;
    [bytes[o + 2], bytes[o + 1], bytes[o]]
}

/// Rasterize `widget` through the toplevel's own GSK renderer. Retries across
/// real frames: a `WidgetPaintable` only carries content once GTK has drawn.
fn render(
    ctx: &gtk4::glib::MainContext,
    widget: &gtk4::Widget,
) -> Option<(Vec<u8>, usize, i32, i32)> {
    let renderer = widget.native()?.renderer()?;
    let paintable = gtk4::WidgetPaintable::new(Some(widget));
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        widget.queue_draw();
        let until = Instant::now() + Duration::from_millis(20);
        while Instant::now() < until {
            ctx.iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
        let (w, h) = (widget.width(), widget.height());
        let snap = gtk4::Snapshot::new();
        gtk4::gdk::prelude::PaintableExt::snapshot(&paintable, &snap, w as f64, h as f64);
        if let Some(node) = snap.to_node() {
            let tex = renderer.render_texture(&node, None);
            let stride = tex.width() as usize * 4;
            let mut buf = vec![0u8; stride * tex.height() as usize];
            tex.download(&mut buf, stride);
            return Some((buf, stride, tex.width(), tex.height()));
        }
    }
    None
}

#[test]
fn regression_linux_text_area_parity() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(WIN_W as i32, WIN_H as i32);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    type Slot = Rc<RefCell<Option<runtime_world::Signal<String>>>>;
    let ph_slot: Slot = Rc::new(RefCell::new(None));
    let auto_slot: Slot = Rc::new(RefCell::new(None));
    let ph_edits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let (ps, asl, pe) = (ph_slot.clone(), auto_slot.clone(), ph_edits.clone());

    let app = newcore::start(backend.clone(), |_r| {}, move || {
        let ph_value = runtime_world::signal(String::new());
        let auto_value = runtime_world::signal(String::new());
        *ps.borrow_mut() = Some(ph_value);
        *asl.borrow_mut() = Some(auto_value);
        let boxed = |extra: StyleRules| StyleRules {
            width: Some(Length::Px(BOX_W).into()),
            ..extra
        };
        view()
            // 0: placeholder text_area (empty, controlled).
            .child(
                text_area()
                    .value(ph_value)
                    .placeholder("Write a note")
                    .on_change(move |v: String| {
                        pe.borrow_mut().push(v.clone());
                        ph_value.set(v);
                    })
                    .style(boxed(StyleRules { color: lit(TEXT_COLOR), ..Default::default() }))
                    .build(),
            )
            // 1: autosize text_area, 3..6 rows.
            .child(
                text_area()
                    .value(auto_value)
                    .on_change(move |v: String| auto_value.set(v))
                    .min_rows(3)
                    .max_rows(6)
                    .style(boxed(StyleRules::default()))
                    .build(),
            )
            // 2: styled text_area — background, text colour, font size.
            .child(
                text_area()
                    .value("Hi".to_string())
                    // Fixed height: this section is about paint, so it must
                    // not depend on the autosize fixed in section 2.
                    .style(boxed(StyleRules {
                        height: Some(Length::Px(80.0).into()),
                        background: lit(AREA_BG),
                        color: lit(AREA_FG),
                        font_size: Some(Tokenized::Literal(Length::Px(AREA_FONT_PX))),
                        ..Default::default()
                    }))
                    .build(),
            )
            // 3: a text_input with the same text colour + placeholder, the
            //    reference for the placeholder's colour on this backend.
            .child(
                text_input()
                    .placeholder("Write a note")
                    .style(boxed(StyleRules { color: lit(TEXT_COLOR), ..Default::default() }))
                    .build(),
            )
            .build()
    });

    window.present();
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..20_000 {
        if window.is_mapped() {
            break;
        }
        ctx.iteration(false);
    }
    if !window.is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    settle(&ctx, &backend);

    let root = window.child().expect("root attached");
    let views = all::<gtk4::TextView>(&root);
    assert_eq!(views.len(), 3, "three text_areas mounted");
    let (ph_view, auto_view, styled_view) = (&views[0], &views[1], &views[2]);
    let entry: gtk4::Entry = all::<gtk4::Entry>(&root).into_iter().next().expect("text_input");
    let ph_value = ph_slot.borrow().expect("slot filled");
    let auto_value = auto_slot.borrow().expect("slot filled");

    let frame_of = |w: &gtk4::Widget| -> (f32, f32, f32, f32) {
        let b = backend.borrow();
        let id = b.node_id_of_widget(w).expect("node id");
        b.node_frame(id).expect("node frame")
    };

    let mut failures: Vec<String> = Vec::new();
    let mut report = |name: &str, r: Result<(), String>| match r {
        Ok(()) => eprintln!("[text_area_parity] {name}: PASS"),
        Err(e) => {
            eprintln!("[text_area_parity] {name}: FAIL — {e}");
            failures.push(format!("{name}: {e}"));
        }
    };

    // ---------------------------------------------------------------------
    // 1. Placeholder.
    // ---------------------------------------------------------------------
    let r = (|| -> Result<(), String> {
        let label = overlay_label(ph_view)
            .ok_or("an empty text_area with a placeholder shows no placeholder at all")?;
        if label.text() != "Write a note" {
            return Err(format!("placeholder text is {:?}", label.text()));
        }
        if !shown(&label) {
            return Err("placeholder exists but is not shown on an empty text_area".into());
        }
        if buffer_text(ph_view) != "" {
            return Err(format!("the placeholder leaked into the value: {:?}", buffer_text(ph_view)));
        }
        if label.can_target() || label.is_focusable() {
            return Err("placeholder must be hit-transparent and unfocusable (caret/focus go to the text view)".into());
        }
        // Sits exactly where the first glyph will: the caret position of the
        // empty buffer, in the text view's widget coordinates.
        let loc = ph_view.iter_location(&ph_view.buffer().start_iter());
        let (cx, cy) = ph_view.buffer_to_window_coords(gtk4::TextWindowType::Widget, loc.x(), loc.y());
        let b = label.compute_bounds(ph_view).ok_or("placeholder has no bounds")?;
        if !near(b.x(), cx as f32, 1.0) || !near(b.y(), cy as f32, 1.0) {
            return Err(format!(
                "placeholder at ({}, {}) but the first glyph sits at ({cx}, {cy})",
                b.x(),
                b.y()
            ));
        }
        // Same colour this backend gives a text_input's placeholder under the
        // same style: the text colour, dimmed.
        let entry_ph = all::<gtk4::Label>(entry.upcast_ref())
            .into_iter()
            .find(|l| l.text() == "Write a note")
            .ok_or("text_input placeholder label not found")?;
        let (a, e) = (label.color(), entry_ph.color());
        let want = rgba(TEXT_COLOR);
        if !(near(a.red(), want[0], 0.02) && near(a.green(), want[1], 0.02) && near(a.blue(), want[2], 0.02)) {
            return Err(format!("placeholder colour {a:?} is not the text colour {TEXT_COLOR} dimmed"));
        }
        if !(a.alpha() < 0.9 && a.alpha() > 0.2) {
            return Err(format!("placeholder alpha {} is not dimmed", a.alpha()));
        }
        if !(near(a.red(), e.red(), 0.02)
            && near(a.green(), e.green(), 0.02)
            && near(a.blue(), e.blue(), 0.02)
            && near(a.alpha(), e.alpha(), 0.02))
        {
            return Err(format!("text_area placeholder {a:?} != text_input placeholder {e:?}"));
        }

        // Typing hides it, and the edit reported is ONLY what was typed.
        let buffer = ph_view.buffer();
        buffer.insert_at_cursor("x");
        settle(&ctx, &backend);
        if shown(&label) {
            return Err("placeholder still shown after typing".into());
        }
        if ph_edits.borrow().as_slice() != ["x".to_string()] {
            return Err(format!("on_change saw {:?}, want [\"x\"]", ph_edits.borrow()));
        }
        // Clearing it brings it back.
        let (s, e) = buffer.bounds();
        let (mut s, mut e) = (s, e);
        buffer.delete(&mut s, &mut e);
        settle(&ctx, &backend);
        if !shown(&label) {
            return Err("placeholder did not come back once the text was cleared".into());
        }
        // Programmatic writes drive it too.
        ph_value.set("from signal".to_string());
        settle(&ctx, &backend);
        if shown(&label) {
            return Err("placeholder shown over a programmatically-set value".into());
        }
        ph_value.set(String::new());
        settle(&ctx, &backend);
        if !shown(&label) {
            return Err("placeholder did not return after the signal cleared the value".into());
        }
        Ok(())
    })();
    report("regression_linux_text_area_placeholder_ignored", r);

    // ---------------------------------------------------------------------
    // 2. Intrinsic size + min_rows / max_rows autosize.
    // ---------------------------------------------------------------------
    let r = (|| -> Result<(), String> {
        let sw = auto_view.parent().ok_or("text view has no scroller")?;
        let line = line_height_of(auto_view, 0) as f32;
        if line <= 0.0 {
            return Err("text view reports no line height".into());
        }
        let height = || frame_of(&sw).3;

        // Empty → floored at min_rows.
        let h = height();
        if !near(h, 3.0 * line, 1.0) {
            return Err(format!("empty, min_rows 3: box {h}px, want 3 x {line} = {}", 3.0 * line));
        }

        // Five lines → grows to fit them exactly (between floor and cap).
        auto_value.set("one\ntwo\nthree\nfour\nfive".to_string());
        settle(&ctx, &backend);
        let (h, c) = (height(), content_height(auto_view) as f32);
        if !near(h, c, 1.0) || !near(h, 5.0 * line, 1.0) {
            return Err(format!("5 lines: box {h}px, text view draws {c}px (5 x {line})"));
        }

        // A soft-wrapped paragraph is measured at the box width: the box
        // matches the text view's own wrapped height.
        let long = "wrap ".repeat(40);
        auto_value.set(long.trim_end().to_string());
        settle(&ctx, &backend);
        let (h, c) = (height(), content_height(auto_view) as f32);
        if c <= line * 1.5 {
            return Err(format!("the long paragraph did not wrap (content {c}px)"));
        }
        if !near(h, c.clamp(3.0 * line, 6.0 * line), 1.0) {
            return Err(format!("wrapped paragraph: box {h}px but the text view draws {c}px"));
        }

        // Typing (the user path, not the signal) grows it too.
        auto_value.set("a".to_string());
        settle(&ctx, &backend);
        let buffer = auto_view.buffer();
        buffer.place_cursor(&buffer.end_iter());
        buffer.insert_at_cursor("\nb\nc\nd");
        settle(&ctx, &backend);
        let h = height();
        if !near(h, 4.0 * line, 1.0) {
            return Err(format!("typing to 4 lines: box {h}px, want {}", 4.0 * line));
        }

        // Ten lines → capped at max_rows, and the rest scrolls.
        auto_value.set((1..=10).map(|n| n.to_string()).collect::<Vec<_>>().join("\n"));
        settle(&ctx, &backend);
        let h = height();
        if !near(h, 6.0 * line, 1.0) {
            return Err(format!("10 lines, max_rows 6: box {h}px, want {}", 6.0 * line));
        }
        let adj = auto_view.vadjustment().ok_or("no vadjustment")?;
        if adj.upper() <= adj.page_size() + line as f64 / 2.0 {
            return Err(format!(
                "content past max_rows must scroll: upper {} page {}",
                adj.upper(),
                adj.page_size()
            ));
        }
        Ok(())
    })();
    report("regression_linux_text_area_no_autosize", r);

    // ---------------------------------------------------------------------
    // 3. Author colours + font reach the text area.
    // ---------------------------------------------------------------------
    let r = (|| -> Result<(), String> {
        // Every check runs (and reports) on its own, so one failure doesn't
        // hide the state of the rest.
        let mut errs: Vec<String> = Vec::new();
        let c = styled_view.color();
        let want = rgba(AREA_FG);
        if !(near(c.red(), want[0], 0.02) && near(c.green(), want[1], 0.02) && near(c.blue(), want[2], 0.02)) {
            errs.push(format!("text colour is {c:?}, want {AREA_FG} — the desktop theme's colour is showing"));
        }
        match styled_view.pango_context().font_description() {
            Some(fd) => {
                let px = fd.size() as f32 / gtk4::pango::SCALE as f32;
                let px = if fd.is_size_absolute() { px } else { px * 96.0 / 72.0 };
                if !near(px, AREA_FONT_PX, 0.6) {
                    errs.push(format!("font size {px}px, want {AREA_FONT_PX}px"));
                }
            }
            None => errs.push("text view has no font description".into()),
        }

        // Pixels: the box's interior is the author's background, not the
        // theme's text-view fill.
        let sw = styled_view.parent().ok_or("text view has no scroller")?;
        let (x, y, w, h) = frame_of(&sw);
        match render(&ctx, &root) {
            None => errs.push("GTK never produced a frame to sample".into()),
            Some(img) => {
                let (px_x, px_y) = ((x + w - 12.0) as i32, (y + h / 2.0) as i32);
                if w <= 12.0 || h <= 0.0 || px_x >= img.2 || px_y >= img.3 {
                    errs.push(format!("text area frame {w}x{h} at ({x},{y}) has no samplable interior"));
                } else {
                    let got = sample(&img, px_x, px_y);
                    if got != [255, 0, 0] {
                        errs.push(format!(
                            "pixel inside the text area is {got:?}, want the author's {AREA_BG} background"
                        ));
                    }
                }
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs.join("; ")) }
    })();
    report("regression_linux_text_area_author_colors_ignored", r);

    app.stop();
    assert!(failures.is_empty(), "text_area parity failures:\n  {}", failures.join("\n  "));
}

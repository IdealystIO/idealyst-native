//! Layout under a reactive region, and native-control intrinsic sizes.
//!
//! # The bugs this pins
//!
//! 1. **A reactive anchor was a real flex box.** Every `if` / `for` region is
//!    swapped under an anchor node (`Host::create_anchor`). This backend
//!    minted it as a plain view — a flex item that sizes to its content — so
//!    a full-window layer holding `if open { panel }` gave the panel a box
//!    of the PANEL's own height to resolve against: `height: 100%` came out
//!    at content height and `position: absolute; top: 0; bottom: 0` at 0
//!    (an absolute child contributes no size, so the anchor was 0 tall). The
//!    same panel without the `if` filled the layer, as it does on web, where
//!    the anchor is `display: contents`.
//! 2. **Native controls had no measure fn.** `button`, `slider`, `toggle` and
//!    `activity_indicator` wrapped their GTK widget without one, so Taffy
//!    gave them no intrinsic size: a button laid out 0×0 (its label still
//!    drew) and a slider got its stretch width but zero height.
//!
//! Plus the guard the anchor fix needs: an anchor directly under a
//! `scroll_view` must not cap the scroll document at one viewport.
//!
//! # Why an integration test with a real window
//!
//! GTK may be driven only from the thread that ran `gtk::init`, so this is
//! its own test binary with exactly one GTK `#[test]`. Skips rather than
//! fails with no display.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_shared::{AlignItems, FlexDirection, Length, Position, StyleRules, StyleSheet};
use runtime_vocabulary::builders::{activity_indicator, button, scroll_view, slider, text, toggle, view};

const WIN_W: f32 = 1024.0;
const WIN_H: f32 = 768.0;

fn sheet(f: impl Fn() -> StyleRules + 'static) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(move |_| f()))
}

fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
    Some(Length::Px(v).into())
}

/// `position: absolute` with every inset 0 — the full-window layer.
fn layer(dir: Option<FlexDirection>) -> StyleRules {
    StyleRules {
        position: Some(Position::Absolute),
        top: px(0.0),
        right: px(0.0),
        bottom: px(0.0),
        left: px(0.0),
        flex_direction: dir,
        ..Default::default()
    }
}

/// Every framed node under `w`: `(gtk type name, label text if any, frame)`.
type Framed = (String, Option<String>, (f32, f32, f32, f32));

fn frames(backend: &LinuxBackend, w: &gtk4::Widget, out: &mut Vec<Framed>) {
    if let Some(f) = backend.node_id_of_widget(w).and_then(|id| backend.node_frame(id)) {
        let label = w
            .downcast_ref::<gtk4::Label>()
            .map(|l| l.text().to_string())
            .or_else(|| w.downcast_ref::<gtk4::Button>().and_then(|b| b.label()).map(|s| s.to_string()));
        out.push((w.type_().name().to_string(), label, f));
    }
    let mut c = w.first_child();
    while let Some(ch) = c {
        frames(backend, &ch, out);
        c = ch.next_sibling();
    }
}

/// The frame of the view directly holding the label `text`.
fn parent_frame_of_label(backend: &LinuxBackend, root: &gtk4::Widget, text: &str) -> (f32, f32, f32, f32) {
    fn find(w: &gtk4::Widget, text: &str) -> Option<gtk4::Widget> {
        if w.downcast_ref::<gtk4::Label>().is_some_and(|l| l.text() == text) {
            return Some(w.clone());
        }
        let mut c = w.first_child();
        while let Some(ch) = c {
            if let Some(f) = find(&ch, text) {
                return Some(f);
            }
            c = ch.next_sibling();
        }
        None
    }
    let label = find(root, text).unwrap_or_else(|| panic!("label {text:?} not mounted"));
    let parent = label.parent().expect("label has a parent view");
    backend
        .node_id_of_widget(&parent)
        .and_then(|id| backend.node_frame(id))
        .expect("parent view is a framed node")
}

fn frame_of_type(all: &[Framed], ty: &str) -> (f32, f32, f32, f32) {
    all.iter()
        .find(|(t, _, _)| t == ty)
        .unwrap_or_else(|| panic!("no {ty} mounted"))
        .2
}

#[test]
fn regression_reactive_region_is_layout_transparent_and_controls_measure() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    let window = gtk4::Window::new();
    window.set_default_size(WIN_W as i32, WIN_H as i32);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let _app = newcore::start(backend.clone(), |_r| {}, move || {
        view()
            // Panel A: `height: 100%` under an `if` in an inset-sized layer.
            // `FlexStart` so the controls' widths come from their measure,
            // not from cross-axis stretch.
            .child(
                view()
                    .style(sheet(|| layer(None)))
                    .child(move || {
                        view()
                            .style(sheet(|| StyleRules {
                                width: px(300.0),
                                height: Some(Length::Percent(100.0).into()),
                                align_items: Some(AlignItems::FlexStart),
                                ..Default::default()
                            }))
                            .child(text().content("panel a").build())
                            .child(button().label("Hide").build())
                            .child(slider().style(sheet(|| StyleRules { width: px(276.0), ..Default::default() })).build())
                            .child(toggle().build())
                            .child(activity_indicator().build())
                            .build()
                    })
                    .build(),
            )
            // Panel B: pinned by `top: 0; bottom: 0` under an `if`.
            .child(
                view()
                    .style(sheet(|| layer(None)))
                    .child(move || {
                        view()
                            .style(sheet(|| StyleRules {
                                position: Some(Position::Absolute),
                                top: px(0.0),
                                bottom: px(0.0),
                                right: px(0.0),
                                width: px(310.0),
                                ..Default::default()
                            }))
                            .child(text().content("panel b").build())
                            .build()
                    })
                    .build(),
            )
            // Panel C: row-stretch under an `if`.
            .child(
                view()
                    .style(sheet(|| layer(Some(FlexDirection::Row))))
                    .child(move || {
                        view()
                            .style(sheet(|| StyleRules { width: px(320.0), ..Default::default() }))
                            .child(text().content("panel c").build())
                            .build()
                    })
                    .build(),
            )
            // An `if` directly inside a scroll view, holding 3000+ px of
            // content: the document must still be that tall.
            .child(
                scroll_view()
                    .style(sheet(|| StyleRules {
                        position: Some(Position::Absolute),
                        left: px(0.0),
                        top: px(0.0),
                        width: px(200.0),
                        height: px(300.0),
                        ..Default::default()
                    }))
                    .child(move || {
                        // Content whose CHILDREN make it tall, as real page
                        // content does (an explicit height on a direct flex
                        // child would shrink to the viewport under flexbox's
                        // automatic minimum, anchor or not).
                        view()
                            .child(text().content("tall").build())
                            .child(view().style(sheet(|| StyleRules { height: px(3000.0), ..Default::default() })).build())
                            .build()
                    })
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
    for _ in 0..2_000 {
        ctx.iteration(false);
    }
    // host-gtk's frame-clock pump stands in here: under `cargo test` the
    // window gets no frame callbacks, so drive the layout pass directly.
    backend.borrow_mut().run_layout(WIN_W, WIN_H);
    for _ in 0..500 {
        ctx.iteration(false);
    }

    let b = backend.borrow();
    let root = window.child().expect("root attached");

    // --- 1. Panels under an `if` resolve against the layer ---------------
    let a = parent_frame_of_label(&b, &root, "panel a");
    assert_eq!((a.2, a.3), (300.0, WIN_H), "`height: 100%` under an `if` must fill the layer: {a:?}");
    let pb = parent_frame_of_label(&b, &root, "panel b");
    assert_eq!(pb.3, WIN_H, "`top: 0; bottom: 0` under an `if` must span the layer: {pb:?}");
    assert_eq!(pb.0, WIN_W - 310.0, "`right: 0` resolves against the layer, not the anchor: {pb:?}");
    let c = parent_frame_of_label(&b, &root, "panel c");
    assert_eq!(c.3, WIN_H, "a row layer must stretch a panel under an `if`: {c:?}");

    // --- 2. Native controls take their intrinsic size --------------------
    let mut all = Vec::new();
    frames(&b, &root, &mut all);
    let btn = frame_of_type(&all, "GtkButton");
    assert!(btn.2 > 0.0 && btn.3 > 0.0, "a button must measure its label: {btn:?}");
    let sl = frame_of_type(&all, "GtkScale");
    assert!(sl.2 == 276.0 && sl.3 > 0.0, "a slider must have a height: {sl:?}");
    let sw = frame_of_type(&all, "GtkSwitch");
    assert!(sw.2 > 0.0 && sw.3 > 0.0, "a toggle must measure: {sw:?}");
    let sp = frame_of_type(&all, "GtkSpinner");
    assert_eq!((sp.2, sp.3), (16.0, 16.0), "a small activity indicator is 16 px: {sp:?}");

    // --- 3. An anchor under a scroll view keeps the scroll extent --------
    let scrolled = {
        fn find(w: &gtk4::Widget) -> Option<gtk4::ScrolledWindow> {
            if let Some(s) = w.downcast_ref::<gtk4::ScrolledWindow>() {
                return Some(s.clone());
            }
            let mut c = w.first_child();
            while let Some(ch) = c {
                if let Some(f) = find(&ch) {
                    return Some(f);
                }
                c = ch.next_sibling();
            }
            None
        }
        find(&root).expect("scroll view mounted")
    };
    let document = scrolled.child().expect("scroll document");
    let (_, doc_h, _, _) = document.measure(gtk4::Orientation::Vertical, -1);
    assert!(
        doc_h >= 3000,
        "the scroll document measures {doc_h} px; an anchor framed to the 300 px viewport \
         capped it, so the page could not scroll to its 3000 px of content",
    );
}

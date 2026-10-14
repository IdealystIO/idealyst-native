//! Regression: a subtree the framework DISCARDS must give back every piece
//! of per-node state the GTK backend kept for it.
//!
//! ## The bug this pins
//!
//! The backend keeps a dozen side tables keyed by node id — `nodes` (which
//! owns the Taffy node and a strong ref to the widget), `image_nodes`,
//! `input_controllers`, `echo_guards`, `native_styles`, `text_areas`,
//! `sticky_nodes`, `state_controllers`, the portal maps, … — and nothing
//! ever removed an entry. `clear_children` only detaches (it has to: a
//! `LazyPersistent` navigator screen is detached and re-inserted on the
//! SAME nodes), `remove_child` is a no-op on this anchored host, and
//! `Host::release_subtree` — the one call that means "gone for good" —
//! fell through to the trait's no-op default. So every `when` branch swap,
//! keyed-list rebuild and navigator pop leaked its whole subtree: table
//! entries, GTK widgets (held alive by `nodes`), and Taffy nodes, which
//! `clear_children` merely unparented into extra roots.
//!
//! ## Shape
//!
//! A structural hole swaps a heavy branch (views, text, an image, a
//! text_input with handlers, a text_area, a hover-styled view, a scroll
//! view holding a sticky child, a viewport portal, and a virtualizer with
//! mounted rows) in and out repeatedly, through the real scene + vocabulary
//! (so the framework's own call order — scope drop, `release_portal`,
//! `release_virtualizer`, `release_subtree`, `clear_children` — is what
//! runs). After every swap back to the light branch, every table and the
//! live Taffy node count must equal the baseline taken in that same state.
//! Before the fix each cycle added the heavy branch's entries on top.
//!
//! One GTK `#[test]` in this binary: GTK may only be driven from the thread
//! that ran `gtk::init`, and cargo gives each test its own thread.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_shared::primitives::portal::{PortalTarget, ViewportPlacement};
use runtime_shared::primitives::virtualizer::ItemSize;
use runtime_shared::{Length, Position, StyleRules, StyleSheet};
use runtime_vocabulary::builders::{
    image, portal, scroll_view, text, text_area, text_input, view, virtualizer,
};

const WIN_W: f32 = 640.0;
const WIN_H: f32 = 480.0;
const CYCLES: usize = 4;

/// A valid 2x2 RGB PNG: decodes inside `create_image`, so the image node
/// reaches a settled outcome (its `image_nodes` entry is populated) without
/// any network.
const PNG_2X2: &str = "data:image/png;base64,\
    iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4\
    z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==";

/// Hover variant → the node gets `state_controllers`.
fn hoverable() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_| StyleRules {
            width: Some(Length::Px(100.0).into()),
            height: Some(Length::Px(20.0).into()),
            ..Default::default()
        })
        .variant("__state_hovered", "on", |_| StyleRules {
            width: Some(Length::Px(120.0).into()),
            ..Default::default()
        }),
    )
}

fn heavy_branch() -> runtime_scene::Element {
    view()
        .style(hoverable())
        .child(text().content("hello").build())
        .child(image().src(PNG_2X2).build())
        .child(
            text_input()
                .value("typed")
                .on_focus(|_| {})
                .on_key_down(|_| runtime_shared::primitives::key::KeyOutcome::Default)
                .style(StyleRules {
                    width: Some(Length::Px(200.0).into()),
                    ..Default::default()
                })
                .build(),
        )
        .child(text_area().value("multi\nline").build())
        .child(
            view()
                .child(text().content("nested").build())
                .child(view().build())
                .build(),
        )
        .child(
            scroll_view()
                .style(StyleRules {
                    height: Some(Length::Px(60.0).into()),
                    ..Default::default()
                })
                .child(
                    view()
                        .style(StyleRules {
                            position: Some(Position::Sticky),
                            top: Some(Length::Px(0.0).into()),
                            height: Some(Length::Px(10.0).into()),
                            ..Default::default()
                        })
                        .build(),
                )
                .child(text().content("scrolled").build())
                .build(),
        )
        .child(
            portal(PortalTarget::Viewport(ViewportPlacement::Center))
                .child(text().content("in a portal").build())
                .build(),
        )
        .child(
            virtualizer(
                || 5,
                |i| i as u64,
                ItemSize::Known(Rc::new(|_| 20.0)),
                |i| view().child(text().content(format!("row {i}")).build()).build(),
            )
            .style(StyleRules {
                // `min_height`: the heavy column overflows the window, and a
                // scroll container's min-content is 0, so without it flex
                // shrink squeezes the list to 0 high and no row ever mounts.
                height: Some(Length::Px(100.0).into()),
                min_height: Some(Length::Px(100.0).into()),
                width: Some(Length::Px(200.0).into()),
                ..Default::default()
            })
            .build(),
        )
        .build()
}

/// Drain the main loop and run a layout pass, as host-gtk's 16 ms pump
/// would (under `cargo test` the window gets no frame callbacks).
fn settle(ctx: &gtk4::glib::MainContext, backend: &Rc<RefCell<LinuxBackend>>) {
    for _ in 0..2_000 {
        ctx.iteration(false);
    }
    if let Ok(mut b) = backend.try_borrow_mut() {
        b.run_layout(WIN_W, WIN_H);
    }
    for _ in 0..2_000 {
        ctx.iteration(false);
    }
}

/// Labels the virtualizer's rows render (`row N`) currently in the window.
fn count_row_labels(w: &gtk4::Widget) -> usize {
    let own = w
        .downcast_ref::<gtk4::Label>()
        .is_some_and(|l| l.text().starts_with("row ")) as usize;
    let mut n = own;
    let mut c = w.first_child();
    while let Some(x) = c {
        n += count_row_labels(&x);
        c = x.next_sibling();
    }
    n
}

fn counts(backend: &Rc<RefCell<LinuxBackend>>) -> Vec<(&'static str, usize)> {
    backend.borrow().per_node_state_counts_for_test()
}

fn count_of(c: &[(&'static str, usize)], name: &str) -> usize {
    c.iter().find(|(n, _)| *n == name).map(|(_, v)| *v).unwrap()
}

#[test]
fn regression_linux_released_subtree_frees_per_node_state() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(WIN_W as i32, WIN_H as i32);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let heavy_slot: Rc<RefCell<Option<runtime_world::Signal<bool>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = heavy_slot.clone();
    let app = newcore::start(backend.clone(), |_r| {}, move || {
        let heavy = runtime_world::signal(false);
        *slot_for_build.borrow_mut() = Some(heavy);
        view()
            .child(move || {
                if heavy.get() {
                    heavy_branch()
                } else {
                    text().content("idle").build()
                }
            })
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

    let heavy = heavy_slot.borrow().expect("signal created in build");
    let baseline = counts(&backend);

    for cycle in 0..CYCLES {
        heavy.set(true);
        newcore::flush_sync();
        settle(&ctx, &backend);
        // Rows mount from the list's adjustment signals, i.e. once GTK has
        // ALLOCATED it — a frame-clock tick, which needs wall time to pass,
        // not just main-loop spins. Wait for it (bounded) rather than
        // racing it.
        for _ in 0..400 {
            if count_row_labels(window.upcast_ref()) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            settle(&ctx, &backend);
        }
        let mounted = counts(&backend);
        assert!(
            count_row_labels(window.upcast_ref()) > 0,
            "cycle {cycle}: the virtualizer mounted no rows, so recycled-cell \
             teardown is not being exercised (did the list lay out at 0 height?)",
        );
        // Guard the test against going vacuous: the heavy branch must
        // really have populated each table it is meant to exercise.
        for table in [
            "nodes",
            "layout_nodes",
            "image_nodes",
            "input_controllers",
            "echo_guards",
            "native_styles",
            "text_areas",
            "sticky_nodes",
            "state_controllers",
            "portal_roots",
            "virtualizers",
        ] {
            assert!(
                count_of(&mounted, table) > count_of(&baseline, table),
                "cycle {cycle}: the heavy branch never populated `{table}` \
                 (baseline {baseline:?}, mounted {mounted:?}) — the test no \
                 longer exercises its teardown",
            );
        }

        heavy.set(false);
        newcore::flush_sync();
        settle(&ctx, &backend);
        let after = counts(&backend);
        assert_eq!(
            after, baseline,
            "cycle {cycle}: discarding the heavy branch must return every \
             per-node table and the live Taffy node count to baseline",
        );
    }

    // The window is still a working tree after all that freeing.
    assert!(window.child().is_some());
    app.stop();
}

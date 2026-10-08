//! `text_input(disabled = …)` / `text_area(disabled = …)` /
//! `toggle(disabled = …)` / `slider(disabled = …)` must be natively inert on
//! GTK.
//!
//! ## The bug this pins
//!
//! `StyleOps::set_disabled` — the host half of the `disabled` prop — was
//! never implemented on this backend, so it was the trait's no-op default.
//! The vocabulary's edit gate still dropped the `on_change`, so nothing
//! looked wrong in a unit test: but on screen a disabled `Field` was a Tab
//! stop you could click into and type into, the caret blinked, the focus
//! ring lit, and every keystroke was silently thrown away. Web, macOS, iOS
//! and Android all refuse focus on a disabled field. A disabled `button`
//! was worse: its mount path has no press-block flag (it relies on the
//! native disable), so it stayed clickable.
//!
//! `toggle` / `slider` had a second gap: the primitives had no `disabled`
//! input at all, so `set_disabled` never reached a `GtkSwitch` / `GtkScale`
//! and a disabled switch or slider stayed fully live. They are also
//! controlled here — `update_toggle_value` / `update_slider_value` were the
//! trait's no-op defaults on this backend, so the widgets never followed
//! their signals.
//!
//! The test drives the real path — a signal flips `disabled`, the
//! vocabulary calls `set_disabled`, GTK must refuse — and asserts on GTK's
//! own focus machinery (`grab_focus`, Tab traversal, the window's focus
//! widget), not on the backend having been called.
//!
//! One `#[test]` on purpose: GTK must be driven from the thread that ran
//! `gtk::init`, and cargo gives every test its own thread.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_vocabulary::builders::{button, slider, text_area, text_input, toggle, view};

fn find<T: IsA<gtk4::Widget>>(root: &gtk4::Widget) -> Option<T> {
    if let Ok(w) = root.clone().downcast::<T>() {
        return Some(w);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(f) = find::<T>(&c) {
            return Some(f);
        }
        child = c.next_sibling();
    }
    None
}

/// Commit staged signal writes (a `Signal::set` from test code stages; the
/// flush is not implicit — in a real app the input dispatch that made the
/// write queues it), then drain GTK's main loop.
fn pump(ctx: &gtk4::glib::MainContext) {
    newcore::flush_sync();
    for _ in 0..2_000 {
        ctx.iteration(false);
    }
}

/// `true` when the window's focus widget is `w` or inside it (a
/// `GtkEntry` focuses through its internal `GtkText`).
fn focus_within(window: &gtk4::Window, w: &impl IsA<gtk4::Widget>) -> bool {
    let w = w.as_ref();
    gtk4::prelude::RootExt::focus(window)
        .is_some_and(|f| &f == w || f.is_ancestor(w))
}

#[test]
fn regression_linux_disabled_text_input_still_editable() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let window = gtk4::Window::new();
    window.set_default_size(600, 400);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let slot: Rc<RefCell<Option<runtime_world::Signal<bool>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let edits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let e1 = edits.clone();
    let e2 = edits.clone();
    let e3 = edits.clone();
    let e4 = edits.clone();
    type Controls = (runtime_world::Signal<bool>, runtime_world::Signal<f32>);
    let values: Rc<RefCell<Option<Controls>>> = Rc::new(RefCell::new(None));
    let values_for_build = values.clone();
    let app = newcore::start(backend.clone(), |_r| {}, move || {
        let off = runtime_world::signal(false);
        *slot_for_build.borrow_mut() = Some(off);
        let on = runtime_world::signal(false);
        let level = runtime_world::signal(0.25f32);
        *values_for_build.borrow_mut() = Some((on, level));
        view()
            .child(
                text_input()
                    .value("field")
                    .on_change(move |v| e1.borrow_mut().push(v))
                    .disabled(off)
                    .build(),
            )
            .child(
                text_area()
                    .value("notes")
                    .on_change(move |v| e2.borrow_mut().push(v))
                    .disabled(off)
                    .build(),
            )
            // `button` has NO press-block flag in its mount path (that is
            // the bare-`pressable` path): it relies on the native disable
            // alone, so pre-fix a disabled button stayed fully clickable
            // and focusable here.
            .child(
                button()
                    .label("Go")
                    .on_press(|| {})
                    .disabled(off)
                    .build(),
            )
            .child(
                toggle()
                    .value(on)
                    .on_change(move |v| e3.borrow_mut().push(format!("toggle {v}")))
                    .disabled(off)
                    .build(),
            )
            .child(
                slider()
                    .value(level)
                    .range(0.0, 1.0)
                    .on_change(move |v| e4.borrow_mut().push(format!("slider {v}")))
                    .disabled(off)
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
    pump(&ctx);

    let root = window.child().expect("root attached");
    let entry: gtk4::Entry = find(&root).expect("text_input is a GtkEntry");
    let text_view: gtk4::TextView = find(&root).expect("text_area wraps a GtkTextView");
    let go: gtk4::Button = find(&root).expect("button is a GtkButton");
    let off = slot.borrow().expect("build filled the slot");
    let switch: gtk4::Switch = find(&root).expect("toggle is a GtkSwitch");
    let scale: gtk4::Scale = find(&root).expect("slider is a GtkScale");
    let (on, level) = values.borrow().expect("build filled the values");

    // Controlled toggle / slider follow their signals, and the backend's
    // own write is not echoed back as a user change.
    on.set(true);
    level.set(0.75);
    pump(&ctx);
    assert!(switch.is_active(), "the switch follows its signal");
    assert_eq!(scale.value(), 0.75, "the slider follows its signal");
    assert!(edits.borrow().is_empty(), "a programmatic write is not reported: {:?}", edits.borrow());
    // A user flip / drag reaches the author while enabled (the same GTK
    // signals a click / drag emits).
    switch.set_active(false);
    scale.set_value(0.5);
    pump(&ctx);
    assert_eq!(
        *edits.borrow(),
        vec!["toggle false".to_string(), "slider 0.5".to_string()],
        "enabled controls report changes"
    );
    edits.borrow_mut().clear();

    // Enabled: both take focus — the baseline, so the disabled assertions
    // below are about `disabled` and not about an unfocusable environment.
    assert!(entry.grab_focus(), "an enabled text_input takes focus");
    assert!(focus_within(&window, &entry));
    assert!(text_view.grab_focus(), "an enabled text_area takes focus");
    assert!(focus_within(&window, &text_view));

    // Disable while the TEXT AREA holds focus: focus must be released, not
    // left on an inert field (where its focus ring would stay lit).
    off.set(true);
    pump(&ctx);
    assert!(
        !focus_within(&window, &text_view),
        "a text_area that goes disabled must drop the focus it holds",
    );
    assert!(!entry.is_sensitive() && !text_view.is_sensitive(), "GTK's own disabled state");
    assert!(!entry.grab_focus(), "a disabled text_input must refuse focus (click / programmatic)");
    assert!(!text_view.grab_focus(), "a disabled text_area must refuse focus");
    assert!(!focus_within(&window, &entry) && !focus_within(&window, &text_view));
    // The button: GTK's own disabled state, and not a focus target (so no
    // Enter/Space). Asserted on sensitivity rather than by firing input:
    // GTK4 cannot synthesize a pointer event, and `gtk_widget_activate`
    // is a programmatic path that does NOT check sensitivity (verified —
    // it fires `clicked` on an insensitive button), so it would prove
    // nothing about what a user can do. Real pointer/key events never
    // reach the controllers of an insensitive widget.
    assert!(!go.is_sensitive(), "a disabled button is insensitive");
    assert!(!go.grab_focus(), "a disabled button is not a focus target");
    // Toggle / slider: GTK's own disabled state (pre-fix they stayed
    // sensitive — `set_disabled` never reached them) and not focus targets.
    // A change that still arrives (programmatic, or in flight when the
    // control went inert) is dropped by the vocabulary's gate.
    assert!(!switch.is_sensitive(), "a disabled toggle is insensitive");
    assert!(!scale.is_sensitive(), "a disabled slider is insensitive");
    assert!(!switch.grab_focus() && !scale.grab_focus(), "disabled controls refuse focus");
    switch.set_active(true);
    scale.set_value(0.9);
    pump(&ctx);

    // Keyboard traversal skips them: with only the two disabled fields in
    // the window, Tab finds nothing to land on inside either.
    gtk4::prelude::RootExt::set_focus(&window, None::<&gtk4::Widget>);
    window.child_focus(gtk4::DirectionType::TabForward);
    assert!(
        !focus_within(&window, &entry) && !focus_within(&window, &text_view),
        "Tab must skip a disabled field",
    );

    // Disable while the ENTRY holds focus (re-enable, focus, disable).
    off.set(false);
    pump(&ctx);
    assert!(entry.grab_focus(), "re-enabled: focus works again (a live toggle, not a latch)");
    assert!(go.is_sensitive(), "re-enabled: the button is live again");
    off.set(true);
    pump(&ctx);
    assert!(
        !focus_within(&window, &entry),
        "a text_input that goes disabled must drop the focus it holds",
    );

    assert!(edits.borrow().is_empty(), "no edit was reported along the way");
    app.stop();
}

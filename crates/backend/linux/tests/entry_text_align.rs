//! `text_align` on a `text_input` (`Field(text_align = …)`): the `GtkEntry` paints
//! its own text, so the style has to reach `GtkEditable:xalign`. It didn't, and a
//! figure typed in a right-aligned column sat at the left. A style going back to
//! none goes back to the start.
//!
//! One test on purpose: GTK must be driven from the thread that ran `gtk::init`.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_shared::{Length, StyleRules, TextAlign};
use runtime_vocabulary::builders::{text_input, view};

fn entries(root: &gtk4::Widget, out: &mut Vec<gtk4::Entry>) {
    if let Ok(e) = root.clone().downcast::<gtk4::Entry>() {
        out.push(e);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        entries(&c, out);
        child = c.next_sibling();
    }
}

#[test]
fn regression_gtk_entry_ignored_text_align() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    let window = gtk4::Window::new();
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let slot: Rc<RefCell<Option<runtime_world::Signal<Option<TextAlign>>>>> = Rc::default();
    let s = slot.clone();
    let _app = newcore::start(backend.clone(), |_r| {}, move || {
        let align = runtime_world::signal(Some(TextAlign::Right));
        *s.borrow_mut() = Some(align);
        view()
            .child(
                text_input()
                    .value("12".to_string())
                    .style(move || StyleRules {
                        width: Some(Length::Px(300.0).into()),
                        text_align: align.get(),
                        ..Default::default()
                    })
                    .build(),
            )
            .build()
    });
    window.present();
    newcore::flush_sync();

    let mut found = Vec::new();
    entries(window.upcast_ref(), &mut found);
    let entry = found.pop().expect("the text_input's GtkEntry");
    let align = slot.borrow().expect("slot filled");

    assert_eq!(EditableExt::alignment(&entry), 1.0, "right");
    for (value, xalign) in [(Some(TextAlign::Center), 0.5), (Some(TextAlign::Left), 0.0), (Some(TextAlign::Right), 1.0), (None, 0.0)] {
        align.set(value);
        newcore::flush_sync();
        assert_eq!(EditableExt::alignment(&entry), xalign, "{value:?}");
    }
    println!("ran: GtkEntry xalign follows text_align");
}

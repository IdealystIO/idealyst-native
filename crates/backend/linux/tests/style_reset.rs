//! A style that drops a prop resets it on GTK. Linux already resolves each style
//! from scratch against `TextPaint` and the ancestor chain, so this pins that: it is
//! the backend the others are measured against, and a stateful path here would bring
//! the bug back.
//!
//! One test on purpose: GTK must be driven from the thread that ran `gtk::init`.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_shared::{Color, FontWeight, Length, ObjectFit, StyleRules, Tokenized};
use runtime_vocabulary::builders::{image, text, view};

fn find<T: IsA<gtk4::Widget>>(root: &gtk4::Widget, out: &mut Vec<T>) {
    if let Ok(w) = root.clone().downcast::<T>() {
        out.push(w);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        find(&c, out);
        child = c.next_sibling();
    }
}

const SVG: &str = "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'/>";

#[test]
fn dropped_style_props_reset_on_gtk() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    let window = gtk4::Window::new();
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));

    let slot: Rc<RefCell<Option<runtime_world::Signal<bool>>>> = Rc::default();
    let s = slot.clone();
    let _app = newcore::start(backend.clone(), |_r| {}, move || {
        let styled = runtime_world::signal(true);
        *s.borrow_mut() = Some(styled);
        view()
            .child(
                text()
                    .content("Twelve")
                    .style(move || {
                        if !styled.get() {
                            return StyleRules::default();
                        }
                        StyleRules {
                            color: Some(Tokenized::Literal(Color("#ff0000".into()))),
                            font_size: Some(Length::Px(40.0).into()),
                            font_weight: Some(FontWeight::Bold),
                            line_height: Some(60.0.into()),
                            opacity: Some(0.5.into()),
                            ..Default::default()
                        }
                    })
                    .build(),
            )
            .child(
                image()
                    .src(SVG)
                    .style(move || StyleRules {
                        object_fit: styled.get().then_some(ObjectFit::Cover),
                        ..Default::default()
                    })
                    .build(),
            )
            .build()
    });
    window.present();
    newcore::flush_sync();

    let root: gtk4::Widget = window.clone().upcast();
    let mut labels: Vec<gtk4::Label> = Vec::new();
    find(&root, &mut labels);
    let label = labels.into_iter().find(|l| l.text() == "Twelve").expect("the text mounted");
    let mut pictures: Vec<gtk4::Picture> = Vec::new();
    find(&root, &mut pictures);
    let picture = pictures.pop().expect("the image mounted");
    let attrs = |label: &gtk4::Label| label.attributes().map(|a| a.to_string()).unwrap_or_default();

    let styled_attrs = attrs(&label);
    assert!(styled_attrs.contains("foreground"), "a stated colour, got {styled_attrs}");
    assert!(styled_attrs.contains("Bold 40px"), "a stated weight, got {styled_attrs}");
    assert!(styled_attrs.contains("absolute-line-height"), "a stated line height, got {styled_attrs}");
    // GTK keeps opacity in 8 bits.
    assert!((label.opacity() - 0.5).abs() < 0.01, "a stated opacity, got {}", label.opacity());
    assert_eq!(picture.content_fit(), gtk4::ContentFit::Cover);

    slot.borrow().as_ref().expect("the signal").set(false);
    newcore::flush_sync();

    let dropped = attrs(&label);
    assert!(!dropped.contains("#ffff00000000"), "colour, got {dropped}");
    assert!(!dropped.contains("40px"), "font size, got {dropped}");
    assert!(!dropped.contains("Bold"), "font weight, got {dropped}");
    assert!(!dropped.contains("absolute-line-height"), "line height, got {dropped}");
    assert_eq!(label.opacity(), 1.0, "opacity");
    assert_eq!(picture.content_fit(), gtk4::ContentFit::Contain, "object_fit");
}

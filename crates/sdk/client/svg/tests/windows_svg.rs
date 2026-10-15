//! The Windows `Svg` is a real image of the markup, not the placeholder.
//!
//! ## The bug this pins
//!
//! `register` had no Windows arm, so every `Svg` on Windows mounted the
//! External placeholder: nothing drawn, no `on_load`, no intrinsic size,
//! and the box sized by nothing. Now it is one image node fed the markup
//! as an SVG `data:` URI (the backend's resvg image path).
//!
//! Checked here, on a real HWND + the scene painter's layout: the svg lays
//! out at its intrinsic size (the sibling below it starts at its height),
//! `on_load` fires, the handle reports the intrinsic size, a markup change
//! re-renders at the new size, and bad markup reaches `on_error` with the
//! parser's message.
//!
//! Runs on a Windows target only — on the Windows VM through
//! `scripts/windows-vm-runner.sh`, or under Wine.
#![cfg(windows)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use backend_windows::{newcore, WindowsBackend};
use runtime_shared::Ref;
use runtime_vocabulary::glue::{self, IntoElement};
use runtime_vocabulary::robot::{Query, Robot};
use svg::prelude::*;
use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WINDOW_EX_STYLE, WS_POPUP, WS_VISIBLE};

fn rect_svg(w: u32, h: u32) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"><rect width="{w}" height="{h}" fill="red"/></svg>"#)
}

fn host_window() -> windows::Win32::Foundation::HWND {
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("svg-test"),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            800,
            600,
            None,
            None,
            None,
            None,
        )
    }
    .expect("host window")
}

/// The painted `y` of the text laid out directly below the svg.
fn below_y(robot: &Robot) -> f32 {
    let below = robot.find(Query::label("below the svg")).expect("sibling text registered");
    robot.introspect_native(&below).expect("introspect").expect("painted node").frame.y
}

#[test]
fn regression_windows_svg_is_an_image_of_its_markup() {
    let backend = Rc::new(RefCell::new(WindowsBackend::new(host_window())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let loads = Rc::new(Cell::new(0));
    let errors: Rc<RefCell<Vec<String>>> = Rc::default();
    // Created inside the app (signals belong to its world), handed out here.
    let markup_slot = Rc::new(Cell::new(None));
    let handle: Ref<SvgHandle> = Ref::new();

    let (l, e, h, slot) = (loads.clone(), errors.clone(), handle.clone(), markup_slot.clone());
    let app = newcore::start(backend.clone(), svg::register, move || {
        let markup = glue::signal(rect_svg(40, 30));
        slot.set(Some(markup));
        let row = glue::view(vec![
            newcore::SceneElement::from(
                Svg(SvgProps {
                    markup: svg::markup(move || markup.get()),
                    on_load: Some(Rc::new({
                        let l = l.clone();
                        move || l.set(l.get() + 1)
                    })),
                    on_error: Some(Rc::new({
                        let e = e.clone();
                        move |m: String| e.borrow_mut().push(m)
                    })),
                })
                .bind(h.clone()),
            ),
            glue::text("below the svg").into_element(),
        ]);
        row.into_element()
    });
    let robot = Robot::new();
    let markup = markup_slot.get().expect("app built");

    assert_eq!(loads.get(), 1, "on_load fires for the first markup");
    assert!(errors.borrow().is_empty(), "{:?}", errors.borrow());
    assert_eq!(handle.with(|h| h.intrinsic_size()).flatten(), Some((40.0, 30.0)));
    assert_eq!(below_y(&robot), 30.0, "the svg lays out at its intrinsic height");

    markup.set(rect_svg(40, 50));
    newcore::flush_sync();
    // The window's paint runs the layout pass; there is no message pump here.
    backend.borrow_mut().relayout();
    assert_eq!(loads.get(), 2, "a markup change re-renders");
    assert_eq!(handle.with(|h| h.intrinsic_size()).flatten(), Some((40.0, 50.0)));
    assert_eq!(below_y(&robot), 50.0, "and re-lays out at the new size");

    markup.set("<svg".to_string());
    newcore::flush_sync();
    // The window's paint runs the layout pass; there is no message pump here.
    backend.borrow_mut().relayout();
    assert_eq!(errors.borrow().len(), 1, "bad markup reaches on_error");
    assert!(!errors.borrow()[0].is_empty());
    assert_eq!(below_y(&robot), 50.0, "a bad string keeps the image that was showing");

    app.stop();
}

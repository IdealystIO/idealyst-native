//! The Windows `WebView` is a real WebView2 host, not the placeholder.
//!
//! ## The bug this pins
//!
//! `register` had no Windows arm, so every `WebView` on Windows mounted the
//! External "not supported" placeholder: no browser, no window, and none of
//! the callbacks ever fired.
//!
//! Checked on a real HWND: the mount creates a child host window the layout
//! frames (`IdealystWebViewHost`, sized by the author style), and — a test
//! binary has no `WebView2Loader.dll` beside it — the missing loader reaches
//! `on_error` instead of failing silently or taking the app down. The page
//! itself (navigation, messages) needs the loader and the WebView2 runtime;
//! that is checked by hand in `examples/sdk-gallery`.
//!
//! Runs on a Windows target only — on the Windows VM through
//! `scripts/windows-vm-runner.sh`.
#![cfg(windows)]

use std::cell::Cell;
use std::rc::Rc;

use backend_windows::{newcore, WindowsBackend};
use runtime_vocabulary::glue::{self, IntoElement};
use runtime_vocabulary::robot::{Query, Robot};
use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetClassNameW, GetParent, GetWindow, GW_CHILD, GW_HWNDNEXT, WINDOW_EX_STYLE, WS_POPUP,
    WS_VISIBLE,
};

/// The child windows of `parent` with class `class`.
fn children_of_class(parent: HWND, class: &str) -> Vec<HWND> {
    let mut out = Vec::new();
    unsafe {
        let mut child = GetWindow(parent, GW_CHILD).unwrap_or_default();
        while !child.is_invalid() {
            let mut buf = [0u16; 64];
            let n = GetClassNameW(child, &mut buf);
            if String::from_utf16_lossy(&buf[..n as usize]) == class {
                out.push(child);
            }
            child = GetWindow(child, GW_HWNDNEXT).unwrap_or_default();
        }
    }
    out
}

#[test]
fn regression_windows_webview_mounts_a_webview2_host() {
    let host = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("webview-test"), WS_POPUP | WS_VISIBLE, 0, 0, 800, 600, None, None, None, None)
    }
    .expect("host window");
    let backend = Rc::new(std::cell::RefCell::new(WindowsBackend::new(host)));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let errors = Rc::new(Cell::new(0));
    let e = errors.clone();
    let app = newcore::start(backend.clone(), webview::register, move || {
        glue::view(vec![
            newcore::SceneElement::from(webview::WebView(webview::WebViewProps {
                url: webview::url("https://example.com"),
                on_error: Some(Rc::new(move || e.set(e.get() + 1))),
                ..Default::default()
            })
            .with_style(runtime_shared::StyleRules {
                height: Some(runtime_shared::Length::Px(200.0).into()),
                ..Default::default()
            })),
            glue::text("below the webview").into_element(),
        ])
        .into_element()
    });
    backend.borrow_mut().relayout();

    let hosts = children_of_class(host, "IdealystWebViewHost");
    assert_eq!(hosts.len(), 1, "one WebView2 host window, a child of the app window");
    assert_eq!(unsafe { GetParent(hosts[0]) }.unwrap(), host);
    assert_eq!(errors.get(), 1, "the missing loader reaches on_error");

    let robot = Robot::new();
    let below = robot.find(Query::label("below the webview")).expect("sibling text");
    let y = robot.introspect_native(&below).unwrap().unwrap().frame.y;
    assert_eq!(y, 200.0, "the host is laid out at its author height");
    let mut rc = windows::Win32::Foundation::RECT::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetClientRect(hosts[0], &mut rc).unwrap() };
    let scale = backend.borrow().scale();
    assert_eq!(rc.bottom - rc.top, (200.0 * scale).round() as i32, "and its window is framed to it");

    app.stop();
}

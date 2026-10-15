//! The Windows `code_block` mounts as ONE painted colored-runs leaf.
//!
//! ## The bug this pins
//!
//! `register` had no Windows arm (the old `windows.rs` still targeted the
//! deleted External table and was not even compiled), so Windows fell
//! through to the portable `<pre>`/span handler: one text node per token
//! inside a view — and native views are flex columns, so every token of a
//! docs code panel stacked on its own line.
//!
//! The observable contract checked here: a single-line block of several
//! tokens is exactly as tall as a single-token block — one line — and is
//! painted by the backend's code leaf, not by text nodes.
//!
//! Runs on a Windows target only (real HWND + the scene painter's layout);
//! on the macOS dev machine, under Wine via
//! `cargo test --target x86_64-pc-windows-gnu` with a Wine runner.
#![cfg(windows)]

use std::cell::RefCell;
use std::rc::Rc;

use backend_windows::{newcore, WindowsBackend};
use codeblock::code_block;
use runtime_shared::Color;
use runtime_vocabulary::glue::{self, IntoElement};
use runtime_vocabulary::robot::{Query, Robot};
use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, WINDOW_EX_STYLE, WS_POPUP, WS_VISIBLE,
};

/// Mount `spans` as a code block with a labeled text directly below it, and
/// report that text's painted `y` — how tall the block laid out — plus the
/// painted classes of every robot element in the tree.
///
/// Measured through the sibling, not the block: the block is a registry
/// item with no robot element of its own, and made the ROOT it would be
/// stretched to the window, hiding its content height entirely.
fn mount(spans: &[&str]) -> (f32, Vec<String>) {
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("codeblock-test"),
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
    .expect("host window");
    let backend = Rc::new(RefCell::new(WindowsBackend::new(hwnd)));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let spans: Vec<(String, Color)> =
        spans.iter().map(|t| (t.to_string(), Color("#336699".into()))).collect();
    let app = newcore::start(backend.clone(), codeblock::register, move || {
        glue::view(vec![
            newcore::SceneElement::from(code_block(spans)),
            glue::text("below the block").into_element(),
        ])
        .into_element()
    });
    let robot = Robot::new();
    let below = robot.find(Query::label("below the block")).expect("sibling text registered");
    let y = robot
        .introspect_native(&below)
        .expect("introspect")
        .expect("painted node")
        .frame
        .y;
    let classes = robot
        .elements()
        .iter()
        .filter_map(|e| robot.introspect_native(e).ok().flatten().map(|n| n.class))
        .collect();
    app.stop();
    (y, classes)
}

#[test]
fn regression_windows_code_block_is_one_line_not_a_token_column() {
    let (one_token, one_classes) = mount(&["main"]);
    let (many_tokens, classes) = mount(&["fn", " ", "main", "()", " ", "{}"]);
    assert!(one_token > 0.0, "the block must lay out");
    assert!(
        (many_tokens - one_token).abs() <= 1.0,
        "a one-line block is one line tall however many tokens it has \
         (one token {one_token}px, six tokens {many_tokens}px)"
    );
    // The only text node is the sibling: tokens are painted runs.
    let texts = |c: &[String]| c.iter().filter(|k| *k == "IdealystText").count();
    assert_eq!(texts(&one_classes), 1, "{one_classes:?}");
    assert_eq!(texts(&classes), 1, "tokens must not be text nodes; got {classes:?}");
}

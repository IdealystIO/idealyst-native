//! Headless smoke for TextInput: clicks to focus, dispatches keys,
//! snapshots the rendered row each step to show the value + cursor
//! advancing.

use std::cell::RefCell;
use std::rc::Rc;

use backend_terminal::{TerminalBackend, TerminalKey};
use framework_core::primitives::text_input::text_input;
use framework_core::{
    children, signal, text, view, AlignItems, Color, FlexDirection, JustifyContent, Length,
    Primitive, StyleRules, StyleSheet, Tokenized,
};

fn px(v: f32) -> Tokenized<Length> { Tokenized::Literal(Length::Px(v)) }
fn pct(v: f32) -> Tokenized<Length> { Tokenized::Literal(Length::Percent(v)) }
fn cval(s: &str) -> Tokenized<Color> { Tokenized::Literal(Color(s.into())) }
fn sheet(r: StyleRules) -> Rc<StyleSheet> { Rc::new(StyleSheet::r#static(r)) }

fn app() -> Primitive {
    let value = signal!(String::new());
    let value_for_change = value;
    let value_for_label = value;

    let page = sheet(StyleRules {
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        background: Some(cval("#0a0c11")),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        gap: Some(px(1.0)),
        ..Default::default()
    });
    let input_style = sheet(StyleRules {
        background: Some(cval("#1a1f2e")),
        color: Some(cval("#ffffff")),
        width: Some(px(20.0)),
        padding_left: Some(px(1.0)),
        padding_right: Some(px(1.0)),
        ..Default::default()
    });

    view(children![
        text(move || format!("you typed: {}", value_for_label.get())),
        text_input(value, move |new| value_for_change.set(new))
            .placeholder("type here…".to_string())
            .with_style(input_style),
    ])
    .with_style(page)
    .into()
}

fn key(name: &str) -> TerminalKey {
    TerminalKey {
        key: name.to_string(),
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    }
}

fn snapshot(backend: &Rc<RefCell<TerminalBackend>>, label: &str) {
    let grid = backend.borrow_mut().render_to_grid();
    println!("\n[{}]", label);
    for row in 0..grid.rows {
        let mut line = String::new();
        for col in 0..grid.cols {
            line.push(grid.cell(col, row).map(|c| c.glyph).unwrap_or(' '));
        }
        println!("  |{}|", line.trim_end());
    }
}

fn main() {
    host_terminal::install_scheduler_for_testing();
    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend.borrow_mut().set_viewport(40, 7);
    backend_terminal::install_global_self(Rc::downgrade(&backend));
    let _owner = framework_core::mount(backend.clone(), app);

    snapshot(&backend, "initial (placeholder)");

    // Drive the layout pass so frames are populated for hit-test.
    let _ = backend.borrow_mut().render_to_grid();

    // Click the input. Center of viewport is (20, 3); input sits at
    // around row 3.
    {
        let outcome = backend.borrow_mut().dispatch_click(20, 4);
        println!("\nclick (20,4) → {:?}", outcome);
    }
    snapshot(&backend, "after click (focused, cursor visible)");

    // Type "hi"
    for ch in ['h', 'i', ' ', 't', 'h', 'e', 'r', 'e'] {
        backend.borrow_mut().dispatch_key(&key(&ch.to_string()));
    }
    host_terminal::tick_scheduler_for_testing();
    snapshot(&backend, "after typing 'hi there'");

    // Move the cursor home and delete the first two chars.
    backend.borrow_mut().dispatch_key(&key("Home"));
    backend.borrow_mut().dispatch_key(&key("Delete"));
    backend.borrow_mut().dispatch_key(&key("Delete"));
    host_terminal::tick_scheduler_for_testing();
    snapshot(&backend, "after Home + 2x Delete");

    // Press Escape to blur.
    backend.borrow_mut().dispatch_key(&key("Escape"));
    snapshot(&backend, "after Escape (blurred, no cursor)");
}

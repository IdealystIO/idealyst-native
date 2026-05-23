//! Headless smoke for the new primitives: Toggle and
//! ActivityIndicator. Snapshots a handful of frames and prints the
//! row containing the spinner so you can see it cycle through its
//! 10-step braille sequence.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use backend_terminal::TerminalBackend;
use framework_core::primitives::activity_indicator::activity_indicator;
use framework_core::primitives::toggle::toggle;
use framework_core::{
    children, signal, text, view, AlignItems, Color, FlexDirection, JustifyContent, Length,
    Primitive, StyleRules, StyleSheet, Tokenized,
};

fn pct(v: f32) -> Tokenized<Length> { Tokenized::Literal(Length::Percent(v)) }
fn px(v: f32) -> Tokenized<Length> { Tokenized::Literal(Length::Px(v)) }
fn cval(s: &str) -> Tokenized<Color> { Tokenized::Literal(Color(s.into())) }
fn sheet(r: StyleRules) -> Rc<StyleSheet> { Rc::new(StyleSheet::r#static(r)) }

fn app() -> Primitive {
    let on = signal!(true);
    let on_for_change = on;
    let page = sheet(StyleRules {
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        background: Some(cval("#0a0c11")),
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        gap: Some(px(2.0)),
        ..Default::default()
    });
    view(children![
        text("spinner ->"),
        activity_indicator(),
        toggle(on, move |new| on_for_change.set(new)),
    ])
    .with_style(page)
    .into()
}

fn main() {
    host_terminal::install_scheduler_for_testing();
    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend.borrow_mut().set_viewport(40, 5);
    backend_terminal::install_global_self(Rc::downgrade(&backend));
    let _owner = framework_core::mount(backend.clone(), app);

    for frame in 0..12 {
        host_terminal::tick_scheduler_for_testing();
        let grid = backend.borrow_mut().render_to_grid();
        // Print the middle row so we can see the spinner advance.
        let row = grid.rows / 2;
        let mut line = String::new();
        for col in 0..grid.cols {
            line.push(grid.cell(col, row).map(|c| c.glyph).unwrap_or(' '));
        }
        println!("frame {:>2}: |{}|", frame, line.trim_end());
        std::thread::sleep(Duration::from_millis(33));
    }
}

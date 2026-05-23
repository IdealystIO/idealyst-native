//! Regression: clicking a Button (which fires `Signal::set` and
//! triggers a reactive text update) used to panic with "RefCell
//! already borrowed" because the click handler ran while the host
//! still held the backend's `&mut` borrow from `dispatch_click`.
//! Confirms the deferred-fire path works for both Button and
//! Toggle.

use std::cell::RefCell;
use std::rc::Rc;

use backend_terminal::TerminalBackend;
use framework_core::primitives::toggle::toggle;
use framework_core::{
    button, children, signal, text, view, AlignItems, FlexDirection, JustifyContent, Length,
    Primitive, StyleRules, StyleSheet, Tokenized,
};

fn px(v: f32) -> Tokenized<Length> { Tokenized::Literal(Length::Px(v)) }
fn pct(v: f32) -> Tokenized<Length> { Tokenized::Literal(Length::Percent(v)) }
fn sheet(r: StyleRules) -> Rc<StyleSheet> { Rc::new(StyleSheet::r#static(r)) }

fn app() -> Primitive {
    let count = signal!(0i32);
    let flag = signal!(false);
    let count_for_label = count;
    let count_for_inc = count;
    let flag_for_change = flag;
    let flag_for_label = flag;
    let page = sheet(StyleRules {
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        gap: Some(px(1.0)),
        ..Default::default()
    });
    view(children![
        text(move || format!("count={}, flag={}", count_for_label.get(), flag_for_label.get())),
        button("[ + ]", move || count_for_inc.update(|n| *n += 1)),
        toggle(flag, move |new| flag_for_change.set(new)),
    ])
    .with_style(page)
    .into()
}

fn simulate_click(backend: &Rc<RefCell<TerminalBackend>>, col: u16, row: u16) {
    // Mirror the host's pattern exactly: borrow_mut → dispatch_click
    // → drop borrow → fire returned handler.
    let outcome = backend.borrow_mut().dispatch_click(col, row);
    if let backend_terminal::ClickOutcome::HandlerFired(h) = outcome {
        h();
    }
}

fn main() {
    host_terminal::install_scheduler_for_testing();
    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend.borrow_mut().set_viewport(40, 10);
    backend_terminal::install_global_self(Rc::downgrade(&backend));
    let _owner = framework_core::mount(backend.clone(), app);

    // Render once so frames populate.
    let _ = backend.borrow_mut().render_to_grid();

    // Click the button at roughly center vertical. Layout sits the
    // button somewhere in rows 4-6 in a 10-row viewport with column
    // flex + justify-center.
    for row in 3..7 {
        simulate_click(&backend, 20, row);
    }
    host_terminal::tick_scheduler_for_testing();

    // Click the toggle (sits one row below the button).
    for row in 4..8 {
        simulate_click(&backend, 20, row);
    }
    host_terminal::tick_scheduler_for_testing();

    // Render the result.
    let grid = backend.borrow_mut().render_to_grid();
    for r in 0..grid.rows {
        let mut line = String::new();
        for c in 0..grid.cols {
            line.push(grid.cell(c, r).map(|x| x.glyph).unwrap_or(' '));
        }
        println!("|{}|", line.trim_end());
    }
    println!("\nno panic — clicks dispatched cleanly");
}

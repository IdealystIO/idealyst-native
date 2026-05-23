//! Regression smoke for z-index. Two siblings — a `text("HEADLINE…")`
//! and a `view` with a solid red bg — overlap. The test toggles the
//! red view's `z_index` between -1 (behind) and +1 (in front) and
//! shows the rendered row each time.
//!
//! Expected:
//!   - z = -1: "HEADLINE" stays readable; red bg shows around / behind it.
//!   - z = +1: red bg covers the row; the underlying text is hidden.

use std::cell::RefCell;
use std::rc::Rc;

use backend_terminal::TerminalBackend;
use framework_core::animation::{AnimProp, AnimatedValue};
use framework_core::{
    animated, children, signal, text, view, AlignItems, Color, FlexDirection, Length, Position,
    Primitive, Ref, StyleRules, StyleSheet, Tokenized, ViewHandle,
};

fn px(v: f32) -> Tokenized<Length> { Tokenized::Literal(Length::Px(v)) }
fn cval(s: &str) -> Tokenized<Color> { Tokenized::Literal(Color(s.into())) }
fn sheet(r: StyleRules) -> Rc<StyleSheet> { Rc::new(StyleSheet::r#static(r)) }

struct Shared {
    z: Rc<std::cell::Cell<Option<AnimatedValue<f32>>>>,
}

fn app(shared: Rc<Shared>) -> Primitive {
    let z = animated!(0.0_f32);
    shared.z.set(Some(z.clone()));

    // Page: solid cream bg, single row centered.
    let page = sheet(StyleRules {
        width: Some(Tokenized::Literal(Length::Percent(100.0))),
        height: Some(Tokenized::Literal(Length::Percent(100.0))),
        background: Some(cval("#0a0c11")),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(framework_core::JustifyContent::Center),
        position: Some(Position::Relative),
        ..Default::default()
    });
    let head = sheet(StyleRules {
        color: Some(cval("#ffd28b")),
        ..Default::default()
    });
    // Red square positioned over the headline.
    let red = sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        width: Some(Tokenized::Literal(Length::Percent(100.0))),
        height: Some(Tokenized::Literal(Length::Percent(100.0))),
        background: Some(cval("rgba(200, 60, 60, 1.0)")),
        ..Default::default()
    });
    let red_ref: Ref<ViewHandle> = Ref::new();
    z.bind(red_ref, AnimProp::ZIndex);

    view(children![
        text("HEADLINE_HEADLINE_HEADLINE").with_style(head),
        view(children![]).with_style(red).bind(red_ref),
    ])
    .with_style(page)
    .into()
}

fn dump_row(backend: &Rc<RefCell<TerminalBackend>>, label: &str) {
    host_terminal::tick_scheduler_for_testing();
    let grid = backend.borrow_mut().render_to_grid();
    let r = grid.rows / 2;
    let mut line = String::new();
    for c in 0..grid.cols {
        line.push(grid.cell(c, r).map(|x| x.glyph).unwrap_or(' '));
    }
    println!("{:>30}: |{}|", label, line);
}

fn main() {
    host_terminal::install_scheduler_for_testing();
    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend.borrow_mut().set_viewport(40, 3);
    backend_terminal::install_global_self(Rc::downgrade(&backend));
    let shared = Rc::new(Shared {
        z: Rc::new(std::cell::Cell::new(None)),
    });
    let shared_for_app = shared.clone();
    let _owner = framework_core::mount(backend.clone(), move || app(shared_for_app.clone()));
    let z = shared.z.take().unwrap();

    // z = 0 (declared after text, equal z → tree-order tiebreak; red wins, on top).
    z.set(0.0);
    dump_row(&backend, "z=0 (tiebreak: red on top)");

    // z = -1: red behind text. We should see HEADLINE glyphs.
    z.set(-1.0);
    dump_row(&backend, "z=-1 (red behind text)");

    // z = +1: red on top. HEADLINE should be hidden.
    z.set(1.0);
    dump_row(&backend, "z=+1 (red over text)");

    // Toggle back to confirm transitions cleanly.
    z.set(-1.0);
    dump_row(&backend, "z=-1 again");
}

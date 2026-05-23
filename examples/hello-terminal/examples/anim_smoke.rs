//! Headless smoke test for the animation pipeline. Renders ~30
//! frames at fake "30 fps" deltas and prints the counter card's bg
//! color across the frames. Should observe the warm-then-cool tween
//! after each `count` change.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use backend_terminal::TerminalBackend;
use framework_core::animation::{AnimProp, TweenTo};
use framework_core::{
    animated, button, children, effect, on_cleanup, signal, text, view, AlignItems, Color,
    FlexDirection, JustifyContent, Length, Primitive, StyleRules, StyleSheet, Tokenized,
};

fn px(v: f32) -> Tokenized<Length> { Tokenized::Literal(Length::Px(v)) }
fn pct(v: f32) -> Tokenized<Length> { Tokenized::Literal(Length::Percent(v)) }
fn cval(s: &str) -> Tokenized<Color> { Tokenized::Literal(Color(s.into())) }
fn sheet(r: StyleRules) -> Rc<StyleSheet> { Rc::new(StyleSheet::r#static(r)) }

fn app() -> Primitive {
    let count = signal!(0i32);
    let card_bg = animated!((0.10_f32, 0.12_f32, 0.18_f32, 1.0_f32));

    let card_bg_for_effect = card_bg.clone();
    let count_for_effect = count;
    effect!({
        let _ = count_for_effect.get();
        card_bg_for_effect.set((0.95, 0.50, 0.20, 1.0));
        let card_bg_for_tween = card_bg_for_effect.clone();
        let task = framework_core::after_ms(20, move || {
            card_bg_for_tween.animate(
                TweenTo::new((0.10, 0.12, 0.18, 1.0), Duration::from_millis(600))
                    .ease_in_out(),
            );
        });
        on_cleanup(move || drop(task));
    });

    let page = sheet(StyleRules {
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        background: Some(cval("#0a0c11")),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });
    let card_style = sheet(StyleRules {
        background: Some(cval("#1a1f2e")),
        padding_top: Some(px(1.0)),
        padding_bottom: Some(px(1.0)),
        padding_left: Some(px(4.0)),
        padding_right: Some(px(4.0)),
        ..Default::default()
    });
    let card_ref: framework_core::Ref<framework_core::ViewHandle> = framework_core::Ref::new();
    card_bg.bind_color(card_ref, AnimProp::BackgroundColor);

    // Drive count from a button — we'll press it manually in main.
    view(children![
        view(children![text(move || format!("count = {}", count.get()))])
            .with_style(card_style)
            .bind(card_ref),
        button("[ + ]", move || count.update(|n| *n += 1)),
    ])
    .with_style(page)
    .into()
}

fn main() {
    // Install host-terminal's scheduler so animation tweens advance
    // when we tick.
    host_terminal::install_scheduler_for_testing();

    let backend = Rc::new(RefCell::new(TerminalBackend::new()));
    backend.borrow_mut().set_viewport(50, 10);
    backend_terminal::install_global_self(Rc::downgrade(&backend));
    let _owner = framework_core::mount(backend.clone(), app);

    println!("frame  | card bg (RGB)");
    println!("-------+----------------------");
    for frame in 0..30 {
        host_terminal::tick_scheduler_for_testing();
        let grid = backend.borrow_mut().render_to_grid();
        // Sample the middle of the card area to read its current bg.
        let bg = grid.cell(25, 5).and_then(|c| c.bg);
        match bg {
            Some(c) => println!("{:>5}  | rgb({:3}, {:3}, {:3})", frame, c.r, c.g, c.b),
            None => println!("{:>5}  | (none)", frame),
        }
        // After frame 5 click the button so the flash tween fires.
        if frame == 5 {
            let outcome = backend.borrow_mut().dispatch_click(25, 7);
            println!("       ^ click [ + ] → {:?}", outcome);
        }
        std::thread::sleep(Duration::from_millis(33));
    }
}

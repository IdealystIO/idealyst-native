//! `Tokenized::resolve()` is a reactive read: an effect that resolves a
//! token re-runs when the theme changes it.
//!
//! On the new kernel it wasn't. `resolve()` reads `runtime_shared`'s
//! token registry, whose signals belong to the legacy arena, so a world
//! effect never subscribed and kept the first theme's value. Every
//! idea-ui icon tint is such an effect (`.color(move || fg.resolve())`),
//! so after a light/dark toggle the Inspector's icons stayed in the old
//! theme's color while everything class-styled re-tinted.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::{Color, TokenEntry, TokenValue, Tokenized};
use runtime_world::World;

fn text_token() -> Tokenized<Color> {
    Tokenized::token("color-text", Color("#fallback".into()))
}

fn tokens(text: &str) -> Vec<TokenEntry> {
    vec![TokenEntry { name: "color-text", value: TokenValue::Color(Color(text.into())) }]
}

#[test]
fn regression_an_effect_resolving_a_token_follows_theme_swaps() {
    let world = World::new();
    let seen: Rc<RefCell<Vec<String>>> = Rc::default();
    world.enter(|| {
        runtime_vocabulary::glue::install_tokens(&tokens("#111111"));
        let seen = seen.clone();
        let _tint = runtime_world::effect(move || seen.borrow_mut().push(text_token().resolve().0));
    });
    world.flush();
    assert_eq!(seen.borrow().last().map(String::as_str), Some("#111111"));

    world.enter(|| runtime_vocabulary::glue::update_tokens(&tokens("#eeeeee")));
    world.flush();
    assert_eq!(
        seen.borrow().last().map(String::as_str),
        Some("#eeeeee"),
        "the effect re-ran with the new theme's value: {:?}",
        seen.borrow()
    );
}

/// A literal is not a theme read: resolving one subscribes nothing, so
/// a theme swap leaves the effect alone.
#[test]
fn a_literal_resolve_does_not_subscribe_to_the_theme() {
    let world = World::new();
    let runs = Rc::new(RefCell::new(0));
    world.enter(|| {
        runtime_vocabulary::glue::install_tokens(&tokens("#111111"));
        let runs = runs.clone();
        let _e = runtime_world::effect(move || {
            let _ = Tokenized::Literal(Color("#123456".into())).resolve();
            *runs.borrow_mut() += 1;
        });
    });
    world.flush();
    world.enter(|| runtime_vocabulary::glue::update_tokens(&tokens("#eeeeee")));
    world.flush();
    assert_eq!(*runs.borrow(), 1);
}

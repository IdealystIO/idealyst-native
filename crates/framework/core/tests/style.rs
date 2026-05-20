//! Style suite — `Tokenized<T>`, the token registry, and style
//! resolution.
//!
//! Scope:
//! - `Tokenized::Literal` vs `Tokenized::Token` constructors + `.value()`
//! - `install_tokens` / `update_tokens` round-trip through the registry
//! - Per-token reactivity: an Effect that reads a Tokenized value
//!   re-fires when its token is updated
//! - Per-token reactivity is SCOPED to the token name: updating an
//!   unrelated token doesn't fire the subscriber

#[path = "common/mod.rs"]
mod common;

use framework_core::{
    install_tokens, signal, update_tokens, Color, Effect, Length, Signal, TokenEntry, TokenValue,
    Tokenized,
};

/// `Tokenized::Literal` returns the literal value from `.value()`.
#[test]
fn tokenized_literal_returns_literal_value() {
    let t: Tokenized<Color> = Tokenized::Literal(Color("#ff0000".into()));
    assert_eq!(t.value(), &Color("#ff0000".into()));
}

/// `Tokenized::Token` returns the FALLBACK from `.value()` when no
/// runtime token registry is installed for that name.
#[test]
fn tokenized_token_returns_fallback_when_unset() {
    let t: Tokenized<Color> = Tokenized::token("primary", Color("#fallback".into()));
    assert_eq!(t.value(), &Color("#fallback".into()));
}

/// After `install_tokens`, `Tokenized::token(name, ...).resolve()`
/// returns the installed value (not the fallback).
#[test]
fn install_tokens_makes_resolve_return_installed_value() {
    install_tokens(&[TokenEntry {
        name: "test-color-primary",
        value: TokenValue::Color(Color("#installed".into())),
    }]);

    let t: Tokenized<Color> = Tokenized::token("test-color-primary", Color("#fallback".into()));
    assert_eq!(
        t.resolve(),
        Color("#installed".into()),
        "resolve picks up installed value"
    );

    // The fallback is unchanged; only resolve is affected.
    assert_eq!(t.value(), &Color("#fallback".into()));
}

/// `update_tokens` swaps the value for an already-registered name.
#[test]
fn update_tokens_swaps_value() {
    install_tokens(&[TokenEntry {
        name: "test-bg-1",
        value: TokenValue::Color(Color("#aaa".into())),
    }]);
    let t: Tokenized<Color> = Tokenized::token("test-bg-1", Color("#fallback".into()));
    assert_eq!(t.resolve(), Color("#aaa".into()));

    update_tokens(&[TokenEntry {
        name: "test-bg-1",
        value: TokenValue::Color(Color("#bbb".into())),
    }]);
    assert_eq!(t.resolve(), Color("#bbb".into()));
}

/// Per-token reactivity: an Effect that reads `t.resolve()` re-fires
/// when `update_tokens` changes that specific token.
#[test]
fn token_resolve_is_reactive() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_tokens(&[TokenEntry {
        name: "test-react-1",
        value: TokenValue::Color(Color("#red".into())),
    }]);

    let t: Tokenized<Color> = Tokenized::token("test-react-1", Color("#fallback".into()));
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = Effect::new(move || {
        let _ = t.resolve();
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1, "initial");

    update_tokens(&[TokenEntry {
        name: "test-react-1",
        value: TokenValue::Color(Color("#blue".into())),
    }]);
    assert_eq!(count.get(), 2, "subscriber re-fired on token update");
}

/// Updating an UNRELATED token does NOT fire a subscriber to a
/// different token (per-token reactivity, not global theme
/// reactivity).
#[test]
fn token_subscribers_are_per_token() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_tokens(&[
        TokenEntry {
            name: "test-iso-A",
            value: TokenValue::Color(Color("#aaa".into())),
        },
        TokenEntry {
            name: "test-iso-B",
            value: TokenValue::Color(Color("#bbb".into())),
        },
    ]);

    let ta: Tokenized<Color> = Tokenized::token("test-iso-A", Color("#fa".into()));
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = Effect::new(move || {
        let _ = ta.resolve();
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1);

    // Update B: A's subscriber should NOT fire.
    update_tokens(&[TokenEntry {
        name: "test-iso-B",
        value: TokenValue::Color(Color("#new-b".into())),
    }]);
    assert_eq!(count.get(), 1, "unrelated token update didn't fire subscriber");

    // Update A: subscriber fires.
    update_tokens(&[TokenEntry {
        name: "test-iso-A",
        value: TokenValue::Color(Color("#new-a".into())),
    }]);
    assert_eq!(count.get(), 2, "subscribed token update fired subscriber");
}

/// `Tokenized<Length>` works the same as `Tokenized<Color>`.
#[test]
fn tokenized_length_round_trip() {
    install_tokens(&[TokenEntry {
        name: "test-spacing-1",
        value: TokenValue::Length(Length::Px(8.0)),
    }]);
    let t: Tokenized<Length> = Tokenized::token("test-spacing-1", Length::Px(16.0));
    assert_eq!(t.resolve(), Length::Px(8.0));

    update_tokens(&[TokenEntry {
        name: "test-spacing-1",
        value: TokenValue::Length(Length::Px(24.0)),
    }]);
    assert_eq!(t.resolve(), Length::Px(24.0));
}

/// `Tokenized<f32>` works the same.
#[test]
fn tokenized_number_round_trip() {
    install_tokens(&[TokenEntry {
        name: "test-radius-1",
        value: TokenValue::Number(4.0),
    }]);
    let t: Tokenized<f32> = Tokenized::token("test-radius-1", 0.0);
    assert_eq!(t.resolve(), 4.0);
}

/// Updating multiple tokens in one `update_tokens` call fires each
/// subscriber once (not twice for a 2-token update).
#[test]
fn batched_update_tokens_fires_each_subscriber_once() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_tokens(&[
        TokenEntry {
            name: "test-multi-A",
            value: TokenValue::Color(Color("#a1".into())),
        },
        TokenEntry {
            name: "test-multi-B",
            value: TokenValue::Color(Color("#b1".into())),
        },
    ]);

    let ta: Tokenized<Color> = Tokenized::token("test-multi-A", Color("#fb".into()));
    let tb: Tokenized<Color> = Tokenized::token("test-multi-B", Color("#fb".into()));

    let count_a: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let count_b: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ca = count_a.clone();
    let cb = count_b.clone();

    let _ea = Effect::new(move || {
        let _ = ta.resolve();
        ca.set(ca.get() + 1);
    });
    let _eb = Effect::new(move || {
        let _ = tb.resolve();
        cb.set(cb.get() + 1);
    });

    assert_eq!(count_a.get(), 1);
    assert_eq!(count_b.get(), 1);

    update_tokens(&[
        TokenEntry {
            name: "test-multi-A",
            value: TokenValue::Color(Color("#a2".into())),
        },
        TokenEntry {
            name: "test-multi-B",
            value: TokenValue::Color(Color("#b2".into())),
        },
    ]);

    assert_eq!(count_a.get(), 2, "A subscriber fired once");
    assert_eq!(count_b.get(), 2, "B subscriber fired once");
}

/// Reads of `Tokenized` inside an Effect that don't actually call
/// `.resolve()` don't subscribe — only `.resolve()` is the reactive
/// entry point.
#[test]
fn tokenized_value_alone_does_not_subscribe() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_tokens(&[TokenEntry {
        name: "test-novalue-1",
        value: TokenValue::Color(Color("#x".into())),
    }]);

    let t: Tokenized<Color> = Tokenized::token("test-novalue-1", Color("#f".into()));
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = Effect::new(move || {
        // `.value()` reads the fallback, not the registry — should
        // NOT subscribe to token changes.
        let _ = t.value();
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1);
    update_tokens(&[TokenEntry {
        name: "test-novalue-1",
        value: TokenValue::Color(Color("#y".into())),
    }]);
    assert_eq!(count.get(), 1, "value() doesn't subscribe; resolve() does");
}

/// Signal-based reactivity still works alongside the token system —
/// just to confirm token reactivity doesn't break ordinary effects.
#[test]
fn signal_reactivity_alongside_tokens() {
    use std::cell::Cell;
    use std::rc::Rc;

    let s: Signal<i32> = signal!(0);
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = Effect::new(move || {
        let _ = s.get();
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1);
    s.set(1);
    assert_eq!(count.get(), 2);

    // Token updates on unrelated tokens shouldn't fire signal subscribers.
    install_tokens(&[TokenEntry {
        name: "test-isolation-token",
        value: TokenValue::Color(Color("#abc".into())),
    }]);
    update_tokens(&[TokenEntry {
        name: "test-isolation-token",
        value: TokenValue::Color(Color("#def".into())),
    }]);
    assert_eq!(count.get(), 2, "token update didn't fire signal subscriber");
}

//! The token registry — `Tokenized<T>`, `install_tokens` /
//! `update_tokens`, and per-token reactive subscription.
//!
//! RELOCATED from `runtime-core/tests/style.rs` (deletion baseline §4.2,
//! SV-R). `shared/src/style.rs` has **zero** inline tests, and this is
//! the registry the whole theming surface runs on — it is driven by
//! `runtime-vocabulary`'s `ThemeCtx` on the surviving core exactly as it
//! was by the walker's theme driver before.
//!
//! Scope:
//! - `Tokenized::Literal` vs `Tokenized::Token` constructors + `.value()`
//! - `install_tokens` / `update_tokens` round-trip through the registry
//! - per-token reactivity: an effect reading a `Tokenized` value re-fires
//!   when ITS token is updated…
//! - …and is SCOPED to the token name: an unrelated token's update is
//!   silent
//! - the pending-map ordering invariant (`update_tokens` populates every
//!   new value BEFORE firing any subscriber, so a subscriber reading a
//!   sibling token in the same batch never sees a half-applied theme)
//!
//! **What stayed behind**, and why it is not a loss here: the other 13
//! cases in the old file drove the WALKER (`TestRuntime` + backend
//! `Event` assertions) or the `stylesheet!` macro, neither of which
//! runtime-shared can reach. Their homes on the surviving core are
//! `runtime-vocabulary/tests/vocab.rs` (sheet registration, sweep,
//! signal-class fallback, state overlays) and
//! `runtime-vocabulary/src/style_attach.rs::overlay_merge_tests`
//! (breakpoint/container overlay merge, ported in an earlier wave). The
//! two the deletion baseline flagged as genuinely thin —  the native
//! container-query feedback loop and typeface asset emission — are
//! called out in the baseline doc's §4.1 #17 row, not silently dropped.

use runtime_shared::{
    install_tokens, signal, update_tokens, watch, Color, Length, Signal, TokenEntry, TokenValue,
    Tokenized,
};

#[test]
fn tokenized_literal_returns_literal_value() {
    let t: Tokenized<Color> = Tokenized::Literal(Color("#ff0000".into()));
    assert_eq!(t.value(), &Color("#ff0000".into()));
}

#[test]
fn tokenized_token_returns_fallback_when_unset() {
    let t: Tokenized<Color> = Tokenized::token("primary", Color("#fallback".into()));
    assert_eq!(t.value(), &Color("#fallback".into()));
}

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

    let _e = watch(move || {
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

    let _e = watch(move || {
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

#[test]
fn tokenized_number_round_trip() {
    install_tokens(&[TokenEntry {
        name: "test-radius-1",
        value: TokenValue::Number(4.0),
    }]);
    let t: Tokenized<f32> = Tokenized::token("test-radius-1", 0.0);
    assert_eq!(t.resolve(), 4.0);
}

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

    let _ea = watch(move || {
        let _ = ta.resolve();
        ca.set(ca.get() + 1);
    });
    let _eb = watch(move || {
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

#[test]
fn update_tokens_populates_pending_before_firing_subscribers() {
    use runtime_shared::take_pending_token_updates;
    use std::cell::RefCell;
    use std::rc::Rc;

    // Drain whatever pending state earlier tests left behind so this
    // test reasons about its own writes only.
    let _ = take_pending_token_updates();

    install_tokens(&[TokenEntry {
        name: "test-pending-order",
        value: TokenValue::Color(Color("#aaa".into())),
    }]);
    // Initial install itself queues a pending entry; drain it.
    let _ = take_pending_token_updates();

    let tok: Tokenized<Color> = Tokenized::token("test-pending-order", Color("#000".into()));

    // The Effect mirrors the cohort driver's read-then-flush pattern:
    // it subscribes to the token (via `resolve`) and on every fire
    // pulls the pending queue. We stash what each fire saw so the
    // assertion below can inspect the second fire's view.
    let observed: Rc<RefCell<Vec<Vec<TokenEntry>>>> = Rc::new(RefCell::new(Vec::new()));
    let obs = observed.clone();
    let _e = watch(move || {
        let _ = tok.resolve(); // subscribe
        let drained = take_pending_token_updates();
        // Flatten the Vec<Vec<...>> so the test reads naturally — we
        // only push one TokenEntry per `update_tokens` call here.
        for batch in drained {
            obs.borrow_mut().push(batch);
        }
    });

    // First fire happens at `watch()`; pending was drained above,
    // so this fire sees nothing. The test's load-bearing assertion
    // is about the SECOND fire (post-`update_tokens`).
    observed.borrow_mut().clear();

    update_tokens(&[TokenEntry {
        name: "test-pending-order",
        value: TokenValue::Color(Color("#bbb".into())),
    }]);

    let obs = observed.borrow();
    assert_eq!(
        obs.len(),
        1,
        "Effect should fire exactly once after `update_tokens`, got {} fires",
        obs.len(),
    );
    let drained = &obs[0];
    assert_eq!(
        drained.len(),
        1,
        "the pending batch the Effect drained should contain the one TokenEntry \
         that `update_tokens` was called with — instead got {} entries: {:?}",
        drained.len(),
        drained,
    );
    assert_eq!(drained[0].name, "test-pending-order");
    assert!(
        matches!(&drained[0].value, TokenValue::Color(c) if c.0 == "#bbb"),
        "pending entry's value should be the JUST-written #bbb (proof that the \
         push to PENDING_TOKEN_UPDATES happened BEFORE the sig.set that fired \
         this Effect). Got: {:?}",
        drained[0].value,
    );
}

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

    let _e = watch(move || {
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

#[test]
fn signal_reactivity_alongside_tokens() {
    use std::cell::Cell;
    use std::rc::Rc;

    let s: Signal<i32> = signal(0);
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = watch(move || {
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

//! Walker-coupled regression tests that moved OUT of runtime-shared's
//! `style/tests.rs` and `reactive/tests.rs` when those modules were
//! extracted: they exercise the old-core `Element` /
//! `Element::with_style_overrides` / builder surface, which stays in
//! THIS crate. Kept verbatim; only the import paths changed.

use crate::style::{
    resolve, Color, Easing, Length, Position, StyleApplication, StyleRules, StyleSheet, Tokenized,
    Transition,
};
use crate::reactive::{Effect, Signal};
use std::rc::Rc;

// ------------------------------------------------------------------
// Element::with_style_overrides — the substrate hook navigator
// handlers use to force screen placement (e.g. the web/SSR stack's
// full-bleed absolute fill) through the style system instead of
// stamping raw classes on built nodes.
// ------------------------------------------------------------------

fn fill_overlay() -> Rc<StyleRules> {
    crate::primitives::navigator::stack_screen_fill_rules()
}

fn source_of(el: &crate::element::Element) -> &crate::sources::StyleSource {
    match el {
        crate::element::Element::View { style, .. } => {
            style.as_ref().expect("view should carry a style source")
        }
        _ => panic!("expected a View"),
    }
}

/// An unstyled element gains a Static application whose resolution
/// is exactly the overlay rules.
#[test]
fn style_overrides_on_unstyled_element_resolve_to_overlay() {
    let el: crate::element::Element = crate::view(Vec::new()).into();
    let el = el.with_style_overrides(fill_overlay());
    let app = match source_of(&el) {
        crate::sources::StyleSource::Static(app) => app.clone(),
        _ => panic!("unstyled element should gain a Static source"),
    };
    let r = resolve(&app);
    assert_eq!(r.position, Some(Position::Absolute));
    assert_eq!(r.top, Some(Tokenized::Literal(Length::Px(0.0))));
    assert_eq!(r.left, Some(Tokenized::Literal(Length::Px(0.0))));
    assert_eq!(r.right, Some(Tokenized::Literal(Length::Px(0.0))));
    assert_eq!(r.bottom, Some(Tokenized::Literal(Length::Px(0.0))));
}

/// The overlay wins over the author's conflicting fields but leaves
/// non-conflicting author styling intact — the `!important`-free
/// replacement for the legacy `.ui-nav-screen` class stamp.
#[test]
fn style_overrides_beat_author_position_but_preserve_author_styling() {
    let author = Rc::new(StyleSheet::r#static(StyleRules {
        position: Some(Position::Relative),
        background: Some(Tokenized::Literal(Color("#123456".into()))),
        width: Some(Tokenized::Literal(Length::Px(200.0))),
        ..Default::default()
    }));
    let el: crate::element::Element = crate::view(Vec::new())
        .with_style(StyleApplication::new(author))
        .into();
    let el = el.with_style_overrides(fill_overlay());
    let app = match source_of(&el) {
        crate::sources::StyleSource::Static(app) => app.clone(),
        _ => panic!("static author style should stay Static"),
    };
    let r = resolve(&app);
    assert_eq!(r.position, Some(Position::Absolute), "overlay position wins");
    assert_eq!(r.top, Some(Tokenized::Literal(Length::Px(0.0))), "overlay inset wins");
    assert_eq!(
        r.background,
        Some(Tokenized::Literal(Color("#123456".into()))),
        "author background survives"
    );
    assert_eq!(
        r.width,
        Some(Tokenized::Literal(Length::Px(200.0))),
        "author width survives — the overlay deliberately sets no size"
    );
}

/// A reactive author style stays reactive and re-merges the overlay
/// on every resolution — the property the legacy class splice lacked
/// (the backend's full-`className` re-apply wiped it).
#[test]
fn style_overrides_wrap_reactive_author_source() {
    let author = Rc::new(StyleSheet::r#static(StyleRules {
        background: Some(Tokenized::Literal(Color("#abcdef".into()))),
        ..Default::default()
    }));
    let el: crate::element::Element = crate::view(Vec::new())
        .with_style({
            let author = author.clone();
            move || StyleApplication::new(author.clone())
        })
        .into();
    let el = el.with_style_overrides(fill_overlay());
    let f = match source_of(&el) {
        crate::sources::StyleSource::Reactive(f) => f,
        _ => panic!("reactive author style should stay Reactive"),
    };
    // Every invocation re-merges the overlay.
    for _ in 0..2 {
        let r = resolve(&f());
        assert_eq!(r.position, Some(Position::Absolute));
        assert_eq!(
            r.background,
            Some(Tokenized::Literal(Color("#abcdef".into()))),
            "author background survives through the wrap"
        );
    }
}

#[test]
fn pressable_handler_is_born_batched() {
    // The architecture's payoff: a handler attached through a core
    // builder (here `pressable`) auto-batches at the point the backend
    // invokes it — no per-backend `batch()` needed. Two writes in the
    // handler wake a shared subscriber once, not twice.
    use std::cell::Cell;
    use std::rc::Rc;
    let a = Signal::new(0i32);
    let b = Signal::new(0i32);
    let runs = Rc::new(Cell::new(0));
    let r = runs.clone();
    let _e = Effect::new(move || {
        let _ = (a.get(), b.get());
        r.set(r.get() + 1);
    });
    assert_eq!(runs.get(), 1);

    let pressable = crate::pressable(Vec::new(), move || {
        a.set(1);
        b.set(2);
    });
    let crate::Element::Pressable { on_click, .. } = pressable.primitive else {
        panic!("pressable did not build a Pressable element");
    };
    // Simulate the backend firing the stored handler on a tap.
    on_click();
    assert_eq!(runs.get(), 2, "born-batched handler => one re-run for two writes");
}

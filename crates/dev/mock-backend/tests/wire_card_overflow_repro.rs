//! Investigation of arena finding #8: "the wgpu replay screenshot
//! reproducibly crashed `aas-session-web` with a stack overflow whenever
//! the scene contained idea-ui `Card` rows." Runs the exact crashing chain
//! — real walker → wire record → replay → offscreen GPU — on real Card
//! scenes, on a 16MB-stack thread matching the sidecar.
//!
//! FINDINGS (2026-07-22):
//! * The reported symptom does NOT reproduce on the current tree: static
//!   Cards, 50 Cards in a `scroll_view`, and the faithful "todo row" shape
//!   (Card + text_input + toggle + button in a scroll_view) all replay to a
//!   PNG cleanly. The originally-blamed one-shot replay path is not the
//!   culprit — the crash was likely specific to the original app's
//!   structure or already fixed by intervening framework work.
//! * There IS a real latent robustness gap these tests surfaced: the render
//!   pipeline recurses on tree DEPTH at multiple un-guarded sites, so a
//!   pathologically deep tree (thousands of levels) overflows the stack and
//!   aborts the sidecar. A `walk`-only depth guard is insufficient — with
//!   `walk` capped, a 5000-deep tree still overflows (in Taffy layout or the
//!   recursive `Drop` of the deep `Rc` node tree). A complete fix spans
//!   several sites (shrink `walk`'s frame + iterative/guarded layout +
//!   non-recursive teardown) and is tracked as a scoped follow-up.
//!
//! Run: `cargo test -p mock-backend --features screenshot --test wire_card_overflow_repro -- --nocapture`

#![cfg(feature = "screenshot")]

use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::components::card::variant;
use idea_ui::{Card, CardProps};
use mock_backend::screenshot_app;
use runtime_core::{text, ui, Element, IntoElement};

fn card_list(n: usize, in_scroll: bool) -> Element {
    install_idea_theme(light_theme());
    let rows: Vec<Element> = (0..n)
        .map(|i| {
            Card(CardProps {
                variant: variant::Elevated.into(),
                children: vec![text(format!("Card row {i}")).into_element()],
                ..Default::default()
            })
        })
        .collect();
    if in_scroll {
        ui! { view() { scroll_view() { rows } } }
    } else {
        ui! { view() { rows } }
    }
}

/// Run the screenshot on a 16MB-stack thread — matching the real
/// `aas-session` sidecar (`sidecar.rs`: `.stack_size(16 * 1024 * 1024)`).
/// A plain test thread has a smaller stack, so this keeps the repro
/// faithful to where the crash actually happens.
fn run(w: u32, h: u32, app: impl Fn() -> Element + Send + 'static) -> Result<(), String> {
    std::thread::Builder::new()
        .name("aas-session-repro".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || match screenshot_app(w, h, app) {
            Ok(png) => {
                assert!(!png.is_empty(), "replay produced an empty PNG");
                Ok(())
            }
            Err(e) if e.contains("adapter") || e.contains("device") => {
                eprintln!("[repro] skipped — no wgpu adapter: {e}");
                Ok(())
            }
            Err(e) => Err(e),
        })
        .expect("spawn repro thread")
        .join()
        .expect("repro thread panicked/overflowed")
}

#[test]
fn repro_small_card_list() {
    run(400, 600, || card_list(5, false)).unwrap();
}

#[test]
fn repro_many_cards_in_scroll_view() {
    // The real crash scene: a scrollable list of many Card rows.
    run(400, 600, || card_list(50, true)).unwrap();
}

#[test]
fn repro_todo_rows_cards_with_interactive_controls() {
    // Faithful to the crash scene (todo_app_0): Card rows each containing a
    // text_input (native input the finding says the replay "doesn't draw"),
    // a button, and a toggle — in a scroll_view.
    run(400, 700, || {
        install_idea_theme(light_theme());
        let draft = runtime_core::signal(String::new());
        let rows: Vec<Element> = (0..8)
            .map(|i| {
                Card(CardProps {
                    variant: variant::Elevated.into(),
                    children: ui_children(draft, i),
                    ..Default::default()
                })
            })
            .collect();
        ui! {
            view() {
                text_input(value = draft, on_change = move |t| draft.set(t))
                scroll_view() { rows }
            }
        }
    })
    .unwrap();
}

fn ui_children(draft: runtime_core::Signal<String>, i: usize) -> Vec<Element> {
    let done = runtime_core::signal(false);
    vec![ui! {
        view() {
            toggle(value = done, on_change = move |v| done.set(v))
            text { "Item {i}" }
            text_input(value = draft, on_change = move |t| draft.set(t))
            button(label = "Delete", on_click = move || {})
        }
    }]
}

fn nested_cards(depth: usize) -> Element {
    install_idea_theme(light_theme());
    let mut node = text("leaf").into_element();
    for _ in 0..depth {
        node = Card(CardProps {
            variant: variant::Elevated.into(),
            children: vec![node],
            ..Default::default()
        });
    }
    ui! { view() { node } }
}

#[test]
fn moderately_nested_cards_render() {
    // A generously-deep-but-sane tree (well within any real UI) renders.
    run(400, 600, || nested_cards(40)).unwrap();
}

// Documents the un-fixed multi-site depth-recursion gap: a pathologically
// deep tree overflows the stack across several recursion sites (proven: it
// still overflows with `walk` guarded — Taffy layout / recursive Rc Drop are
// the others). `#[ignore]` because it ABORTS the process (can't be caught),
// so it can't run in the normal suite; run it explicitly when working the
// full multi-site fix: `cargo test ... -- --ignored deep_tree_overflow`.
#[test]
#[ignore = "aborts the process (stack overflow); documents the unfixed multi-site depth-recursion gap"]
fn deep_tree_overflow_multi_site() {
    run(400, 600, || nested_cards(5000)).unwrap();
}

#[test]
fn repro_cards_in_scroll_no_wrapper_root() {
    // scroll_view directly as root child, cards inside.
    run(400, 600, || {
        install_idea_theme(light_theme());
        let rows: Vec<Element> = (0..30)
            .map(|i| {
                Card(CardProps {
                    variant: variant::Elevated.into(),
                    children: vec![text(format!("row {i}")).into_element()],
                    ..Default::default()
                })
            })
            .collect();
        ui! { scroll_view() { rows } }
    })
    .unwrap();
}

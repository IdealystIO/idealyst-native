//! Smoke app for the web backend's new-core boot path (P3b).
//!
//! Everything here is DIRECT vocabulary-builder calls — no `ui!`, no
//! `jsx!` — deliberately, so this crate proves the
//! `runtime_scene` registry-dispatch render path independent of the
//! parallel P3a macro-lowering work (this is the sanctioned deviation
//! from CLAUDE.md §9.2: the macro can't target the new core yet, and
//! this crate exists to gate the layer *under* the macro).
//!
//! Coverage: static + reactive `text`, `button` (event → staged write →
//! driver flush), a two-way `toggle`, a structural Dyn hole (closure
//! child), and a keyed list with add/remove/reverse (keyed
//! reconciliation against the real DOM).

use runtime_core::{Length, StyleRules, Tokenized};
use runtime_scene::{keyed, Element};
use runtime_vocabulary::builders::IntoSceneElement;
use runtime_vocabulary::{button, text, toggle, view};
use runtime_world::signal;
use wasm_bindgen::prelude::*;

/// A minimal literal style — exercises the `StyleOps` delegation
/// (dynamic class minting) on the new-core path.
fn padded_column() -> StyleRules {
    StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(16.0))),
        gap: Some(Tokenized::Literal(Length::Px(8.0))),
        ..StyleRules::default()
    }
}

/// The app tree. Runs inside `World::enter` (the boot path wraps it), so
/// the free `signal()` constructor works; these top-level signals are
/// world-root-owned and live for the page.
pub fn app() -> Element {
    let count = signal(0i32);
    let on = signal(false);
    let rows = signal(vec![1u32, 2, 3]);
    let next_row = signal(4u32);

    view()
        .style(padded_column())
        .child(text().content("New-core web smoke"))
        .child(text().content(move || format!("count = {}", count.get())))
        .child(
            button()
                .label("Increment")
                .on_press(move || count.update(|n| n + 1)),
        )
        .child(toggle().value(on).on_change(move |v| on.set(v)))
        // Structural Dyn hole: a closure child rebuilds when its reads
        // change (`SceneChild` lowers it to `dyn_element`).
        .child(move || {
            if on.get() {
                view()
                    .child(text().content("toggle is ON"))
                    .into_scene_element()
            } else {
                text().content("toggle is OFF").into_scene_element()
            }
        })
        .child(
            button()
                .label("Add row")
                .on_press(move || {
                    let id = next_row.peek();
                    next_row.set(id + 1);
                    rows.update(move |r| {
                        let mut r = r.clone();
                        r.push(id);
                        r
                    });
                }),
        )
        .child(
            button()
                .label("Remove first")
                .on_press(move || {
                    rows.update(|r| r.iter().copied().skip(1).collect())
                }),
        )
        .child(
            button()
                .label("Reverse")
                .on_press(move || {
                    rows.update(|r| r.iter().rev().copied().collect())
                }),
        )
        // Keyed list: rows keep identity across edits (4-pass reconcile).
        .child(keyed(
            move || rows.get(),
            |n| *n,
            |n| text().content(format!("row #{n}")).build(),
        ))
        .build()
}

#[wasm_bindgen(start)]
pub fn main() {
    // Worker-context guard + panic hook: same conventions as the
    // CLI-generated wrappers.
    if web_sys::window().is_none() {
        return;
    }
    console_error_panic_hook::set_once();
    backend_web::install_logger();
    backend_web::newcore::start(app);
}

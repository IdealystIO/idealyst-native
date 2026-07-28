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

use std::rc::Rc;

use runtime_core::{
    Color, Length, StyleApplication, StyleRules, StyleSheet, TokenEntry, TokenValue, Tokenized,
};
use runtime_scene::{keyed, Element};
use runtime_vocabulary::builders::IntoSceneElement;
use runtime_vocabulary::{button, text, theme, toggle, view};
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

// ===========================================================================
// P3c styled section: sheet engine + theme tokens + hover overlay
// ===========================================================================

/// The two theme palettes the "Swap theme" button cycles. Web delivers
/// these as `:root` CSS variables (`install_tokens`/`update_tokens`
/// through the per-world theme driver); the card's `var(--color-surface)`
/// re-tints via the cascade with NO re-apply (the cohort short-circuit).
fn palette(dark: bool) -> [TokenEntry; 2] {
    if dark {
        [
            TokenEntry { name: "color-surface", value: TokenValue::Color(Color("#1e293b".into())) },
            TokenEntry { name: "color-ink", value: TokenValue::Color(Color("#f8fafc".into())) },
        ]
    } else {
        [
            TokenEntry { name: "color-surface", value: TokenValue::Color(Color("#e2e8f0".into())) },
            TokenEntry { name: "color-ink", value: TokenValue::Color(Color("#0f172a".into())) },
        ]
    }
}

/// A `stylesheet!`-shaped sheet built by hand (this crate stays
/// macro-free by design — module docs): token-referencing base + a
/// `size` variant + a `state hovered` overlay. On web the overlay is
/// realized as a `:hover` pseudo-class rule (`apply_styled_variants`),
/// so hovering the card must thicken its border with zero Rust work.
fn card_sheet() -> Rc<StyleSheet> {
    fn side(v: f32) -> Option<Tokenized<Length>> {
        Some(Tokenized::Literal(Length::Px(v)))
    }
    Rc::new(
        StyleSheet::new(|_vs| StyleRules {
            background: Some(Tokenized::token("color-surface", Color("#e2e8f0".into()))),
            color: Some(Tokenized::token("color-ink", Color("#0f172a".into()))),
            padding_top: side(8.0),
            padding_right: side(8.0),
            padding_bottom: side(8.0),
            padding_left: side(8.0),
            border_top_left_radius: side(6.0),
            border_top_right_radius: side(6.0),
            border_bottom_left_radius: side(6.0),
            border_bottom_right_radius: side(6.0),
            border_top_width: Some(Tokenized::Literal(1.0)),
            border_right_width: Some(Tokenized::Literal(1.0)),
            border_bottom_width: Some(Tokenized::Literal(1.0)),
            border_left_width: Some(Tokenized::Literal(1.0)),
            border_top_color: Some(Tokenized::Literal(Color("#64748b".into()))),
            border_right_color: Some(Tokenized::Literal(Color("#64748b".into()))),
            border_bottom_color: Some(Tokenized::Literal(Color("#64748b".into()))),
            border_left_color: Some(Tokenized::Literal(Color("#64748b".into()))),
            ..StyleRules::default()
        })
        .variant("size", "large", |_vs| StyleRules {
            padding_top: side(24.0),
            padding_right: side(24.0),
            padding_bottom: side(24.0),
            padding_left: side(24.0),
            ..StyleRules::default()
        })
        .variant("size", "medium", |_vs| StyleRules::default())
        .variant_default("size", "medium")
        .variant("__state_hovered", "on", |_vs| StyleRules {
            border_top_width: Some(Tokenized::Literal(4.0)),
            border_right_width: Some(Tokenized::Literal(4.0)),
            border_bottom_width: Some(Tokenized::Literal(4.0)),
            border_left_width: Some(Tokenized::Literal(4.0)),
            ..StyleRules::default()
        }),
    )
}

/// The styled section: a static-sheet card (cohort path — on web the
/// theme swap re-tints via the `var()` cascade), a large-variant card,
/// and the theme-swap button.
fn styled_section() -> Element {
    let dark = signal(false);
    // Captured at BUILD time: event handlers run outside `World::enter`
    // (the flush driver commits after dispatch), so the swap handler
    // needs the ctx handle, not the ambient free fn — same capture
    // discipline as world signals. See `runtime_vocabulary::theme`.
    let theme_ctx = theme::theme_ctx();
    view()
        .child(text().content("Styled (P3c sheet engine)"))
        .child(
            view()
                .style(StyleApplication::new(card_sheet()))
                .child(text().content("themed card — hover me")),
        )
        .child(
            view()
                .style(StyleApplication::new(card_sheet()).with("size", "large"))
                .child(text().content("large variant")),
        )
        .child(button().label("Swap theme").on_press(move || {
            let next = !dark.peek();
            dark.set(next);
            theme_ctx.update_tokens(&palette(next));
        }))
        .into_scene_element()
}

/// The app tree. Runs inside `World::enter` (the boot path wraps it), so
/// the free `signal()` constructor works; these top-level signals are
/// world-root-owned and live for the page.
pub fn app() -> Element {
    let count = signal(0i32);
    let on = signal(false);
    let rows = signal(vec![1u32, 2, 3]);
    let next_row = signal(4u32);

    // Install the initial theme before the first styled mount, the
    // normal app shape — the first sheet attach delivers it.
    theme::install_tokens(&palette(false));

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
        .child(styled_section())
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

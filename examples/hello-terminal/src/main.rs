//! Counter + animation demo for the ASCII / terminal backend.
//!
//! Everything in this file uses pure framework primitives — the same
//! tree compiles, unchanged, against iOS / Android / web. The only
//! terminal-specific code is the host crate that boots crossterm.
//!
//! Features:
//! - Counter with `[ - ]` / `[ + ]` buttons (mouse or `+` / `-` keys).
//! - Animated background color on the counter card: when the value
//!   changes, the card flashes warm then settles back via a tween.
//! - Animated translate on the headline: a gentle sine bob driven by
//!   the framework's `raf_loop`.
//! - A `Toggle` (idea-ui style) you can click to show / hide an
//!   `ActivityIndicator` braille spinner.
//! - Reset row + status line; `q` / `Esc` / `Ctrl-C` quits.
//!
//! Run with:
//!
//! ```text
//! cargo run -p hello-terminal
//! ```

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use framework_core::animation::{AnimProp, TweenTo};
use framework_core::primitives::activity_indicator::activity_indicator;
use framework_core::primitives::toggle::toggle;
use framework_core::{
    animated, button, children, effect, on_cleanup, pressable, raf_loop_scoped, signal,
    text, view, when, AlignItems, Color, FlexDirection, JustifyContent, Length, Primitive,
    Signal, StyleRules, StyleSheet, Tokenized,
};
use host_terminal::{KeyCode, KeyEvent, KeyEventKind, RunOptions};

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn pct(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Percent(v))
}

fn cval(s: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(s.into()))
}

fn sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

/// Shared state between `app()` and the host's `on_key` callback —
/// kept on `main`'s stack (NOT in a `thread_local!`) so it drops
/// before the reactive arena TLS does on process exit.
#[derive(Clone)]
struct Shared {
    count: Rc<Cell<Option<Signal<i32>>>>,
    show_spinner: Rc<Cell<Option<Signal<bool>>>>,
}

fn app(shared: Shared) -> Primitive {
    let count = signal!(0i32);
    let show_spinner = signal!(false);
    shared.count.set(Some(count));
    shared.show_spinner.set(Some(show_spinner));

    // ---- Animated values -----------------------------------------
    //
    // `card_bg` is a 4-channel sRGB tuple that we'll bind to the
    // counter card's BackgroundColor. On every count change we kick
    // off a 400ms tween from "flash warm" back to the resting blue.
    let card_bg = animated!((0.10_f32, 0.12_f32, 0.18_f32, 1.0_f32));
    // `title_translate_y` bobs the headline up and down ~1 cell.
    let title_translate_y = animated!(0.0_f32);

    // Per-count effect: re-fire the flash tween whenever the value
    // changes. The effect re-runs because `count.get()` subscribes
    // it to the count signal.
    let card_bg_for_effect = card_bg.clone();
    let count_for_effect = count;
    effect!({
        let _ = count_for_effect.get();
        // Snap to warm orange…
        card_bg_for_effect.set((0.95, 0.50, 0.20, 1.0));
        // …then tween back to resting blue over 600ms.
        let card_bg_for_tween = card_bg_for_effect.clone();
        let task = framework_core::after_ms(20, move || {
            card_bg_for_tween.animate(
                TweenTo::new((0.10, 0.12, 0.18, 1.0), Duration::from_millis(600))
                    .ease_in_out(),
            );
        });
        on_cleanup(move || drop(task));
    });

    // Gentle bobbing: raf_loop reads frame time and writes a sin
    // wave into `title_translate_y`. `raf_loop_scoped` cancels the
    // loop when the surrounding scope drops.
    let translate_for_bob = title_translate_y.clone();
    let start = std::time::Instant::now();
    raf_loop_scoped(move || {
        let t = start.elapsed().as_secs_f32();
        translate_for_bob.set((t * 1.5).sin() * 0.9);
    });

    // ---- Stylesheets --------------------------------------------
    let page = sheet(StyleRules {
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        background: Some(cval("#0a0c11")),
        color: Some(cval("#dde2ee")),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        gap: Some(px(1.0)),
        padding_top: Some(px(2.0)),
        padding_bottom: Some(px(2.0)),
        ..Default::default()
    });

    let title_style = sheet(StyleRules {
        color: Some(cval("#ffd28b")),
        ..Default::default()
    });

    let subtitle_style = sheet(StyleRules {
        color: Some(cval("#7a8298")),
        margin_bottom: Some(px(1.0)),
        ..Default::default()
    });

    let counter_card = sheet(StyleRules {
        background: Some(cval("#1a1f2e")),
        color: Some(cval("#7fe8d6")),
        padding_top: Some(px(1.0)),
        padding_bottom: Some(px(1.0)),
        padding_left: Some(px(4.0)),
        padding_right: Some(px(4.0)),
        border_top_width: Some(Tokenized::Literal(1.0)),
        border_bottom_width: Some(Tokenized::Literal(1.0)),
        border_left_width: Some(Tokenized::Literal(1.0)),
        border_right_width: Some(Tokenized::Literal(1.0)),
        min_width: Some(px(24.0)),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });

    let counter_text = sheet(StyleRules {
        color: Some(cval("#ffffff")),
        ..Default::default()
    });

    let button_row = sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        gap: Some(px(2.0)),
        ..Default::default()
    });

    let inc_button = sheet(StyleRules {
        background: Some(cval("#1d8a5a")),
        color: Some(cval("#ffffff")),
        padding_left: Some(px(2.0)),
        padding_right: Some(px(2.0)),
        min_width: Some(px(12.0)),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });

    let dec_button = sheet(StyleRules {
        background: Some(cval("#a64646")),
        color: Some(cval("#ffffff")),
        padding_left: Some(px(2.0)),
        padding_right: Some(px(2.0)),
        min_width: Some(px(12.0)),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });

    let reset_button = sheet(StyleRules {
        background: Some(cval("#3a3f55")),
        color: Some(cval("#dde2ee")),
        padding_left: Some(px(2.0)),
        padding_right: Some(px(2.0)),
        min_width: Some(px(26.0)),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    });

    let toggle_row = sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        gap: Some(px(2.0)),
        align_items: Some(AlignItems::Center),
        margin_top: Some(px(1.0)),
        ..Default::default()
    });

    let spinner_label_style = sheet(StyleRules {
        color: Some(cval("#7a8298")),
        ..Default::default()
    });

    let help_style = sheet(StyleRules {
        color: Some(cval("#7a8298")),
        margin_top: Some(px(1.0)),
        ..Default::default()
    });

    // ---- Refs for animation bindings ----------------------------
    // `bind` / `bind_color` accept `Ref<ViewHandle>` only — to bob
    // a Text we wrap it in a View and bind that. Same posture as
    // the macOS / iOS backends; the animation system targets view-
    // level transforms.
    let card_ref: framework_core::Ref<framework_core::ViewHandle> = framework_core::Ref::new();
    let title_wrap_ref: framework_core::Ref<framework_core::ViewHandle> =
        framework_core::Ref::new();

    // Bind the animated values to the refs *after* the refs are
    // declared. `card_bg.bind_color(...)` and
    // `title_translate_y.bind(...)` install per-frame writers that
    // route through `Backend::set_animated_*` on every animation
    // tick.
    card_bg.bind_color(card_ref, AnimProp::BackgroundColor);
    title_translate_y.bind(title_wrap_ref, AnimProp::TranslateY);

    // ---- Tree ----------------------------------------------------
    let count_for_label = count;
    let count_label = move || format!("  {}  ", count_for_label.get());

    let count_for_inc = count;
    let count_for_dec = count;
    let count_for_reset = count;
    let spinner_for_toggle = show_spinner;
    let show_spinner_for_label = show_spinner;
    let spinner_status_label = move || {
        if show_spinner_for_label.get() {
            "loading…".to_string()
        } else {
            "idle".to_string()
        }
    };

    view(children![
        view(children![
            text("Idealyst ASCII Backend").with_style(title_style),
        ])
        .bind(title_wrap_ref),
        text("flex layout, animations, mouse + keyboard — all in your shell")
            .with_style(subtitle_style),
        view(children![text(count_label).with_style(counter_text)])
            .with_style(counter_card)
            .bind(card_ref),
        view(children![
            button("[ - ]", move || {
                count_for_dec.update(|n| *n -= 1);
            })
            .with_style(dec_button),
            button("[ + ]", move || {
                count_for_inc.update(|n| *n += 1);
            })
            .with_style(inc_button),
        ])
        .with_style(button_row),
        pressable(
            children![text("[ reset (or press r) ]")],
            move || count_for_reset.set(0),
        )
        .with_style(reset_button),
        view(children![
            toggle(show_spinner, move |new| spinner_for_toggle.set(new)),
            text("show spinner").with_style(spinner_label_style.clone()),
            when(
                move || show_spinner.get(),
                || activity_indicator().into(),
                || text("").into(),
            ),
            text(spinner_status_label).with_style(spinner_label_style),
        ])
        .with_style(toggle_row),
        text("click buttons / toggle, or use  +  -  r  to drive the counter")
            .with_style(help_style.clone()),
        text("press  q  or  Esc  to quit").with_style(help_style),
    ])
    .with_style(page)
    .into()
}

fn main() {
    let shared = Shared {
        count: Rc::new(Cell::new(None)),
        show_spinner: Rc::new(Cell::new(None)),
    };

    let shared_for_app = shared.clone();
    let app_closure = move || app(shared_for_app.clone());

    let shared_for_key = shared.clone();
    let on_key: Rc<dyn Fn(&KeyEvent) -> bool> = Rc::new(move |key: &KeyEvent| {
        if key.kind != KeyEventKind::Press && key.kind != KeyEventKind::Repeat {
            return false;
        }
        let Some(count_sig) = shared_for_key.count.get() else { return false };
        match key.code {
            KeyCode::Char('+') | KeyCode::Char('=') => {
                count_sig.update(|n| *n += 1);
                true
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                count_sig.update(|n| *n -= 1);
                true
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                count_sig.set(0);
                true
            }
            KeyCode::Char(' ') => {
                if let Some(sp) = shared_for_key.show_spinner.get() {
                    sp.update(|b| *b = !*b);
                }
                true
            }
            _ => false,
        }
    });

    let opts = RunOptions {
        target_fps: 30,
        on_key: Some(on_key),
    };

    if let Err(e) = host_terminal::run(app_closure, opts) {
        eprintln!("hello-terminal exited with error: {e}");
        std::process::exit(1);
    }
}

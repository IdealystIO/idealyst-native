//! Regression test for the theme cohort + `update_tokens` batching.
//!
//! `set_idea_theme(light_theme())` / `set_idea_theme(dark_theme())`
//! pushes ~50+ token values through `update_tokens`, which fires the
//! per-token `Signal<TokenValue>` registry inside `reactive::batch` so
//! every subscribed Effect re-runs exactly ONCE per swap rather than
//! once per token it reads (see [[project_update_tokens_batch]] memory
//! entry). The cohort driver Effect is what actually flushes the new
//! `:root` variables to the web backend.
//!
//! This app gives that machinery a workout we can eyeball: themed
//! components (Badge / Alert / Card / Btn all subscribe to several
//! tokens) and a toggle button that swaps the whole token set. The
//! "swaps so far" counter is a reactive sanity check — if it stops
//! incrementing, the on_click closure or its signal got pruned out.
//!
//! What to look for in the browser:
//! - On click, every surface/text color visibly flips (light ↔ dark).
//! - The swap counter increments by 1 per click.
//! - No `RuntimeError` or `panicked at` in the devtools console (the
//!   cohort driver's panic-message data and the token registry's
//!   `Signal<TokenValue>` allocations must all survive pruning).

use idea_ui::{
    dark_theme, install_idea_theme, light_theme, set_idea_theme, tone, variant, Alert, Badge,
    Btn, Card, Stack, StackGap, Typography,
};
use runtime_core::{rx, signal, ui, Element, Signal};
use std::rc::Rc;

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let is_dark: Signal<bool> = signal!(false);
    let swap_count: Signal<u32> = signal!(0);

    let toggle: Rc<dyn Fn()> = Rc::new(move || {
        let now_dark = !is_dark.get();
        is_dark.set(now_dark);
        swap_count.update(|n| *n += 1);
        if now_dark {
            set_idea_theme(dark_theme());
        } else {
            set_idea_theme(light_theme());
        }
    });

    ui! {
        view {
            Stack(gap = StackGap::Lg) {
                Typography(
                    content = "theme-swap regression test".to_string(),
                    kind = idea_ui::typography_kind::H2,
                )
                Typography(
                    content = rx!(format!(
                        "theme: {} \u{2014} swaps: {}",
                        if is_dark.get() { "dark" } else { "light" },
                        swap_count.get(),
                    )),
                )
                Btn(
                    label = "Toggle theme".to_string(),
                    on_click = toggle,
                    tone = tone::Primary,
                    variant = variant::Filled,
                )
                Card() {
                    Stack(gap = StackGap::Sm) {
                        Typography(content = "Card surface (tokenized background)".to_string())
                        Stack(gap = StackGap::Xs) {
                            Badge(label = "primary".to_string(),  tone = tone::Primary,  variant = variant::Soft)
                            Badge(label = "success".to_string(),  tone = tone::Success,  variant = variant::Soft)
                            Badge(label = "danger".to_string(),   tone = tone::Danger,   variant = variant::Soft)
                            Badge(label = "warning".to_string(),  tone = tone::Warning,  variant = variant::Soft)
                            Badge(label = "info".to_string(),     tone = tone::Info,     variant = variant::Soft)
                        }
                        Alert(
                            tone = tone::Info,
                            variant = variant::Soft,
                            title = "Reactive cohort check".to_string(),
                            body = Some(
                                "Every colored chunk above subscribes to one or more theme \
                                 tokens. Click Toggle theme and they should all repaint \
                                 together in one frame."
                                    .to_string(),
                            ),
                        )
                    }
                }
            }
        }
    }
}

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

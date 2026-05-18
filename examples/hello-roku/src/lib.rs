//! End-to-end Rust-authored reactive Roku app.
//!
//! Single `app()` function covering the Phase 2 reactivity surface:
//!
//! - **Reactiveness**: a `count` signal threads through four
//!   reactive `Text` labels via `bind!`. Each label has its own
//!   `#[method]` transformer.
//! - **if/else inside `#[method]`**: `parity_label`.
//! - **`match` inside `#[method]`**: `status_label` — literal arms
//!   + a `_` fallback. Transpiler lowers to chained
//!   `if/else if/else`.
//! - **`for` loop inside `#[method]`**: `sum_to`.
//! - **`for` loop inside `ui! {}`**: the "step pip" row at the
//!   top — the macro statically expands the loop into three
//!   sibling `Text` nodes at snapshot time.
//! - **Button press**: `bind_press!(increment(count) => count)`
//!   and `bind_press!(decrement(count) => count)`.
//! - **Focus navigation across multiple buttons**: D-pad
//!   left/right cycles; OK fires the focused button. The visual
//!   highlight comes from the framework's `state hovered { ... }`
//!   stylesheet overlay — same author API web uses for CSS
//!   `:hover`, shipped to Roku as a per-state wire command.

use backend_roku::method;
use framework_core::{
    install_themes, signal, stylesheet, ui, AlignItems, Color, FlexDirection, FontWeight,
    JustifyContent, Length, Primitive, Signal, ThemeTokens, TokenEntry, TokenValue, Tokenized,
};

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// Two-variant theme. Each named token resolves to a different
/// concrete color depending on which variant is active. Stylesheets
/// reference these tokens by closing over the theme handle the
/// `stylesheet! { pub Name<Theme> { ... } }` block receives — the
/// resulting `Tokenized<Color>` carries the token name + fallback,
/// and the Roku backend ships both to the device so the BS runtime
/// can re-resolve when the active theme name changes.
#[derive(Clone)]
pub struct Theme {
    pub bg: Tokenized<Color>,
    pub fg: Tokenized<Color>,
    pub accent: Tokenized<Color>,
    pub muted: Tokenized<Color>,
    pub badge_even: Tokenized<Color>,
    pub badge_odd: Tokenized<Color>,
}

fn tok(name: &'static str, fallback: &str) -> Tokenized<Color> {
    Tokenized::token(name, Color(fallback.into()))
}

pub fn light_theme() -> Theme {
    Theme {
        bg: tok("page-bg", "#FFFFFF"),
        fg: tok("page-fg", "#1A1A1F"),
        accent: tok("accent", "#FFCC00"),
        muted: tok("muted", "#6B7280"),
        badge_even: tok("badge-even", "#10B981"),
        badge_odd: tok("badge-odd", "#F472B6"),
    }
}

pub fn dark_theme() -> Theme {
    Theme {
        bg: tok("page-bg", "#0F1115"),
        fg: tok("page-fg", "#E8EAF0"),
        accent: tok("accent", "#FFCC00"),
        muted: tok("muted", "#9099A8"),
        badge_even: tok("badge-even", "#34D399"),
        badge_odd: tok("badge-odd", "#F9A8D4"),
    }
}

impl ThemeTokens for Theme {
    fn tokens(&self) -> Vec<TokenEntry> {
        fn entry(t: &Tokenized<Color>) -> TokenEntry {
            TokenEntry {
                name: t.name().expect("theme color fields must be Tokenized::Token"),
                value: TokenValue::Color(t.value().clone()),
            }
        }
        vec![
            entry(&self.bg),
            entry(&self.fg),
            entry(&self.accent),
            entry(&self.muted),
            entry(&self.badge_even),
            entry(&self.badge_odd),
        ]
    }
}

// ---------------------------------------------------------------------------
// Stylesheets
// ---------------------------------------------------------------------------

stylesheet! {
    pub Page<Theme> {
        base(t) {
            flex_direction: FlexDirection::Column,
            gap: Length::Px(24.0),
            padding_top: Length::Px(60.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexStart,
            background: t.bg.clone(),
        }
    }
}

stylesheet! {
    pub Title<Theme> {
        base(t) {
            font_size: Length::Px(40.0),
            font_weight: FontWeight::Bold,
            color: t.fg.clone(),
        }
    }
}

stylesheet! {
    pub Counter<Theme> {
        base(t) {
            font_size: Length::Px(140.0),
            font_weight: FontWeight::Bold,
            color: t.accent.clone(),
        }
    }
}

stylesheet! {
    pub MetaRow<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(16.0),
            align_items: AlignItems::Center,
        }
    }
}

stylesheet! {
    pub MetaKey<Theme> {
        base(t) {
            font_size: Length::Px(28.0),
            color: t.muted.clone(),
        }
    }
}

stylesheet! {
    pub MetaValue<Theme> {
        base(t) {
            font_size: Length::Px(32.0),
            font_weight: FontWeight::SemiBold,
            color: t.fg.clone(),
        }
    }
}

stylesheet! {
    pub StepPipRow<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(12.0),
            align_items: AlignItems::Center,
        }
    }
}

stylesheet! {
    pub StepPip<Theme> {
        base(_t) {
            font_size: Length::Px(48.0),
            color: "#4B5563",
        }
    }
}

stylesheet! {
    pub ButtonRow<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(24.0),
            align_items: AlignItems::Center,
        }
    }
}

// Per-bucket stylesheet for the `bind_switch!` demo — five
// pre-built badges, each shown when the count_bucket method
// returns its matching value.
stylesheet! {
    pub StatusBadge<Theme> {
        base(_t) {
            font_size: Length::Px(32.0),
            font_weight: FontWeight::SemiBold,
            color: "#A5B4FC",
        }
    }
}

// Per-row dot for the `bind_repeat!` demo. The runtime clones
// this row template per row, allocating fresh node ids per
// instance — no upper bound on `count`.
stylesheet! {
    pub RepeatRow<Theme> {
        base(_t) {
            font_size: Length::Px(40.0),
            color: "#34D399",
        }
    }
}

stylesheet! {
    pub RepeatRowContainer<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(8.0),
            align_items: AlignItems::Center,
        }
    }
}

// Two stylistically distinct badges; `bind_when!` picks one or the
// other based on the parity of `count`. Same author syntax both
// branches would use on any backend — Roku's runtime toggles
// `.visible` between them.
stylesheet! {
    pub BadgeEven<Theme> {
        base(t) {
            font_size: Length::Px(36.0),
            font_weight: FontWeight::Bold,
            color: t.badge_even.clone(),
        }
    }
}

stylesheet! {
    pub BadgeOdd<Theme> {
        base(t) {
            font_size: Length::Px(36.0),
            font_weight: FontWeight::Bold,
            color: t.badge_odd.clone(),
        }
    }
}

// Button stylesheet with a `state hovered` overlay. Web's CSS
// :hover and Roku's D-pad focus both activate this — same author
// API, two compatible code paths.
stylesheet! {
    pub CounterButton<Theme> {
        base(_t) {
            min_width: Length::Px(220.0),
            min_height: Length::Px(80.0),
            background: "#2563EB",
        }
        state hovered(_t) {
            background: "#FFCC00",
        }
    }
}

// ---------------------------------------------------------------------------
// #[method] transformers — all bodies use only constructs the v0
// transpiler supports. Each one ends up in the device's
// `methods.brs` plus the generated `dispatch_method` switch.
// ---------------------------------------------------------------------------

#[method]
pub fn count_value(n: i32) -> i32 {
    n
}

#[method]
pub fn increment(n: i32) -> i32 {
    n + 1
}

#[method]
pub fn decrement(n: i32) -> i32 {
    n - 1
}

/// if/else demo.
#[method]
pub fn parity_label(n: i32) -> &'static str {
    if n % 2 == 0 {
        "Even"
    } else {
        "Odd"
    }
}

/// match demo — literal arms + `_` fallback. Transpiles to a
/// chained if/else if/else.
#[method]
pub fn status_label(n: i32) -> &'static str {
    match n {
        0 => "fresh",
        1 => "warming up",
        2 => "rolling",
        3 => "rolling",
        _ => "keep going",
    }
}

/// for-loop demo inside a `#[method]`. Sums 1 + 2 + ... + n.
#[method]
pub fn sum_to(n: i32) -> i32 {
    let mut total = 0;
    for i in 1..=n {
        total = total + i;
    }
    total
}

/// Condition for `bind_when!` — structural reactivity. The two
/// `then` / `else_` subtrees ship over the wire pre-built; the
/// device-side runtime toggles which one is visible on every
/// signal change.
#[method]
pub fn is_even(n: i32) -> bool {
    n % 2 == 0
}

/// Discriminant for `bind_switch!`. Clamps the count into a
/// small bucket so the switch arms cover every reachable value.
#[method]
pub fn count_bucket(n: i32) -> i32 {
    if n <= 0 {
        0
    } else if n == 1 {
        1
    } else if n <= 3 {
        2
    } else if n <= 6 {
        3
    } else {
        4
    }
}

/// Count source for `bind_repeat!`. Identity in this demo; in a
/// real app you might `min(count, max)` here.
#[method]
pub fn row_count(n: i32) -> i32 {
    n
}

/// Per-row label transpiled to BS. Each cloned row's synthetic
/// row-index signal seeds this method's argument, so the cloned
/// Label ends up displaying its own index.
#[method]
pub fn row_label(i: i32) -> i32 {
    i + 1
}

/// Flip the active theme name. Output goes back into the theme
/// signal; the BS runtime's theme subscriber sees the change and
/// re-applies every styled node against the new variant's tokens.
#[method]
pub fn next_theme(current: String) -> String {
    if current == "dark" {
        "light".to_string()
    } else {
        "dark".to_string()
    }
}

/// Row count derived from a `Signal<Vec<i32>>` — the list's
/// length. Transpiles to BS as `v.Count()`.
#[method]
pub fn items_count(v: Vec<i32>) -> usize {
    v.len()
}

/// Per-row content derived from the same `Signal<Vec<i32>>`:
/// looks up `v[i]` and returns the integer. The bind! inside
/// the row template subscribes to BOTH `items` and the row-index
/// signal, so mutating `items` (replacing the whole Vec via
/// `items.set(...)`) re-fires every row with the new data.
#[method]
pub fn item_at(v: Vec<i32>, i: i32) -> i32 {
    v[i as usize].clone()
}

// ---------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------

pub fn app() -> Primitive {
    // Active-theme signal. Its String value names the current
    // theme variant; `install_themes` registers both variants
    // with the backend and binds this signal as the active-theme
    // driver. The toggle button writes back through this signal
    // and the BS runtime re-applies every styled node when it
    // changes.
    let theme: Signal<String> = signal!("dark".to_string());
    install_themes(
        theme,
        &[
            ("light", light_theme()),
            ("dark", dark_theme()),
        ],
    );

    let count: Signal<i32> = signal!(0);
    // A `Signal<Vec<i32>>` — fibonacci-ish seed list. The whole
    // Vec round-trips through `serde_json::Value` to the device,
    // where BS keeps it as an `roArray` and the `#[method]`s
    // (`items_count` / `item_at`) read from it directly. Mutating
    // this signal (replacing the entire Vec via `items.set(...)`)
    // would reactively re-render every visible row + reconcile
    // row count.
    let items: Signal<Vec<i32>> = signal!(vec![1, 2, 3, 5, 8, 13, 21]);

    ui! {
        View(style = page_style()) {
            Text(style = title_style()) { "Reactivity Demo" }

            // for-loop in `ui!{}` — the macro expands this into
            // three sibling `Text` nodes at snapshot time. Pip
            // glyphs render statically; they're here to prove the
            // ui-level for syntax round-trips through the
            // snapshot pipeline.
            View(style = step_pip_row_style()) {
                for _ in 0..5 {
                    Text(style = step_pip_style()) { "•" }
                }
            }

            Text(style = counter_style()) { count_value(count) }

            View(style = meta_row_style()) {
                Text(style = meta_key_style()) { "Parity:" }
                Text(style = meta_value_style()) { parity_label(count) }
            }

            View(style = meta_row_style()) {
                Text(style = meta_key_style()) { "Status:" }
                Text(style = meta_value_style()) { status_label(count) }
            }

            View(style = meta_row_style()) {
                Text(style = meta_key_style()) { "Sum 1..N:" }
                Text(style = meta_value_style()) { sum_to(count) }
            }

            // Structural reactivity via plain `if` / `else`. The
            // `ui!` macro detects the call shape `is_even(count)`,
            // builds a `Derived<bool>` with structured metadata,
            // and emits a `Primitive::When` — same Roku wire op
            // (`BindWhen`) that `bind_when!` used to produce.
            if is_even(count) {
                Text(style = badge_even_style()) { "★ EVEN ★" }
            } else {
                Text(style = badge_odd_style()) { "✦ ODD ✦" }
            }

            // Plain `match` over a `#[method]` call. `ui!` detects
            // the structured call shape and emits a
            // `Primitive::Switch` with literal-keyed arms — same
            // Roku wire op (`BindSwitch`) that `bind_switch!`
            // produced.
            match count_bucket(count) {
                0 => { Text(style = status_badge_style()) { "○ press +1 to begin"     } },
                1 => { Text(style = status_badge_style()) { "◔ one tick on the clock" } },
                2 => { Text(style = status_badge_style()) { "◐ warming up"            } },
                3 => { Text(style = status_badge_style()) { "◕ in the groove"         } },
                _ => { Text(style = status_badge_style()) { "● keep climbing"         } },
            }

            // Plain `for` loop over a `#[method]` call. `ui!`
            // detects the structured call shape and lowers to a
            // `Primitive::Virtualizer` with a captured row
            // template; the loop binder (`i`) is a `Signal<i32>`
            // the runtime mints per row. The trailing
            // `.with_style(...)` pins the row container's flex
            // direction so rows lay out horizontally.
            for i in row_count(count) {
                Text(style = repeat_row_style()) { row_label(i) }
            }.with_style(repeat_row_container_style())

            View(style = button_row_style()) {
                Button(
                    label = "-1",
                    on_click = decrement(count) => count,
                    style = counter_button_style(),
                )
                Button(
                    label = "+1",
                    on_click = increment(count) => count,
                    style = counter_button_style(),
                )
                Button(
                    label = "Theme",
                    on_click = next_theme(theme) => theme,
                    style = counter_button_style(),
                )
            }
        }
    }
}

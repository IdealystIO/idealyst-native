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
use runtime_core::{
    signal, stylesheet, ui, AlignItems, Color, FlexDirection, FontWeight,
    JustifyContent, Length, Element, Signal, Tokenized,
};
use idea_ui::{install_themes, ThemeTokens, TokenEntry, TokenValue};

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
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: Length::Px(24.0),
            // TV safe zone — Roku design guidelines recommend 5%
            // inset on each axis so content avoids the bezel /
            // overscan area. With Percent(100) on a Virtualizer
            // anchor inside this page, the carousel sizes to the
            // inset content area automatically (no per-component
            // padding needed).
            padding_top: Length::Px(60.0),
            padding_bottom: Length::Px(60.0),
            padding_left: Length::Px(80.0),
            padding_right: Length::Px(80.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexStart,
            background: Tokenized::token("page-bg", Color("#FFFFFF".into())),
        }
    }
}

stylesheet! {
    pub Title<Theme> {
        base(_t) {
            font_size: Length::Px(40.0),
            font_weight: FontWeight::Bold,
            color: Tokenized::token("page-fg", Color("#1A1A1F".into())),
        }
    }
}

stylesheet! {
    pub Counter<Theme> {
        base(_t) {
            font_size: Length::Px(140.0),
            font_weight: FontWeight::Bold,
            color: Tokenized::token("accent", Color("#FFCC00".into())),
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
        base(_t) {
            font_size: Length::Px(28.0),
            color: Tokenized::token("muted", Color("#6B7280".into())),
        }
    }
}

stylesheet! {
    pub MetaValue<Theme> {
        base(_t) {
            font_size: Length::Px(32.0),
            font_weight: FontWeight::SemiBold,
            color: Tokenized::token("page-fg", Color("#1A1A1F".into())),
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

// Per-row card for the carousel demo. The build pipeline reads
// `background` + `color` + `font_size` off this stylesheet and
// bakes them into the generated item component's init() — so
// each Roku MarkupList/RowList cell renders as a colored card
// with centered text. Theme-aware: cards use the active theme's
// accent color (fallback baked into the BS at snapshot time).
stylesheet! {
    pub RepeatRow<Theme> {
        base(_t) {
            font_size: Length::Px(96.0),
            font_weight: FontWeight::Bold,
            color: Tokenized::token("page-bg", Color("#FFFFFF".into())),
            background: Tokenized::token("accent", Color("#FFCC00".into())),
        }
    }
}

// Section header above each carousel row — "Trending",
// "Recently Added", etc.
stylesheet! {
    pub RowLabel<Theme> {
        base(_t) {
            font_size: Length::Px(40.0),
            color: Tokenized::token("page-fg", Color("#1A1A1F".into())),
            font_weight: FontWeight::Bold,
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
        base(_t) {
            font_size: Length::Px(36.0),
            font_weight: FontWeight::Bold,
            color: Tokenized::token("badge-even", Color("#10B981".into())),
        }
    }
}

stylesheet! {
    pub BadgeOdd<Theme> {
        base(_t) {
            font_size: Length::Px(36.0),
            font_weight: FontWeight::Bold,
            color: Tokenized::token("badge-odd", Color("#F472B6".into())),
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

pub fn app() -> Element {
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

    // Four independent data rails. Each one is its own
    // `Signal<Vec<i32>>`; the `items_count` + `item_at` methods
    // are generic over the signal they're called against, so the
    // same #[method] pair drives every row. Mutating any one
    // signal reactively repopulates only that carousel.
    let trending: Signal<Vec<i32>> = signal!(vec![1, 2, 3, 5, 8, 13, 21, 34, 55]);
    let recent: Signal<Vec<i32>> = signal!(vec![10, 20, 30, 40, 50, 60, 70, 80]);
    let top_picks: Signal<Vec<i32>> = signal!(vec![100, 200, 300, 400, 500, 600, 700, 800, 900, 1000]);
    let popular: Signal<Vec<i32>> = signal!(vec![7, 14, 21, 28, 35, 42, 49, 56, 63, 70]);

    ui! {
        View(style = page_style()) {
            Text(style = title_style()) { "Reactivity Demo" }

            // ----- Row 1: Trending -----
            Text(style = row_label_style()) { "Trending" }
            for i in items_count(trending) {
                Text(style = repeat_row_style()) { item_at(trending, i) }
            }.with_style(repeat_row_container_style()).horizontal(true)

            // ----- Row 2: Recently Added -----
            Text(style = row_label_style()) { "Recently Added" }
            for i in items_count(recent) {
                Text(style = repeat_row_style()) { item_at(recent, i) }
            }.with_style(repeat_row_container_style()).horizontal(true)

            // ----- Row 3: Top Picks -----
            Text(style = row_label_style()) { "Top Picks for You" }
            for i in items_count(top_picks) {
                Text(style = repeat_row_style()) { item_at(top_picks, i) }
            }.with_style(repeat_row_container_style()).horizontal(true)

            // ----- Row 4: Popular -----
            // Fourth row pushes the page taller than the visible
            // viewport, so the BS-side `maybeScrollToFocused`
            // shifts the page vertically when D-pad focus lands
            // on this carousel or the Theme button below.
            Text(style = row_label_style()) { "Popular Right Now" }
            for i in items_count(popular) {
                Text(style = repeat_row_style()) { item_at(popular, i) }
            }.with_style(repeat_row_container_style()).horizontal(true)

            View(style = button_row_style()) {
                Button(
                    label = "Theme",
                    on_click = next_theme(theme) => theme,
                    style = counter_button_style(),
                )
            }
        }
    }
}

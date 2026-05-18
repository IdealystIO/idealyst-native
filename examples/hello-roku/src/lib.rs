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
    bind, bind_press, bind_repeat, bind_switch, bind_when, install_theme, signal,
    stylesheet, ui, AlignItems, FlexDirection, FontWeight, JustifyContent, Length,
    Primitive, Signal, ThemeTokens, TokenEntry,
};

#[derive(Clone)]
pub struct Theme;

impl ThemeTokens for Theme {
    fn tokens(&self) -> Vec<TokenEntry> {
        Vec::new()
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
            padding_top: Length::Px(60.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::FlexStart,
        }
    }
}

stylesheet! {
    pub Title<Theme> {
        base(_t) {
            font_size: Length::Px(40.0),
            font_weight: FontWeight::Bold,
            color: "#FFFFFF",
        }
    }
}

stylesheet! {
    pub Counter<Theme> {
        base(_t) {
            font_size: Length::Px(140.0),
            font_weight: FontWeight::Bold,
            color: "#FFCC00",
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
            color: "#9CA3AF",
        }
    }
}

stylesheet! {
    pub MetaValue<Theme> {
        base(_t) {
            font_size: Length::Px(32.0),
            font_weight: FontWeight::SemiBold,
            color: "#FFFFFF",
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
        base(_t) {
            font_size: Length::Px(36.0),
            font_weight: FontWeight::Bold,
            color: "#10B981",
        }
    }
}

stylesheet! {
    pub BadgeOdd<Theme> {
        base(_t) {
            font_size: Length::Px(36.0),
            font_weight: FontWeight::Bold,
            color: "#F472B6",
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
    install_theme(Theme);

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

            Text(style = counter_style()) { bind!(count_value(count)) }

            View(style = meta_row_style()) {
                Text(style = meta_key_style()) { "Parity:" }
                Text(style = meta_value_style()) { bind!(parity_label(count)) }
            }

            View(style = meta_row_style()) {
                Text(style = meta_key_style()) { "Status:" }
                Text(style = meta_value_style()) { bind!(status_label(count)) }
            }

            View(style = meta_row_style()) {
                Text(style = meta_key_style()) { "Sum 1..N:" }
                Text(style = meta_value_style()) { bind!(sum_to(count)) }
            }

            // bind_when! — *structural* reactivity. Both branches
            // ship pre-built; the device toggles which subtree is
            // visible based on `is_even(count)`.
            bind_when!(is_even(count),
                then  = ui! { Text(style = badge_even_style()) { "★ EVEN ★" } },
                else_ = ui! { Text(style = badge_odd_style())  { "✦ ODD ✦"  } },
            )

            // bind_switch! — N-way conditional. Five pre-built
            // arms, the matching one shows based on the
            // count_bucket method's return value.
            bind_switch!(count_bucket(count),
                0 => ui! { Text(style = status_badge_style()) { "○ press +1 to begin"  } },
                1 => ui! { Text(style = status_badge_style()) { "◔ one tick on the clock" } },
                2 => ui! { Text(style = status_badge_style()) { "◐ warming up"          } },
                3 => ui! { Text(style = status_badge_style()) { "◕ in the groove"       } },
                _ => ui! { Text(style = status_badge_style()) { "● keep climbing"       } },
            )

            // bind_repeat! reading from a `Signal<Vec<i32>>`. Row
            // count tracks the list length; each row binds to
            // `item_at(items, i)` so per-row content is the
            // corresponding list element. Mutating `items` would
            // reactively re-render — same path that powers `count`
            // updates above.
            bind_repeat!(items_count(items),
                row = |i| ui! { Text(style = repeat_row_style()) { bind!(item_at(items, i)) } },
                style = repeat_row_container_style(),
            )

            View(style = button_row_style()) {
                Button(
                    label = "-1",
                    on_click = bind_press!(decrement(count) => count),
                    style = counter_button_style(),
                )
                Button(
                    label = "+1",
                    on_click = bind_press!(increment(count) => count),
                    style = counter_button_style(),
                )
            }
        }
    }
}

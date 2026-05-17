//! hello-roku — styled demo that exercises the Phase 1 flex engine.
//!
//! The screen lays out as:
//!
//! ```text
//!   ┌─────────────────────────────────────────┐
//!   │  Hello, Roku!                           │  ← Title (font 72, accent color)
//!   │  Authored in Rust ...                   │  ← Subtitle (font 32, muted)
//!   │                                         │
//!   │  ┌──────────┐ ┌──────────┐ ┌──────────┐ │  ← Cards row (flex_dir Row,
//!   │  │  Layout  │ │  Method  │ │ Reactive │ │     justify SpaceBetween,
//!   │  │   Flex   │ │  → BRS   │ │   Soon   │ │     each card flex_grow 1)
//!   │  └──────────┘ └──────────┘ └──────────┘ │
//!   │                                         │
//!   │                                  v0 ... │  ← Footer (justify FlexEnd)
//!   └─────────────────────────────────────────┘
//! ```
//!
//! Features exercised:
//!   - `background` / `color`                       (color)
//!   - `font_size` / `font_weight`                  (typography)
//!   - `flex_direction: Row`                        (cards stack horizontally)
//!   - `justify_content: SpaceBetween, FlexEnd`     (main-axis alignment)
//!   - `align_items: Center, Stretch`               (cross-axis alignment)
//!   - `gap`                                        (spacing between siblings)
//!   - `padding`                                    (per-side fan-out)
//!   - `flex_grow`                                  (cards share width equally)

use backend_roku::method;
use framework_core::{
    install_theme, stylesheet, ui, AlignItems, FlexDirection, FontWeight,
    JustifyContent, Length, Primitive, ThemeTokens, TokenEntry,
};

/// Trivial theme — no tokens. The framework requires `install_theme`
/// before resolving stylesheets, but our stylesheets use literal
/// values rather than referencing `t.*` so the theme is just a
/// placeholder.
#[derive(Clone)]
pub struct Theme;

impl ThemeTokens for Theme {
    fn tokens(&self) -> Vec<TokenEntry> {
        Vec::new()
    }
}

stylesheet! {
    pub Page<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: Length::Px(40.0),
        }
    }
}

stylesheet! {
    pub Title<Theme> {
        base(_t) {
            font_size: Length::Px(72.0),
            font_weight: FontWeight::Bold,
            color: "#FFCC00",
        }
    }
}

stylesheet! {
    pub Subtitle<Theme> {
        base(_t) {
            font_size: Length::Px(28.0),
            color: "#9CA3AF",
        }
    }
}

stylesheet! {
    pub Header<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: Length::Px(8.0),
        }
    }
}

stylesheet! {
    pub CardsRow<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(24.0),
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Stretch,
        }
    }
}

stylesheet! {
    pub Card<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            background: "#1F2937",
            padding: 32,
            gap: 12,
            flex_grow: 1.0,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            min_height: Length::Px(160.0),
        }
    }
}

stylesheet! {
    pub CardLabel<Theme> {
        base(_t) {
            font_size: Length::Px(24.0),
            color: "#9CA3AF",
        }
    }
}

stylesheet! {
    pub CardValue<Theme> {
        base(_t) {
            font_size: Length::Px(40.0),
            font_weight: FontWeight::Bold,
            color: "#FFFFFF",
        }
    }
}

stylesheet! {
    pub Footer<Theme> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::FlexEnd,
        }
    }
}

stylesheet! {
    pub FooterText<Theme> {
        base(_t) {
            font_size: Length::Px(20.0),
            color: "#6B7280",
        }
    }
}

/// Tiny `#[method]` to keep the BRS transpilation pipeline exercised.
#[method]
pub fn greeting_length(n: i32) -> i32 {
    n * 2 + 1
}

pub fn app() -> Primitive {
    install_theme(Theme);

    ui! {
        View(style = page_style()) {
            View(style = header_style()) {
                Text(style = title_style()) { "Hello, Roku!" }
                Text(style = subtitle_style()) { "Authored in Rust, rendered on-device" }
            }

            View(style = cards_row_style()) {
                View(style = card_style()) {
                    Text(style = card_label_style()) { "Layout" }
                    Text(style = card_value_style()) { "Flex" }
                }
                View(style = card_style()) {
                    Text(style = card_label_style()) { "Method" }
                    Text(style = card_value_style()) { "→ BRS" }
                }
                View(style = card_style()) {
                    Text(style = card_label_style()) { "Reactive" }
                    Text(style = card_value_style()) { "Soon" }
                }
            }

            View(style = footer_style()) {
                Text(style = footer_text_style()) { "v0 — flex layout active" }
            }
        }
    }
}

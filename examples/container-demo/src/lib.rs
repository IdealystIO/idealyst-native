//! `container-demo` — a one-screen visual proof of **container queries**.
//!
//! Both columns render the *same* `Card` stylesheet. The only difference
//! is the width of the `.container()` view each card sits in:
//!
//! - **Left** — a 300 dp container. Below the card's 400 dp threshold, so
//!   the card stays in its mobile-first base layout: the two swatches
//!   stack in a **Column**.
//! - **Right** — a 600 dp container. At/above 400 dp, the `container
//!   (min_width: 400px)` overlay flips the card to a **Row**: swatches
//!   side by side.
//!
//! One stylesheet, two layouts, decided by the box each card is in — not
//! by the window. On native this rides the resolved-inline-size signal
//! the backend feeds from its layout pass; on web it's a `@container`
//! rule. Run it:
//!
//! ```text
//! idealyst dev --macos --local
//! ```

use idea_ui::{install_idea_theme, light_theme};
use runtime_core::{
    stylesheet, ui, AlignItems, Color, Element, FlexDirection, JustifyContent, Length,
};

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

// The whole screen: a heading and a row of two differently-sized
// containers.
stylesheet! {
    Page<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 24.0,
            padding: 32.0,
            background: Color("#0f172a".into()),
            align_items: AlignItems::FlexStart,
        }
    }
}

stylesheet! {
    Heading<()> {
        base(_t) {
            color: Color("#e2e8f0".into()),
            font_size: 20.0,
        }
    }
}

stylesheet! {
    Caption<()> {
        base(_t) {
            color: Color("#94a3b8".into()),
            font_size: 13.0,
        }
    }
}

stylesheet! {
    Row<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: 24.0,
            align_items: AlignItems::FlexStart,
        }
    }
}

// The two containers. Their fixed width is what each card queries.
stylesheet! {
    NarrowContainer<()> {
        base(_t) {
            width: Length::Px(300.0),
            flex_direction: FlexDirection::Column,
            gap: 8.0,
        }
    }
}

stylesheet! {
    WideContainer<()> {
        base(_t) {
            width: Length::Px(600.0),
            flex_direction: FlexDirection::Column,
            gap: 8.0,
        }
    }
}

// The card — IDENTICAL stylesheet for both columns. Base is the
// mobile-first stacked layout; the container overlay switches to a row
// once the card's container is at least 400 dp wide.
stylesheet! {
    Card<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            gap: 12.0,
            padding: 16.0,
            background: Color("#1e293b".into()),
            border_radius: 12.0,
        }
        container (min_width: 400px)(_t) {
            flex_direction: FlexDirection::Row,
        }
    }
}

stylesheet! {
    SwatchA<()> {
        base(_t) {
            background: Color("#6366f1".into()),
            padding: 24.0,
            border_radius: 8.0,
            min_width: Length::Px(96.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    SwatchB<()> {
        base(_t) {
            background: Color("#ec4899".into()),
            padding: 24.0,
            border_radius: 8.0,
            min_width: Length::Px(96.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
    }
}

stylesheet! {
    SwatchLabel<()> {
        base(_t) {
            color: Color("#ffffff".into()),
            font_size: 16.0,
        }
    }
}

/// The shared card — IDENTICAL stylesheet for both columns. Its layout is
/// decided solely by the width of the container it ends up in. The swatch
/// `test_id`s let a driver read their frames over the robot bridge to
/// confirm the layout switch (stacked vs. side-by-side).
fn card(a_id: &'static str, b_id: &'static str) -> Element {
    ui! {
        view(style = Card()) {
            view(style = SwatchA(), test_id = a_id) { text(style = SwatchLabel()) { "A" } }
            view(style = SwatchB(), test_id = b_id) { text(style = SwatchLabel()) { "B" } }
        }
    }
}

/// One labelled container holding the shared card. The two branches
/// differ only in the container's width + caption (each `stylesheet!`
/// mints a distinct type, so they can't share an `if`/`else` binding).
fn container_column(is_wide: bool) -> Element {
    if is_wide {
        ui! {
            view(style = WideContainer()) {
                text(style = Caption()) { "600 dp container → Row" }
                { card("sw-a-wide", "sw-b-wide") }
            }.container()
        }
    } else {
        ui! {
            view(style = NarrowContainer()) {
                text(style = Caption()) { "300 dp container → Column" }
                { card("sw-a-narrow", "sw-b-narrow") }
            }.container()
        }
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());
    ui! {
        view(style = Page()) {
            text(style = Heading()) { "Container queries — same Card, different container width" }
            view(style = Row()) {
                { container_column(false) }
                { container_column(true) }
            }
        }
    }
}

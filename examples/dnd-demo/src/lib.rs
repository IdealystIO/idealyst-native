//! `dnd-demo` — a one-screen proof of the [`dnd`] cross-platform drag-and-drop
//! SDK.
//!
//! Three color chips sit in a palette. Drag any chip into either bin:
//!
//! - While a chip hovers a bin, the bin **highlights** — driven by the
//!   reactive [`Droppable::is_over`] signal.
//! - Drop it and the bin records the chip (`on_drop` delivers the payload),
//!   shown in the bin's status line.
//! - Miss every bin and the chip **springs back** to the palette
//!   (`Draggable` snap-back).
//!
//! One author tree, identical behavior on every backend — the chips follow the
//! finger through the same `AnimatedValue → Translate` path every backend
//! implements, and the bins are hit-tested in window space. Activation is
//! [`Activation::platform_default`]: immediate drag on desktop/web, long-press
//! to pick up on touch.
//!
//! ```text
//! idealyst dev --macos --local
//! idealyst dev --web
//! ```

use dnd::{Activation, DragContext, Draggable, Droppable};
use idea_ui::{install_idea_theme, light_theme};
use runtime_core::animation::{AnimProp, AnimatedValue};
use runtime_core::{
    signal, text, view, AlignItems, Color, Element, FlexDirection, JustifyContent, Length, Ref,
    Signal, StyleRules, StyleSheet, Tokenized, ViewHandle,
};
use std::rc::Rc;

/// Bin fill colors, sRGB `(r,g,b,a)` in `0..1` (the shape `bind_color` wants).
/// `#1e293b` at rest, `#334155` while a chip hovers.
const BIN_BG: (f32, f32, f32, f32) = (0.1176, 0.1608, 0.2314, 1.0);
const BIN_BG_OVER: (f32, f32, f32, f32) = (0.2, 0.2549, 0.3333, 1.0);

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

/// The payload that travels from a chip to a bin. `Copy`, so the drag source's
/// payload closure can hand out fresh copies cheaply.
#[derive(Clone, Copy)]
struct ChipData {
    label: &'static str,
    color: &'static str,
}

const CHIPS: [ChipData; 3] = [
    ChipData {
        label: "Coral",
        color: "#fb7185",
    },
    ChipData {
        label: "Mint",
        color: "#34d399",
    },
    ChipData {
        label: "Sky",
        color: "#38bdf8",
    },
];

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // One context for the whole screen; payload is the chip.
    let ctx: DragContext<ChipData> = DragContext::new();

    // Each bin remembers the last chip dropped on it.
    let bin_a: Signal<Option<ChipData>> = signal!(None);
    let bin_b: Signal<Option<ChipData>> = signal!(None);

    let palette = view(CHIPS.iter().map(|c| chip(&ctx, *c)).collect::<Vec<_>>())
        .with_style(palette_sheet())
        .into();

    let bins = view(vec![
        bin(&ctx, "Bin A", bin_a),
        bin(&ctx, "Bin B", bin_b),
    ])
    .with_style(bins_row_sheet())
    .into();

    // Bins first, palette last: the dragged chip is then a *later* sibling
    // than the bins, so it paints on top of them as it moves over a bin. This
    // is the cross-backend way to elevate the dragged element — paint order is
    // sibling order on AppKit/UIKit/Android, and DOM order on web (there is no
    // `z-index` style prop to lean on). So the chips sit below and drag up.
    view(vec![
        text("Drag & Drop").with_style(title_sheet()).into(),
        text("Drag a chip up into a bin. Miss, and it springs back.")
            .with_style(caption_sheet())
            .into(),
        bins,
        palette,
    ])
    .with_style(page_sheet())
    .into()
}

/// A draggable color chip. Follows the finger via its bound offset and carries
/// its [`ChipData`] as the payload. Defaults to snap-back, so the palette chip
/// returns home after a drop (the drop target keeps its own record).
fn chip(ctx: &DragContext<ChipData>, data: ChipData) -> Element {
    let chip_ref: Ref<ViewHandle> = Ref::new();
    let drag = Draggable::new(ctx, move || data).activation(Activation::platform_default());
    drag.bind(chip_ref);
    let handler = drag.handler();

    view(vec![text(data.label).with_style(chip_label_sheet()).into()])
        .with_style(chip_sheet(data.color))
        .on_touch(move |ev| handler(ev))
        .bind(chip_ref)
        .into()
}

/// A drop bin. Highlights while a chip hovers it (reactive `is_over`) and
/// records the dropped chip in `slot`.
fn bin(ctx: &DragContext<ChipData>, label: &'static str, slot: Signal<Option<ChipData>>) -> Element {
    let bin_ref: Ref<ViewHandle> = Ref::new();

    // Drive the highlight through the animation pipeline (the same path the
    // chip's translate uses, so it's reliable on every backend) rather than a
    // reactive stylesheet swap. on_enter/on_leave nudge the bound background
    // color; bind_color writes it via set_animated_color each frame.
    let bg = AnimatedValue::new(BIN_BG);
    bg.bind_color(bin_ref, AnimProp::BackgroundColor);
    let bg_enter = bg.clone();
    let bg_leave = bg.clone();

    let drop = Droppable::new(ctx)
        .on_enter(move |_| bg_enter.set(BIN_BG_OVER))
        .on_leave(move || bg_leave.set(BIN_BG))
        .on_drop(move |c| slot.set(Some(c)));
    drop.bind(bin_ref);

    let status = text(move || match slot.get() {
        Some(c) => format!("Last drop: {}", c.label),
        None => "drop here".to_string(),
    })
    .with_style(bin_status_sheet())
    .into();

    view(vec![
        text(label).with_style(bin_label_sheet()).into(),
        status,
    ])
    .with_style(bin_sheet())
    .bind(bin_ref)
    .into()
}

// ---------------------------------------------------------------------------
// Styles (imperative `StyleRules` — same helper shape animation-test uses)
// ---------------------------------------------------------------------------

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn col(hex: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(hex.to_string()))
}

fn radius(rules: &mut StyleRules, r: f32) {
    rules.border_top_left_radius = Some(px(r));
    rules.border_top_right_radius = Some(px(r));
    rules.border_bottom_left_radius = Some(px(r));
    rules.border_bottom_right_radius = Some(px(r));
}

fn pad(rules: &mut StyleRules, v: f32) {
    rules.padding_top = Some(px(v));
    rules.padding_right = Some(px(v));
    rules.padding_bottom = Some(px(v));
    rules.padding_left = Some(px(v));
}

fn border(rules: &mut StyleRules, w: f32, color: &str) {
    rules.border_top_width = Some(Tokenized::Literal(w));
    rules.border_right_width = Some(Tokenized::Literal(w));
    rules.border_bottom_width = Some(Tokenized::Literal(w));
    rules.border_left_width = Some(Tokenized::Literal(w));
    rules.border_top_color = Some(col(color));
    rules.border_right_color = Some(col(color));
    rules.border_bottom_color = Some(col(color));
    rules.border_left_color = Some(col(color));
}

fn static_sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

fn page_sheet() -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(px(24.0)),
        background: Some(col("#0f172a")),
        align_items: Some(AlignItems::FlexStart),
        ..Default::default()
    };
    pad(&mut rules, 32.0);
    static_sheet(rules)
}

fn title_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#e2e8f0")),
        font_size: Some(px(24.0)),
        ..Default::default()
    })
}

fn caption_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#94a3b8")),
        font_size: Some(px(14.0)),
        ..Default::default()
    })
}

fn palette_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        gap: Some(px(16.0)),
        ..Default::default()
    })
}

fn chip_sheet(color: &'static str) -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col(color)),
        width: Some(px(96.0)),
        height: Some(px(72.0)),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    };
    radius(&mut rules, 12.0);
    static_sheet(rules)
}

fn chip_label_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#0f172a")),
        font_size: Some(px(15.0)),
        ..Default::default()
    })
}

fn bins_row_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        gap: Some(px(24.0)),
        ..Default::default()
    })
}

/// The bin frame. Background is **not** set here — it is owned by the
/// animated `BackgroundColor` (see `bin`), so the two don't fight over it.
fn bin_sheet() -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        width: Some(px(200.0)),
        height: Some(px(140.0)),
        gap: Some(px(8.0)),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    };
    pad(&mut rules, 16.0);
    border(&mut rules, 2.0, "#334155");
    radius(&mut rules, 14.0);
    static_sheet(rules)
}

fn bin_label_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#e2e8f0")),
        font_size: Some(px(16.0)),
        ..Default::default()
    })
}

fn bin_status_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#94a3b8")),
        font_size: Some(px(13.0)),
        ..Default::default()
    })
}

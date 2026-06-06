//! `modal-demo` — the smallest app that exercises the idea-ui `Modal`.
//!
//! Two buttons:
//!   * **Short modal** — a couple of lines plus a button. The card sizes to
//!     its content (it hugs), centered over a dimming backdrop.
//!   * **Tall modal** — 30 paragraphs, taller than the viewport. The card
//!     caps at the viewport height and its body scrolls to the Close button.
//!
//! Both dismiss by tapping the backdrop (or the in-card Close button). This
//! is the manual repro for the two Modal fixes: the backdrop-tap crash
//! (`remove_child` on the portal's orphan Taffy root) and the
//! "content doesn't render / card collapses to 0×0" scroll-layout bug.
//!
//! Run it: `idealyst run ios examples/modal-demo` (or `--web` / `android`),
//! or `idealyst dev ios examples/modal-demo` for hot reload.

use std::rc::Rc;

use idea_ui::{
    install_idea_theme, light_theme, tone, typography_kind, variant, Button, Modal, Typography,
};
use runtime_core::{
    signal, ui, AlignItems, Element, FlexDirection, JustifyContent, Length, StyleRules, StyleSheet,
    Tokenized, VariantSet,
};

// SDK-registration hook the CLI-generated wrappers call before mount. No
// third-party SDKs here, so it's an empty generic over `Backend`.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

// Recorder-side registration for the runtime-server sidecar. Gated by
// `sidecar` so device/web builds never pull `dev-server`.
#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

/// Viewport-filling root: a centered column with comfortable spacing.
fn root_style() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        gap: Some(Tokenized::Literal(Length::Px(16.0))),
        padding_top: Some(Tokenized::Literal(Length::Px(24.0))),
        padding_right: Some(Tokenized::Literal(Length::Px(24.0))),
        padding_bottom: Some(Tokenized::Literal(Length::Px(24.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(24.0))),
        width: Some(Tokenized::Literal(Length::pct(100.0))),
        height: Some(Tokenized::Literal(Length::pct(100.0))),
        ..Default::default()
    }))
}

pub fn app() -> Element {
    install_idea_theme(light_theme());

    // Each modal is host-owned open state — `if open.get() { Modal { … } }`.
    // Signals are `Copy`, so the same handle is captured by every closure.
    let short_open = signal!(false);
    let tall_open = signal!(false);

    // Handlers are bound as `Rc<dyn Fn()>` (not inline `Rc::new(closure)`):
    // the `ui!` macro feeds field values through `.into()`, and a concrete
    // `Rc<{closure}>` doesn't coerce to `Rc<dyn Fn()>` that way. The close
    // handlers are reused by both the Modal's `on_dismiss` and the in-card
    // Close button, so they're cloned at the call sites.
    let open_short: Rc<dyn Fn()> = Rc::new(move || short_open.set(true));
    let close_short: Rc<dyn Fn()> = Rc::new(move || short_open.set(false));
    let open_tall: Rc<dyn Fn()> = Rc::new(move || tall_open.set(true));
    let close_tall: Rc<dyn Fn()> = Rc::new(move || tall_open.set(false));

    ui! {
        view(style = root_style()) {
            Typography(content = "Modal demo".to_string(), kind = typography_kind::H1)
            Typography(
                content = "A short modal hugs its content; a tall one caps at the \
                    viewport and scrolls. Tap the backdrop to dismiss."
                    .to_string(),
                kind = typography_kind::Body,
                muted = true,
            )

            Button(
                label = "Open short modal".to_string(),
                on_click = open_short,
                tone = tone::Primary,
                variant = variant::Filled,
            )
            Button(
                label = "Open tall modal".to_string(),
                on_click = open_tall,
                tone = tone::Neutral,
                variant = variant::Soft,
            )

            // --- Short modal: content-sized card ---
            if short_open.get() {
                Modal(on_dismiss = Some(close_short.clone())) {
                    Typography(content = "Short modal".to_string(), kind = typography_kind::H2)
                    Typography(
                        content = "This card sizes to its content — no wasted space. \
                            It hugs these two lines plus the button below."
                            .to_string(),
                    )
                    Button(
                        label = "Close".to_string(),
                        on_click = close_short.clone(),
                        tone = tone::Primary,
                        variant = variant::Filled,
                    )
                }
            }

            // --- Tall modal: capped at the viewport, body scrolls ---
            if tall_open.get() {
                Modal(on_dismiss = Some(close_tall.clone())) {
                    Typography(content = "Tall modal".to_string(), kind = typography_kind::H2)
                    Typography(
                        content = "This body is taller than the screen, so the card \
                            caps at the viewport height and the content scrolls."
                            .to_string(),
                    )
                    for i in 0..30 {
                        Typography(
                            content = format!(
                                "Paragraph {} — keep scrolling to reach the Close button.",
                                i + 1
                            ),
                        )
                    }
                    Button(
                        label = "Close".to_string(),
                        on_click = close_tall.clone(),
                        tone = tone::Primary,
                        variant = variant::Filled,
                    )
                }
            }
        }
    }
}

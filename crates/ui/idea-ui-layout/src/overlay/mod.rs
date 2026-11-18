//! Overlays that change KIND with the form factor (rule `overlays`).
//!
//! With a pointer, a control's secondary surface is anchored: a menu or
//! a popover beside the thing that opened it. A thumb cannot reach a
//! panel pinned to a small trigger, and a phone has no hover for a
//! flyout, so on a phone the same content arrives as a SHEET rising from
//! the bottom edge. A form dialog makes the same move: a card with room
//! around it on a tablet or desktop, a sheet on a phone, where a centred
//! card would sit under the keyboard.
//!
//! | With a pointer | On a phone | Component |
//! |---|---|---|
//! | anchored menu | action sheet | [`ActionMenu`], [`ActionMenuSurface`] |
//! | anchored popover | bottom sheet | [`AdaptivePopover`] |
//! | menu with submenus | sheet whose rows push panes | [`DrillSheet`] |
//! | top-anchored dialog card | bottom sheet | [`Dialog`] |
//!
//! Same content and handlers either way; only the surface changes, and
//! [`sheet_serves`] is the one switch that says which.

use std::rc::Rc;

use idea_ui::{Modal, ModalPresentation, ModalSlotStyles};
use runtime_core::{component, rx, ui, Element, IdealystSchema, Length, Reactive, StyleRules, Tokenized};

use crate::width::is_phone;
use crate::Slot;

mod action_menu;
mod adaptive_popover;
mod dialog;
mod dialog_footer;
mod drill_sheet;
mod rows;

pub use action_menu::{ActionMenu, ActionMenuProps, ActionMenuSurface, ActionMenuSurfaceProps};
pub use adaptive_popover::{AdaptivePopover, AdaptivePopoverProps};
pub use dialog::{Dialog, DialogProps, DIALOG_WIDTH};
pub use dialog_footer::{DialogFooter, DialogFooterProps};
pub use drill_sheet::{DrillCx, DrillSheet, DrillSheetProps};
pub use rows::{
    SheetActionRow, SheetActionRowProps, SheetCheckRow, SheetCheckRowProps, SheetHeading, SheetHeadingProps,
    SheetPushRow, SheetPushRowProps, SHEET_ROW_MIN_HEIGHT,
};

/// Whether a control's secondary surface is the sheet rather than an
/// anchored panel. TRACKED: read in a `ui!` `match`, a rotation through
/// 640 re-picks the surface.
///
/// Width decides it because every phone is `Xs`. A tablet in portrait
/// keeps the anchored panel: it has the room, and its triggers are not
/// under a thumb at the bottom edge.
pub fn sheet_serves() -> bool {
    is_phone()
}

/// What a form dialog's `Modal` passes to `presentation`: a bottom sheet
/// on a phone, a centred card everywhere else. [`Dialog`] applies the
/// same switch itself; this is for a `Modal` a screen mounts directly.
///
/// `Modal` reads it per open, so a rotation while open keeps the shape
/// it opened with and the next open takes the new one.
///
/// ```ignore
/// Modal(open = open, presentation = dialog_presentation(), on_dismiss = Some(dismiss), content = body)
/// ```
pub fn dialog_presentation() -> Reactive<ModalPresentation> {
    rx!(if sheet_serves() { ModalPresentation::Sheet } else { ModalPresentation::Centered })
}

/// What the `Button`s of a [`DialogFooter`] pass to `block`: full width
/// in a sheet, hugging their label in a card.
///
/// A per-button prop because the footer cannot impose it: a button that
/// is not `block` sets `align_self: Center`, which beats the footer's
/// `align_items: Stretch`, so a column footer alone would leave two
/// centred, label-wide buttons in a full-width sheet.
///
/// ```ignore
/// Button(label = "Save", block = dialog_block(), on_click = save.clone())
/// ```
pub fn dialog_block() -> Reactive<bool> {
    rx!(sheet_serves())
}

#[runtime_core::props]
#[derive(IdealystSchema)]
pub struct SheetProps {
    /// Whether the sheet is up. Pass the live open state, never
    /// `rx!(true)` inside an `if`: the sheet stays mounted and plays its
    /// exit when this turns false, and unmounting it instead cuts the
    /// animation.
    pub open: bool,
    /// Close the sheet: the backdrop press, Escape and back.
    #[prop(static)]
    pub on_dismiss: Rc<dyn Fn()>,
    /// Builds the sheet's body, on every open.
    #[prop(static)]
    pub content: Slot,
}

impl Default for SheetProps {
    fn default() -> Self {
        Self {
            open: Reactive::Static(false),
            on_dismiss: Rc::new(|| {}),
            content: Slot::default(),
        }
    }
}

/// A bottom sheet driven by a live `open`: the phone form of a menu or a
/// popover. Edge to edge at whatever width it opens at, so a sheet on a
/// window just wider than the modal's cap does not float in the middle
/// of the bottom edge.
///
/// Its rows are [`SheetActionRow`] (does something and closes),
/// [`SheetCheckRow`] (toggles and stays) and [`SheetPushRow`] (pushes a
/// pane, in a [`DrillSheet`]).
///
/// ```ignore
/// Sheet(
///     open = rx!(open.get()),
///     on_dismiss = on_dismiss.clone(),
///     content = move || ui! { Stack(gap = StackGap::Xs) { rows() } },
/// )
/// ```
#[component]
pub fn Sheet(props: &SheetProps) -> Element {
    let open = props.open.clone();
    let on_dismiss = props.on_dismiss.clone();
    let content = props.content.clone();
    ui! {
        Modal(
            open = open,
            presentation = ModalPresentation::Sheet,
            on_dismiss = Some(on_dismiss.clone()),
            styles = ModalSlotStyles::default().surface(StyleRules {
                max_width: Some(Tokenized::Literal(Length::pct(100.0))),
                ..Default::default()
            }),
            content = move || content.build(),
        )
    }
}

#[cfg(test)]
mod tests {
    use runtime_core::{viewport_size, ViewportSize};

    use super::*;

    /// Put the world at a width and let the breakpoint memo see it.
    fn at_width(px: f32) {
        viewport_size().set(ViewportSize::new(px, 800.0));
        idea_theme::testing::commit();
    }

    #[test]
    fn a_form_dialog_is_a_sheet_on_a_phone_and_a_card_everywhere_else() {
        idea_theme::testing::with_test_world(|| {
            let presentation = dialog_presentation();

            at_width(390.0);
            assert_eq!(presentation.get(), ModalPresentation::Sheet);

            // 640 is the `sm` floor: the first width that is not a phone.
            at_width(640.0);
            assert_eq!(presentation.get(), ModalPresentation::Centered);

            at_width(1280.0);
            assert_eq!(presentation.get(), ModalPresentation::Centered);

            // And back: the value is live, not a mount-time snapshot.
            at_width(430.0);
            assert_eq!(presentation.get(), ModalPresentation::Sheet);
        });
    }

    /// The sheet form and the full-width buttons are one decision seen
    /// twice: a sheet with two centred, label-wide buttons is what this
    /// guards against.
    #[test]
    fn the_sheet_form_and_the_block_buttons_agree_at_every_width() {
        idea_theme::testing::with_test_world(|| {
            let presentation = dialog_presentation();
            let block = dialog_block();
            for px in [320.0, 390.0, 440.0, 639.0, 640.0, 768.0, 1440.0] {
                at_width(px);
                assert_eq!(
                    presentation.get() == ModalPresentation::Sheet,
                    block.get(),
                    "presentation and block disagree at {px}",
                );
            }
        });
    }

    #[test]
    fn a_tablet_keeps_the_anchored_surface() {
        idea_theme::testing::with_test_world(|| {
            at_width(390.0);
            assert!(sheet_serves());
            at_width(820.0);
            assert!(!sheet_serves());
        });
    }
}

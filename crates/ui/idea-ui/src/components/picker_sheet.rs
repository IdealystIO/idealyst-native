//! A picker's surface on a phone: what an anchored popover holds beside
//! its trigger elsewhere, in a bottom sheet. A thumb cannot reliably reach
//! a panel pinned to a small trigger, and a popover near the screen's edge
//! is clipped by it, leaving nothing pressable. `Select`, `Autocomplete`,
//! the date inputs and pickers, and `Popover` open this below `Sm` unless
//! told not to (`sheet_on_phone = false`).
//!
//! The sheet exists only while its picker is open: the picker builds it
//! inside its own `if`, so a closed picker mounts nothing for it. It slides
//! in as it mounts, and when the `if` switches it off it asks that `if` to
//! keep it mounted for its slide out (`runtime_core::hold_on_exit`).

use std::rc::Rc;

use runtime_core::{hold_on_exit, signal, ui, Element, PointerEvents, ReadSignal};

use crate::breakpoint::{current_breakpoint, Breakpoint};
use crate::components::icon_button::{IconButton, IconButtonSize};
use crate::components::modal::{Modal, ModalPresentation, ModalSection, SHEET_EXIT_MS};
use idea_theme::extensible::tone;

/// Decides, at each open, whether a picker opens as a sheet. Built in the
/// picker's body: the breakpoint signal is fetched there, inside the world,
/// and only peeked from the press handler that opens the picker, which may
/// run outside it.
#[derive(Clone, Copy)]
pub(crate) struct SheetChoice {
    allowed: bool,
    breakpoint: ReadSignal<Breakpoint>,
}

impl SheetChoice {
    /// `sheet_on_phone`: the picker's own opt-out.
    pub(crate) fn new(sheet_on_phone: bool) -> Self {
        Self { allowed: sheet_on_phone, breakpoint: current_breakpoint() }
    }

    /// This open is a sheet: allowed, and the window is below `Sm`. Read
    /// when the picker OPENS, so a sheet stays a sheet until it closes.
    pub(crate) fn now(&self) -> bool {
        self.allowed && !self.breakpoint.peek().is_at_least(Breakpoint::Sm)
    }
}

/// A bottom sheet holding `content`, open as it mounts. Build it inside the
/// `if` that shows the picker: switching that `if` off slides the sheet out
/// before it is removed. A press on the scrim, Escape or back closes the
/// sheet and runs `on_dismiss`, which should close the picker; a sheet whose
/// owner cannot be told (a `Popover` without `on_dismiss`) still closes.
/// While it slides out its content takes no presses, so a second tap cannot
/// pick again.
pub(crate) fn picker_sheet(
    on_dismiss: Option<Rc<dyn Fn()>>,
    content: impl Fn() -> Vec<Element> + 'static,
) -> Element {
    picker_sheet_with_head(on_dismiss, None, content)
}

/// Builds a sheet's pinned head, handed the sheet's own close.
pub(crate) type SheetHead = Rc<dyn Fn(Rc<dyn Fn()>) -> Element>;

/// [`picker_sheet`] with a `head` pinned above the content: it stays put
/// while the content scrolls under it.
pub(crate) fn picker_sheet_with_head(
    on_dismiss: Option<Rc<dyn Fn()>>,
    head: Option<SheetHead>,
    content: impl Fn() -> Vec<Element> + 'static,
) -> Element {
    let shown = signal(true);
    hold_on_exit(SHEET_EXIT_MS as u32, move || shown.set(false));
    let dismiss: Rc<dyn Fn()> = Rc::new(move || {
        shown.set(false);
        if let Some(cb) = &on_dismiss {
            cb();
        }
    });
    let content = Rc::new(content);
    let body = move || {
        let kids = content();
        let style = move || {
            PickerSheetBody()
                .leaving(if shown.get() { PickerSheetBodyLeaving::No } else { PickerSheetBodyLeaving::Yes })
                .into_style_application()
        };
        ui! { view(style = style) { kids } }
    };
    let header: ModalSection = match head {
        Some(head) => {
            let close = dismiss.clone();
            ModalSection(Some(Rc::new(move || head(close.clone()))))
        }
        None => ModalSection(None),
    };
    ui! {
        Modal(
            open = shown,
            presentation = ModalPresentation::Sheet,
            on_dismiss = Some(dismiss),
            header = header,
            content = body,
        )
    }
}

/// A sheet's search head: the field across the width and a ✕ that closes
/// the sheet.
pub(crate) fn search_head(field: Element, close: Rc<dyn Fn()>) -> Element {
    ui! {
        view(style = PickerSheetSearch()) {
            view(style = PickerSheetSearchField()) { field }
            IconButton(
                icon = Some(super::date_input::CLEAR_GLYPH),
                label = Some("Close".to_string()),
                on_click = close,
                tone = tone::Neutral,
                variant = crate::variant::Ghost,
                size = IconButtonSize::Sm,
            )
        }
    }
}

runtime_core::stylesheet! {
    // The sheet's content, each part the sheet's width. Inert while the
    // sheet slides out.
    PickerSheetBody<crate::theme::IdeaThemeRef> {
        base(t) {
            flex_direction: runtime_core::FlexDirection::Column,
            align_items: runtime_core::AlignItems::Stretch,
            gap: t.spacing.sm(),
        }
        variant leaving {
            #[default]
            no(_t) {}
            yes(_t) {
                pointer_events: PointerEvents::None,
            }
        }
    }
}

runtime_core::stylesheet! {
    // The search field and its ✕ on one line.
    PickerSheetSearch<crate::theme::IdeaThemeRef> {
        base(t) {
            flex_direction: runtime_core::FlexDirection::Row,
            align_items: runtime_core::AlignItems::Center,
            gap: t.spacing.sm(),
        }
    }
}

runtime_core::stylesheet! {
    PickerSheetSearchField<crate::theme::IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: runtime_core::Length::Px(0.0),
        }
    }
}

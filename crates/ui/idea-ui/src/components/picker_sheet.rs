//! A picker's surface on a phone: what an anchored popover holds beside
//! its trigger elsewhere, in a bottom sheet. A thumb cannot reliably reach
//! a panel pinned to a small trigger, and a popover near the screen's edge
//! is clipped by it, leaving nothing pressable. `Select`, `Autocomplete`
//! and `DatePicker` open this below `Sm` unless told not to
//! (`sheet_on_phone = false`).

use std::rc::Rc;

use runtime_core::{ui, Element, ReadSignal, Signal};

use crate::breakpoint::{current_breakpoint, Breakpoint};
use crate::components::modal::{Modal, ModalPresentation};

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

    /// The picker may open as a sheet at all.
    pub(crate) fn allowed(&self) -> bool {
        self.allowed
    }

    /// This open is a sheet: allowed, and the window is below `Sm`. Read
    /// when the picker OPENS, so a sheet stays a sheet until it closes.
    pub(crate) fn now(&self) -> bool {
        self.allowed && !self.breakpoint.peek().is_at_least(Breakpoint::Sm)
    }
}

/// A bottom sheet holding `content`, open while `open` is. A press on the
/// backdrop, Escape or back closes it and runs `on_dismiss`. Mount it once,
/// ungated: the sheet plays its own entry and exit.
pub(crate) fn picker_sheet(
    open: Signal<bool>,
    on_dismiss: Rc<dyn Fn()>,
    content: impl Fn() -> Element + 'static,
) -> Element {
    ui! {
        Modal(
            open = open,
            presentation = ModalPresentation::Sheet,
            on_dismiss = Some(on_dismiss),
            content = content,
        )
    }
}

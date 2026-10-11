//! App-level pointer input for the terminal backend
//! (`runtime_shared::pointer_input`): the host-side mouse event type and
//! its translation to [`AppPointerEvent`].
//!
//! # Units
//!
//! Terminals report the mouse by cell (`column`, `row`). Positions are
//! converted to this backend's **layout px** — the unit Taffy and author
//! stylesheets use, `cell × cell_size` (1 px per cell by default, see
//! `TerminalBackend::set_cell_size`) — at the cell's top-left corner, so
//! a pointer position lines up with the frames views are laid out at.
//! `dx`/`dy` are the difference from the previous mouse position in the
//! same unit (whole cells, scaled). (The reactive viewport signal stays
//! in raw cells; with the default 1:1 cell size the two coincide.)
//!
//! A wheel notch scrolls [`WHEEL_NOTCH_CELLS`] rows (or columns), the
//! same step the host's own scroll-view dispatch uses, reported as
//! `WHEEL_NOTCH_CELLS × cell height` layout px (positive `wheel_dy` =
//! wheel toward the user, the web `deltaY` sign).
//!
//! # What a terminal can't do
//!
//! - **Pointer lock** — terminals have no way to capture or hide the
//!   mouse cursor, and only report positions by cell. `set_pointer_lock`
//!   is a no-op: the app never sees `pointer_locked() == true`.
//! - **Cursor hiding** — the mouse cursor belongs to the terminal
//!   emulator (the host already hides the TEXT cursor). No-op.
//! - **Context menu** — under mouse capture the emulator does not open
//!   its own right-click menu, so a secondary press has no default to
//!   suppress.

use runtime_shared::pointer_input::{AppPointerEvent, MouseButton, PointerPhase};

/// Rows (vertical) / columns (horizontal) one wheel notch scrolls.
pub const WHEEL_NOTCH_CELLS: f32 = 3.0;

/// What a [`TerminalPointer`] reports.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TerminalPointerKind {
    /// A button went down.
    Down(MouseButton),
    /// A button came up.
    Up(MouseButton),
    /// The mouse moved (with or without a button held — a drag).
    Move,
    /// Wheel notches: `(dx, dy)`, positive `dy` = wheel toward the user
    /// (crossterm `ScrollDown`), positive `dx` = `ScrollRight`.
    Wheel(f32, f32),
}

/// Host-side mouse event, in terminal cells. Defined here so the backend
/// doesn't depend on `crossterm`; the host converts. Build with
/// [`TerminalPointer::new`].
#[derive(Copy, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TerminalPointer {
    pub kind: TerminalPointerKind,
    pub column: u16,
    pub row: u16,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl TerminalPointer {
    /// `kind` at `(column, row)` with no modifiers.
    pub fn new(kind: TerminalPointerKind, column: u16, row: u16) -> Self {
        TerminalPointer { kind, column, row, shift: false, ctrl: false, alt: false, meta: false }
    }

    /// The same event with these modifier flags.
    pub fn with_modifiers(self, shift: bool, ctrl: bool, alt: bool, meta: bool) -> Self {
        TerminalPointer { shift, ctrl, alt, meta, ..self }
    }
}

/// Translate to the framework event. `cell_size` is the backend's
/// px-per-cell; `last` the previous position in layout px (for
/// `Move` `dx`/`dy`, `None` = first event → 0).
pub(crate) fn app_pointer_event(
    p: &TerminalPointer,
    cell_size: (f32, f32),
    last: Option<(f32, f32)>,
) -> AppPointerEvent {
    let (x, y) = (p.column as f32 * cell_size.0, p.row as f32 * cell_size.1);
    let (phase, button) = match p.kind {
        TerminalPointerKind::Down(b) => (PointerPhase::Down, Some(b)),
        TerminalPointerKind::Up(b) => (PointerPhase::Up, Some(b)),
        TerminalPointerKind::Move => (PointerPhase::Move, None),
        TerminalPointerKind::Wheel(..) => (PointerPhase::Wheel, None),
    };
    let mut ev = AppPointerEvent {
        button,
        shift: p.shift,
        ctrl: p.ctrl,
        alt: p.alt,
        meta: p.meta,
        ..AppPointerEvent::new(phase, x, y)
    };
    match p.kind {
        TerminalPointerKind::Move => {
            if let Some((lx, ly)) = last {
                ev.dx = x - lx;
                ev.dy = y - ly;
            }
        }
        TerminalPointerKind::Wheel(nx, ny) => {
            ev.wheel_dx = nx * WHEEL_NOTCH_CELLS * cell_size.0;
            ev.wheel_dy = ny * WHEEL_NOTCH_CELLS * cell_size.1;
        }
        _ => {}
    }
    ev
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_convert_to_layout_px() {
        let p = TerminalPointer::new(TerminalPointerKind::Down(MouseButton::Primary), 4, 2);
        let e = app_pointer_event(&p, (8.0, 16.0), None);
        assert_eq!((e.phase, e.button, e.x, e.y), (PointerPhase::Down, Some(MouseButton::Primary), 32.0, 32.0));
        // Default 1:1 cell size: px == cells.
        let e = app_pointer_event(&p, (1.0, 1.0), None);
        assert_eq!((e.x, e.y), (4.0, 2.0));
    }

    #[test]
    fn move_delta_from_previous_position() {
        let p = TerminalPointer::new(TerminalPointerKind::Move, 5, 1);
        let first = app_pointer_event(&p, (1.0, 1.0), None);
        assert_eq!((first.dx, first.dy), (0.0, 0.0));
        let e = app_pointer_event(&p, (1.0, 1.0), Some((3.0, 4.0)));
        assert_eq!((e.dx, e.dy), (2.0, -3.0));
    }

    #[test]
    fn wheel_notch_is_three_cells_in_web_sign() {
        let down = TerminalPointer::new(TerminalPointerKind::Wheel(0.0, 1.0), 0, 0);
        let e = app_pointer_event(&down, (8.0, 16.0), None);
        assert_eq!((e.phase, e.wheel_dx, e.wheel_dy), (PointerPhase::Wheel, 0.0, 48.0));
        let left = TerminalPointer::new(TerminalPointerKind::Wheel(-1.0, 0.0), 0, 0);
        assert_eq!(app_pointer_event(&left, (1.0, 1.0), None).wheel_dx, -3.0);
    }

    #[test]
    fn modifiers_pass_through() {
        let p = TerminalPointer::new(TerminalPointerKind::Move, 0, 0).with_modifiers(true, false, true, false);
        let e = app_pointer_event(&p, (1.0, 1.0), None);
        assert!(e.shift && !e.ctrl && e.alt && !e.meta);
    }
}

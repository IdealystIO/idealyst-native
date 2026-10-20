//! The author `cursor` style (`StyleRules::cursor`) over the painted scene.
//!
//! Painted views have no HWNDs, so there is no per-view window class cursor
//! to set. Windows asks the window under the pointer which cursor to show
//! with `WM_SETCURSOR` every time the pointer moves over it; the host
//! answers it through [`window_message`], which hit-tests the scene and
//! shows the cursor of the nearest node on the hit chain that sets one
//! (CSS `cursor` inherits, so a label inside a `cursor: pointer` button
//! shows the hand too).
//!
//! Only messages for the host window itself are answered: over a child
//! control (an edit, a native button) the control keeps its own cursor —
//! `DefWindowProc` of a child forwards `WM_SETCURSOR` to its parent first,
//! so answering there would put the hand over a text field's I-beam.
//! A hidden or locked pointer (`app_pointer`) is handled before this.
//!
//! [`Cursor::Auto`] and an unset cursor show the class cursor (the arrow).
//! A `link` shows the hand unless its style says otherwise, as a browser
//! shows it over `<a>`.
//! Values Windows has no system cursor for fall back to the nearest one:
//! `grab` / `grabbing` → the hand (Windows has no open/closed hand),
//! `move` → the four-way arrow.

use std::cell::RefCell;

use runtime_shared::Cursor;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, LoadCursorW, SetCursor, HTCLIENT, IDC_APPSTARTING, IDC_ARROW, IDC_CROSS,
    IDC_HAND, IDC_HELP, IDC_IBEAM, IDC_NO, IDC_SIZEALL, IDC_SIZENS, IDC_SIZEWE, IDC_WAIT,
};

use crate::{scene, WindowsBackend};

/// The system cursor (`IDC_*`) for `cursor`; `None` for [`Cursor::Auto`],
/// which leaves the window's class cursor in place.
pub(crate) fn system_cursor(cursor: Cursor) -> Option<PCWSTR> {
    Some(match cursor {
        Cursor::Auto => return None,
        Cursor::Default => IDC_ARROW,
        Cursor::Pointer | Cursor::Grab | Cursor::Grabbing => IDC_HAND,
        Cursor::Text => IDC_IBEAM,
        Cursor::Wait => IDC_WAIT,
        Cursor::Progress => IDC_APPSTARTING,
        Cursor::Help => IDC_HELP,
        Cursor::NotAllowed => IDC_NO,
        Cursor::Move => IDC_SIZEALL,
        Cursor::Crosshair => IDC_CROSS,
        Cursor::ColResize | Cursor::EwResize => IDC_SIZEWE,
        Cursor::RowResize | Cursor::NsResize => IDC_SIZENS,
    })
}

/// The cursor for the logical point `(x, y)`: the nearest node on the hit
/// chain (deepest first) with a `cursor` style, or a primitive's own
/// default (a link's hand) when its style sets none.
pub(crate) fn cursor_at(b: &WindowsBackend, x: f32, y: f32) -> Option<Cursor> {
    scene::hit_chain(b, x, y).into_iter().find_map(|n| {
        let m = b.nodes.get(&n.id)?;
        m.cursor.or(m.default_cursor)
    })
}

/// Host-`WndProc` hook for `WM_SETCURSOR`. Returns `true` when it set the
/// cursor (the `WndProc` then returns `TRUE` without `DefWindowProc`, which
/// would reset it to the class cursor).
pub fn window_message(cell: &RefCell<WindowsBackend>, host: HWND, wparam: WPARAM, lparam: LPARAM) -> bool {
    let hit = (lparam.0 as usize & 0xFFFF) as u32;
    if hit != HTCLIENT || wparam.0 != host.0 as usize {
        return false;
    }
    let mut p = POINT::default();
    unsafe {
        if GetCursorPos(&mut p).is_err() || !ScreenToClient(host, &mut p).as_bool() {
            return false;
        }
    }
    // Busy backend (a handler re-entering the message loop): keep whatever
    // cursor is showing rather than panic on a double borrow.
    let Ok(b) = cell.try_borrow() else { return false };
    let (x, y) = b.dpi.point_to_logical(p.x as f32, p.y as f32);
    let Some(id) = cursor_at(&b, x, y).and_then(system_cursor) else { return false };
    drop(b);
    match unsafe { LoadCursorW(None, id) } {
        Ok(c) => {
            unsafe { SetCursor(Some(c)) };
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeKind, NodeMeta, ViewVisual};

    fn put(b: &mut WindowsBackend, id: u64, parent: Option<u64>, frame: (f32, f32, f32, f32), cursor: Option<Cursor>) {
        let mut m = NodeMeta::new(NodeKind::View(ViewVisual::default()));
        m.frame = frame;
        m.cursor = cursor;
        b.nodes.insert(id, m);
        match parent {
            Some(p) => b.children.entry(p).or_default().push(id),
            None => b.root_id = Some(id),
        }
    }

    /// `cursor` was never read on Windows: idea-ui's clickables set
    /// `cursor: pointer` and still showed the arrow. The nearest node that
    /// sets a cursor decides, its unstyled children inherit it, and a
    /// nested `auto` resets to the class cursor.
    #[test]
    fn regression_author_cursor_resolves_along_the_hit_chain() {
        let mut b = WindowsBackend::new(HWND(std::ptr::null_mut()));
        put(&mut b, 1, None, (0.0, 0.0, 800.0, 600.0), None);
        // A pointer button with an unstyled label inside it…
        put(&mut b, 2, Some(1), (100.0, 100.0, 200.0, 50.0), Some(Cursor::Pointer));
        put(&mut b, 3, Some(2), (10.0, 10.0, 50.0, 20.0), None);
        // …and a pointer card holding an `auto` region.
        put(&mut b, 4, Some(1), (400.0, 100.0, 200.0, 200.0), Some(Cursor::Pointer));
        put(&mut b, 5, Some(4), (0.0, 0.0, 100.0, 100.0), Some(Cursor::Auto));

        assert_eq!(cursor_at(&b, 10.0, 10.0), None, "nothing set: the class cursor");
        assert_eq!(cursor_at(&b, 250.0, 140.0), Some(Cursor::Pointer));
        assert_eq!(cursor_at(&b, 115.0, 115.0), Some(Cursor::Pointer), "the label inherits");
        assert_eq!(cursor_at(&b, 450.0, 150.0), Some(Cursor::Auto));
        assert_eq!(cursor_at(&b, 550.0, 250.0), Some(Cursor::Pointer));

        assert!(system_cursor(Cursor::Auto).is_none());

        // A link with no `cursor` style shows the hand (browsers' `<a>`,
        // macOS links); a style still wins. Windows showed the arrow, so
        // the idea-ui docs' hero links looked dead.
        // Past the hand-built ids 1..=5, so the real link gets its own.
        b.next_id = 100;
        let link = runtime_vocabulary::caps::LinkOps::create_link(
            &mut b,
            runtime_shared::primitives::link::LinkConfig {
                route: "docs",
                url: "/docs".into(),
                external: false,
                on_activate: std::rc::Rc::new(|| {}),
            },
            &runtime_shared::AccessibilityProps::default(),
        );
        b.nodes.get_mut(&link.id).unwrap().frame = (0.0, 400.0, 100.0, 40.0);
        b.children.entry(1).or_default().push(link.id);
        assert_eq!(cursor_at(&b, 50.0, 420.0), Some(Cursor::Pointer));
        b.nodes.get_mut(&link.id).unwrap().cursor = Some(Cursor::Text);
        assert_eq!(cursor_at(&b, 50.0, 420.0), Some(Cursor::Text), "the author's cursor wins");
        assert_eq!(system_cursor(Cursor::Pointer).map(|c| c.0), Some(IDC_HAND.0));
        assert_eq!(system_cursor(Cursor::Text).map(|c| c.0), Some(IDC_IBEAM.0));
        assert_eq!(system_cursor(Cursor::NotAllowed).map(|c| c.0), Some(IDC_NO.0));
    }
}

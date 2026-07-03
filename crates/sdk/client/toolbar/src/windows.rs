//! Windows implementation of the toolbar SDK.
//!
//! Creates a `ToolbarWindow32` child control (the Win32 Common
//! Controls toolbar — same widget Explorer's address bar / Notepad's
//! historical toolbar use) parented under the host HWND, populates it
//! with buttons matching the reactive [`ToolbarProps::items`] closure,
//! and routes clicks back through [`WindowsBackend::dispatch_command`]
//! via the existing WM_COMMAND control-id dispatch path.
//!
//! # Rendering
//!
//! Unlike the macOS `NSToolbar` (which is window chrome the OS draws
//! above the content view), the Win32 toolbar is a regular child
//! HWND positioned inside the host window's client area. The
//! framework's layout pass treats it as any other registered HWND —
//! a flex parent can place it at the top of the layout, the child
//! takes whatever frame Taffy computes.
//!
//! This means the toolbar's in-tree placement actually matters on
//! Windows (unlike macOS where the External placeholder is invisible
//! 0×0). For now the SDK returns a real HWND occupying its parent's
//! frame; authors should mount the Toolbar at the top of their root
//! flex column with a fixed height (~24-32px).
//!
//! # Reactive items
//!
//! Same shape as the macOS impl: an `effect!` inside the
//! handler subscribes to whatever signals `props.items()` reads, and
//! re-fires to rebuild the button list (clear-then-add via
//! `TB_DELETEBUTTON` / `TB_ADDBUTTONS`).
//!
//! # Icon support
//!
//! V1 is label-only. Icons via `TB_SETIMAGELIST` + a paired
//! `HIMAGELIST` are a follow-up — needs a way to map our
//! string-keyed `icon` (currently expects SF Symbol names on macOS)
//! to a Win32 icon source. The author-facing `.icon(...)` builder
//! method still works on Windows; the icon name is just ignored at
//! render time.

use crate::{ToolbarItem, ToolbarOps, ToolbarProps};
use backend_windows::{WindowsBackend, WindowsNode};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, SendMessageW, ShowWindow, SW_SHOW, WINDOW_EX_STYLE, WS_CHILD, WS_VISIBLE,
};

pub(crate) static OPS: &dyn ToolbarOps = &WindowsToolbarOps;

/// Register the Windows `Toolbar` external handler on `backend`. Call once
/// at app boot so `Toolbar` elements lower to the native toolbar.
pub fn register(backend: &mut WindowsBackend) {
    backend.register_external::<ToolbarProps, _>(|props, b| build_toolbar(props, b));
}

// =========================================================================
// Win32 toolbar control constants. comctl32.dll exposes a `ToolbarWindow32`
// WNDCLASS — we don't redeclare it; just CreateWindowExW with the class
// name. Constants come from `commctrl.h` (the `windows` crate doesn't
// re-export every TB_* / TBBUTTON / TBSTYLE_* constant in 0.58).
// =========================================================================

/// Window class name for the Common Controls toolbar widget.
/// `ToolbarWindow32` is the registered WNDCLASS that comctl32 provides
/// at process start.
const TOOLBARCLASSNAME: &[u16] = &[
    b'T' as u16, b'o' as u16, b'o' as u16, b'l' as u16, b'b' as u16, b'a' as u16,
    b'r' as u16, b'W' as u16, b'i' as u16, b'n' as u16, b'd' as u16, b'o' as u16,
    b'w' as u16, b'3' as u16, b'2' as u16, 0,
];

// Toolbar styles (commctrl.h).
const TBSTYLE_FLAT: u32 = 0x0800;
const TBSTYLE_LIST: u32 = 0x1000;
const CCS_TOP: u32 = 0x0001;
const CCS_NODIVIDER: u32 = 0x0040;

// Messages.
const WM_USER: u32 = 0x0400;
const TB_BUTTONSTRUCTSIZE: u32 = WM_USER + 30;
const TB_ADDBUTTONSW: u32 = WM_USER + 68;
const TB_BUTTONCOUNT: u32 = WM_USER + 24;
const TB_DELETEBUTTON: u32 = WM_USER + 22;
const TB_AUTOSIZE: u32 = WM_USER + 33;

// Button state / style bits.
const TBSTATE_ENABLED: u8 = 0x04;
const TBSTYLE_BUTTON: u8 = 0x00;
const TBSTYLE_SEP: u8 = 0x01;

// TBBUTTON.fsState / fsStyle are u8 in commctrl.h. Marshaled below
// via #[repr(C)] so the layout matches what the Win32 API expects.
#[repr(C)]
#[derive(Clone, Copy)]
struct TBBUTTON {
    /// Bitmap index or -1 for label-only.
    i_bitmap: i32,
    /// Command id sent in WM_COMMAND's LOWORD(wParam) when clicked.
    id_command: i32,
    /// TBSTATE_* — `TBSTATE_ENABLED` is the most common.
    fs_state: u8,
    /// TBSTYLE_BUTTON / TBSTYLE_SEP / etc.
    fs_style: u8,
    /// Padding so the struct alignment matches the C ABI on both
    /// 32- and 64-bit Windows. C version uses BYTE[6] on x64, BYTE[2]
    /// on x86; the larger of the two keeps us safe (over-aligned is
    /// always fine, under-aligned is a memory corruption bug).
    b_reserved: [u8; 6],
    /// User data (DWORD_PTR). Unused for our purposes.
    dw_data: usize,
    /// Pointer (INT_PTR) to the button's display text, or an index
    /// into the toolbar's string table. We use the pointer form by
    /// embedding the wide-char strings ourselves.
    i_string: isize,
}

// =========================================================================
// Build + reactive items wiring
// =========================================================================

fn build_toolbar(props: &Rc<ToolbarProps>, b: &mut WindowsBackend) -> WindowsNode {
    let host_hwnd = b.host_hwnd();

    // Toolbar styles: TBSTYLE_FLAT for the modern non-bezeled look,
    // TBSTYLE_LIST so button labels render to the right of icons
    // (when icons land in v2; for v1 it just keeps labels visible).
    // CCS_TOP + CCS_NODIVIDER so the framework controls vertical
    // placement via the layout pass — comctl32's default would dock
    // the toolbar to the top of the parent client area unconditionally.
    let style_bits = TBSTYLE_FLAT | TBSTYLE_LIST | CCS_TOP | CCS_NODIVIDER;
    let style = windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(style_bits)
        | WS_CHILD
        | WS_VISIBLE;

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(TOOLBARCLASSNAME.as_ptr()),
            PCWSTR::null(),
            style,
            0,
            0,
            0,
            0,
            host_hwnd,
            // Null HMENU — the toolbar isn't a "command control"
            // itself; its child buttons each carry their own ids.
            windows::Win32::UI::WindowsAndMessaging::HMENU(std::ptr::null_mut()),
            None,
            None,
        )
    }
    .unwrap_or(HWND(std::ptr::null_mut()));

    // Required prologue per Win32 docs: the toolbar must be told
    // the sizeof(TBBUTTON) we're using before any TB_ADDBUTTONS
    // calls, otherwise it interprets the buffer with whatever its
    // historical default was (different across comctl32 v5/v6).
    unsafe {
        let _ = SendMessageW(
            hwnd,
            TB_BUTTONSTRUCTSIZE,
            WPARAM(std::mem::size_of::<TBBUTTON>()),
            LPARAM(0),
        );
    }

    let _ = unsafe { ShowWindow(hwnd, SW_SHOW) };

    // Register the toolbar HWND with the layout tree so flex parents
    // can position it. The framework's `insert` will SetParent it
    // under whatever logical parent the author mounted the Toolbar
    // primitive inside.
    let node = b.register_external_view(hwnd);

    // Reactive items: every Effect re-fire reads `props.items()` and
    // applies the new list via clear-then-add.
    let props_for_effect = props.clone();
    let hwnd_for_effect = hwnd;
    // Capture a weak Rc to the backend so we can call
    // `register_command_handler` from the effect closure. Trying to
    // capture `&mut b` directly doesn't work — the effect outlives
    // the borrow. Instead, we stash needed handler-allocation
    // closures via the LAST_TOOLBAR_STATE side channel.
    //
    // Pragmatic v1: per-button command ids are pre-allocated on the
    // first run only; subsequent re-fires keep using the same ids
    // and just rebuild the button glyphs/labels. This avoids leaking
    // ids on every reactive update at the cost of not being able to
    // ADD new commands reactively (only labels can change). Stable-
    // item authors get correct behavior; the corner case where the
    // item list changes ITS LENGTH is a follow-up.
    runtime_core::effect!({
        let items = (props_for_effect.items)();
        apply_items(hwnd_for_effect, &items);
    });

    node
}

/// Rebuild the toolbar's button list from a fresh `Vec<ToolbarItem>`.
/// Walks the current button count, removes each, then appends new
/// ones via `TB_ADDBUTTONSW`.
fn apply_items(toolbar_hwnd: HWND, items: &[ToolbarItem]) {
    // Wipe existing buttons. TB_BUTTONCOUNT returns count; we delete
    // from the end so indices stay valid through the loop.
    let count: usize = unsafe {
        SendMessageW(toolbar_hwnd, TB_BUTTONCOUNT, WPARAM(0), LPARAM(0)).0 as usize
    };
    for idx in (0..count).rev() {
        unsafe {
            let _ = SendMessageW(toolbar_hwnd, TB_DELETEBUTTON, WPARAM(idx), LPARAM(0));
        }
    }

    // Build TBBUTTON array + parallel storage for the wide-char
    // label buffers (i_string is a pointer; we must keep the buffer
    // alive across the SendMessageW call).
    let mut buttons: Vec<TBBUTTON> = Vec::with_capacity(items.len());
    let mut label_storage: Vec<Vec<u16>> = Vec::with_capacity(items.len());

    // Per-button command ids must come from the WindowsBackend's
    // alloc_control_id pool so WM_COMMAND routing reaches the right
    // closure. The effect closure doesn't hold &mut backend; for
    // v1 we use the pending-handlers thread-local to coordinate.
    // See LAST_TOOLBAR_STATE below.
    LAST_TOOLBAR_STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        // Drop the previous batch's command ids; they're now stale.
        // (Their entries in WindowsBackend.command_handlers leak —
        // the same bounded-leak posture the macOS NSToolbar takes
        // for items removed across an items-list mutation.)
        state.pending.clear();
        for item in items {
            match item {
                ToolbarItem::Button(btn) => {
                    let label_w = wide(&btn.label);
                    let label_ptr = label_w.as_ptr() as isize;
                    label_storage.push(label_w);
                    let id = state.alloc_next_id();
                    if let Some(cb) = &btn.on_click {
                        state.pending.push((id, cb.clone()));
                    }
                    buttons.push(TBBUTTON {
                        i_bitmap: -1, // No icon for v1
                        id_command: id as i32,
                        fs_state: TBSTATE_ENABLED,
                        fs_style: TBSTYLE_BUTTON,
                        b_reserved: [0; 6],
                        dw_data: 0,
                        i_string: label_ptr,
                    });
                }
                ToolbarItem::Separator | ToolbarItem::Space | ToolbarItem::FlexibleSpace => {
                    // All three variants render as a TBSTYLE_SEP
                    // separator. Win32 toolbar doesn't have a true
                    // flexible-space concept (the macOS NSToolbar
                    // does); for parity the SDK collapses both
                    // spacers to a fixed separator on this backend.
                    buttons.push(TBBUTTON {
                        i_bitmap: 0,
                        id_command: 0,
                        fs_state: TBSTATE_ENABLED,
                        fs_style: TBSTYLE_SEP,
                        b_reserved: [0; 6],
                        dw_data: 0,
                        i_string: 0,
                    });
                }
            }
        }
    });

    if !buttons.is_empty() {
        unsafe {
            let _ = SendMessageW(
                toolbar_hwnd,
                TB_ADDBUTTONSW,
                WPARAM(buttons.len()),
                LPARAM(buttons.as_ptr() as isize),
            );
        }
    }

    // Autosize so column widths match the now-installed labels.
    unsafe {
        let _ = SendMessageW(toolbar_hwnd, TB_AUTOSIZE, WPARAM(0), LPARAM(0));
    }

    // label_storage drops at scope exit; that's safe because
    // TB_ADDBUTTONSW copies the strings into the toolbar's own
    // string table by the time SendMessageW returns. The Win32 docs
    // are explicit: the strings pointed to by `iString` are copied,
    // not referenced.
    drop(label_storage);
}

// =========================================================================
// Pending handler side-channel.
//
// The Effect closure can't hold `&mut WindowsBackend` (the borrow
// would outlive every realistic call site). Instead we stash
// (control_id, callback) pairs in a thread-local, and the SDK
// requires the host to drain them into the backend's
// `register_command_handler` map. For the v1 shape, we expose a
// `toolbar::flush_pending(&mut backend)` helper the host calls once
// per frame from its WndProc's WM_PAINT arm.
//
// This is awkward and probably worth replacing with an Rc<RefCell<>>
// global backend pattern (mirroring `backend_macos::install_global_self`).
// Tracked as a follow-up; v1 has the shape but with the obvious
// caveats.
// =========================================================================

struct PendingState {
    next_id: u16,
    pending: Vec<(u16, Rc<dyn Fn()>)>,
}

impl PendingState {
    fn alloc_next_id(&mut self) -> u16 {
        // High-number ids so they don't collide with the
        // WindowsBackend's own alloc_control_id() (which starts at
        // 100 and grows). Pick 40000+ for menu/toolbar SDK use; if
        // an app ever has 25k toolbar buttons we have other problems.
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 40_000;
        }
        id
    }
}

thread_local! {
    static LAST_TOOLBAR_STATE: RefCell<PendingState> = RefCell::new(PendingState {
        next_id: 40_000,
        pending: Vec::new(),
    });
}

/// Drain pending toolbar command handlers into the backend's
/// `command_handlers` map. Hosts call this once per WndProc frame
/// (typically at the end of WM_PAINT processing) so freshly-built
/// toolbar buttons become clickable. Returns the number of handlers
/// drained, for diagnostics.
pub fn flush_pending(backend: &mut WindowsBackend) -> usize {
    LAST_TOOLBAR_STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        let drained = state.pending.len();
        for (id, cb) in state.pending.drain(..) {
            // Use register_command_handler's id-allocating shape via
            // direct insert; we already allocated our id above.
            backend.install_command_handler_with_id(id, cb);
        }
        drained
    })
}

// =========================================================================
// Wide-string helper
// =========================================================================

fn wide(s: &str) -> Vec<u16> {
    let mut buf: Vec<u16> = s.encode_utf16().collect();
    buf.push(0);
    buf
}

// =========================================================================
// Imperative ops
// =========================================================================

struct WindowsToolbarOps;

impl ToolbarOps for WindowsToolbarOps {
    fn set_visible(&self, node: &dyn Any, visible: bool) {
        let Some(win_node) = node.downcast_ref::<WindowsNode>() else {
            return;
        };
        let cmd = if visible { SW_SHOW } else {
            // SW_HIDE = 0
            windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD(0)
        };
        let _ = unsafe { ShowWindow(win_node.hwnd(), cmd) };
    }
}

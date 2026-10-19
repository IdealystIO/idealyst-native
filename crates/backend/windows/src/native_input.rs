//! Real Win32 input for the robot (`IntrospectionOps::hit_contains` /
//! `native_pointer`).
//!
//! Points arrive in `absolute_frame`'s space: logical px in the host's
//! client area. The hit test is the scene's own (`scene::hit_chain`, the
//! walk the host's `WM_LBUTTONUP` routing uses to find a pressable).
//!
//! Each step is POSTED as the mouse message a real click produces
//! (`WM_MOUSEMOVE`, `WM_LBUTTONDOWN` with `MK_LBUTTON`, `WM_LBUTTONUP`) to
//! the window a real click there lands on: the deepest visible child of
//! the host under the point (`ChildWindowFromPointEx`, as the system
//! routes mouse input to child windows), in that window's client device
//! pixels. Posting, not sending, puts them through the host's message loop
//! — its pre-dispatch hooks (the app pointer sink) and then the window
//! procedure — exactly as input from the system queue.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::MapWindowPoints;
use windows::Win32::UI::WindowsAndMessaging::{
    ChildWindowFromPointEx, PostMessageW, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE,
};

use runtime_vocabulary::caps::{NativeInjection, PointerStep, PointerStepPhase};

use crate::WindowsBackend;

/// `MK_LBUTTON`: the primary button is down.
const MK_LBUTTON: usize = 0x0001;

fn lparam(x: i32, y: i32) -> LPARAM {
    LPARAM((((y as u16) as u32) << 16 | (x as u16) as u32) as isize)
}

/// The deepest visible child of `host` under its client point `pt`
/// (device px), and `pt` in that window's client coordinates.
fn target_under(host: HWND, pt: POINT) -> (HWND, POINT) {
    let (mut hwnd, mut local) = (host, pt);
    loop {
        let child = unsafe { ChildWindowFromPointEx(hwnd, local, CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT) };
        if child.is_invalid() || child == hwnd {
            return (hwnd, local);
        }
        let mut p = [local];
        unsafe { MapWindowPoints(Some(hwnd), Some(child), &mut p) };
        hwnd = child;
        local = p[0];
    }
}

impl WindowsBackend {
    /// `IntrospectionOps::hit_contains`.
    pub(crate) fn hit_contains_impl(&self, id: u64, x: f32, y: f32) -> Option<bool> {
        self.nodes.get(&id)?;
        Some(crate::scene::hit_chain(self, x, y).iter().any(|n| n.id == id))
    }

    /// `IntrospectionOps::native_pointer`.
    pub(crate) fn native_pointer_impl(&self, steps: &[PointerStep]) -> Option<NativeInjection> {
        let host = self.host_hwnd;
        if host.is_invalid() {
            return None;
        }
        let messages: Vec<(u32, usize, POINT)> = steps
            .iter()
            .map(|s| {
                let (dx, dy) = (self.dpi.to_device(s.x), self.dpi.to_device(s.y));
                let pt = POINT { x: dx.round() as i32, y: dy.round() as i32 };
                match s.phase {
                    PointerStepPhase::Down => (WM_LBUTTONDOWN, MK_LBUTTON, pt),
                    PointerStepPhase::Move => (WM_MOUSEMOVE, MK_LBUTTON, pt),
                    PointerStepPhase::Up => (WM_LBUTTONUP, 0, pt),
                    PointerStepPhase::Hover => (WM_MOUSEMOVE, 0, pt),
                }
            })
            .collect();
        let host = host.0 as isize;
        Some(Box::new(move || {
            let host = HWND(host as *mut std::ffi::c_void);
            // The press's window takes the whole gesture, as mouse capture
            // gives it the moves and the release.
            let mut captured: Option<HWND> = None;
            for (msg, keys, pt) in messages {
                let (hwnd, local) = match captured {
                    Some(h) => {
                        let mut p = [pt];
                        unsafe { MapWindowPoints(Some(host), Some(h), &mut p) };
                        (h, p[0])
                    }
                    None => target_under(host, pt),
                };
                if msg == WM_LBUTTONDOWN {
                    captured = Some(hwnd);
                }
                unsafe { PostMessageW(Some(hwnd), msg, WPARAM(keys), lparam(local.x, local.y)) }
                    .map_err(|e| format!("PostMessageW: {e}"))?;
            }
            Ok(())
        }))
    }
}

#[cfg(test)]
mod tests {
    /// A mouse message's `lParam` carries the client point as two signed
    /// 16-bit words, x low — what `GET_X_LPARAM` / `GET_Y_LPARAM` read back.
    #[test]
    fn the_point_packs_as_the_mouse_message_reads_it() {
        let lp = super::lparam(300, 41).0 as u32;
        assert_eq!((lp & 0xffff, lp >> 16), (300, 41));
        let lp = super::lparam(-2, 5).0 as u32;
        assert_eq!(((lp & 0xffff) as u16 as i16, (lp >> 16) as u16 as i16), (-2, 5));
    }
}

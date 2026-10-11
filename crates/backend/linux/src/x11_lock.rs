//! Pointer lock on GDK's X11 backend — the only GTK4 display backend on
//! which it can be done.
//!
//! ## Why X11 only
//!
//! GTK4 has no pointer-lock API on any backend (GTK3's `gdk_device_grab`
//! / `gdk_seat_grab` were removed). Underneath:
//!
//! - **X11**: a lock is an active pointer grab CONFINED to the window
//!   (`XGrabPointer(confine_to = window)`) plus warping the pointer back
//!   to the window centre after each motion, measuring each motion against
//!   the previous one (`pointer_policy::LockTracker` handles the warp's
//!   own echo event). The cursor is hidden by the pointer
//!   source (`app_pointer.rs`). This is what SDL2 does on X11.
//! - **Wayland**: a client cannot warp or confine the pointer at all; it
//!   needs the `zwp_pointer_constraints_v1` + `zwp_relative_pointer_v1`
//!   protocols bound on the compositor's registry. GDK binds them for its
//!   own internal use but exposes neither the objects nor a way to request
//!   a lock, and a second `wl_display` connection cannot lock a surface it
//!   does not own. So on Wayland [`X11Lock::acquire`] returns `None` and
//!   the source reports the request as refused — never a fake lock.
//!
//! ## Why `dlsym`, not a link-time dependency
//!
//! The GDK X11 entry points (`gdk_x11_display_get_xdisplay`,
//! `gdk_x11_surface_get_xid`) live in `libgtk-4` itself, but only when
//! GTK was built with the X11 backend; libX11 is mapped only when GDK
//! uses it. Linking either would make the backend fail to LOAD on a
//! Wayland-only GTK build. Resolving at runtime from the already-mapped
//! libraries (`RTLD_NOLOAD` for libX11 — never loading it ourselves) adds
//! no crate and degrades to "unsupported" when a symbol is absent, the
//! same pattern `gl_loader.rs` uses for libepoxy.
//!
//! ## Why GTK's own `Display*`
//!
//! The grab is made on GDK's X connection, not a private one: a grab
//! belongs to the CLIENT that made it, and with `owner_events = True` the
//! server keeps delivering events for that client's own windows normally
//! — including the XInput2 events GDK selects — so GTK's event path (and
//! our capture controller) is unchanged. A grab from a second connection
//! would steal every pointer event away from GTK's connection.

use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};
use std::sync::OnceLock;

use gtk4::glib::translate::ToGlibPtr;
use gtk4::prelude::*;

type XDisplay = c_void;
type XWindow = c_ulong;

// Xlib constants (X.h).
const BUTTON_PRESS_MASK: c_uint = 1 << 2;
const BUTTON_RELEASE_MASK: c_uint = 1 << 3;
const POINTER_MOTION_MASK: c_uint = 1 << 6;
const GRAB_MODE_ASYNC: c_int = 1;
const GRAB_SUCCESS: c_int = 0;
const CURRENT_TIME: c_ulong = 0;
const NONE: c_ulong = 0;

struct Api {
    get_xdisplay: unsafe extern "C" fn(*mut gtk4::gdk::ffi::GdkDisplay) -> *mut XDisplay,
    get_xid: unsafe extern "C" fn(*mut gtk4::gdk::ffi::GdkSurface) -> XWindow,
    #[allow(clippy::type_complexity)]
    grab_pointer: unsafe extern "C" fn(
        *mut XDisplay,
        XWindow,
        c_int,
        c_uint,
        c_int,
        c_int,
        XWindow,
        c_ulong,
        c_ulong,
    ) -> c_int,
    ungrab_pointer: unsafe extern "C" fn(*mut XDisplay, c_ulong) -> c_int,
    #[allow(clippy::type_complexity)]
    warp_pointer: unsafe extern "C" fn(
        *mut XDisplay,
        XWindow,
        XWindow,
        c_int,
        c_int,
        c_uint,
        c_uint,
        c_int,
        c_int,
    ) -> c_int,
    flush: unsafe extern "C" fn(*mut XDisplay) -> c_int,
}

static API: OnceLock<Option<Api>> = OnceLock::new();

unsafe fn sym(handle: *mut c_void, name: &[u8]) -> *mut c_void {
    debug_assert!(name.ends_with(b"\0"));
    unsafe { libc::dlsym(handle, name.as_ptr() as *const c_char) }
}

fn api() -> Option<&'static Api> {
    API.get_or_init(|| unsafe {
        let gdk_dpy = sym(libc::RTLD_DEFAULT, b"gdk_x11_display_get_xdisplay\0");
        let gdk_xid = sym(libc::RTLD_DEFAULT, b"gdk_x11_surface_get_xid\0");
        // NOLOAD: only if GDK already mapped libX11 — never load it.
        let x11 = libc::dlopen(b"libX11.so.6\0".as_ptr() as *const c_char, libc::RTLD_NOW | libc::RTLD_NOLOAD);
        if gdk_dpy.is_null() || gdk_xid.is_null() || x11.is_null() {
            return None;
        }
        let grab = sym(x11, b"XGrabPointer\0");
        let ungrab = sym(x11, b"XUngrabPointer\0");
        let warp = sym(x11, b"XWarpPointer\0");
        let flush = sym(x11, b"XFlush\0");
        if grab.is_null() || ungrab.is_null() || warp.is_null() || flush.is_null() {
            return None;
        }
        Some(Api {
            get_xdisplay: std::mem::transmute::<*mut c_void, _>(gdk_dpy),
            get_xid: std::mem::transmute::<*mut c_void, _>(gdk_xid),
            grab_pointer: std::mem::transmute::<*mut c_void, _>(grab),
            ungrab_pointer: std::mem::transmute::<*mut c_void, _>(ungrab),
            warp_pointer: std::mem::transmute::<*mut c_void, _>(warp),
            flush: std::mem::transmute::<*mut c_void, _>(flush),
        })
    })
    .as_ref()
}

/// `true` when `display` is GDK's X11 backend (the only one a lock works
/// on — see the module docs). Checked by GType name so no `gdk4-x11`
/// crate is needed.
pub(crate) fn is_x11(display: &gtk4::gdk::Display) -> bool {
    display.type_().name() == "GdkX11Display"
}

/// An active lock: the confined grab, plus where to put the pointer back.
pub(crate) struct X11Lock {
    xdpy: *mut XDisplay,
    xid: XWindow,
    /// Device-pixel scale of the surface (X coordinates are device px,
    /// GDK event coordinates are logical).
    scale: i32,
    /// Lock point in SURFACE coordinates (logical px).
    center: (i32, i32),
    /// Where the pointer was when the lock began (surface coords), so the
    /// cursor reappears there on release instead of at the centre.
    restore: Option<(f64, f64)>,
}

impl X11Lock {
    /// Grab + confine + warp to the centre. `None` when not on X11, the
    /// window has no surface yet, or the server refused the grab
    /// (another client holds one, the window is not viewable).
    pub(crate) fn acquire(window: &gtk4::Window, restore: Option<(f64, f64)>) -> Option<Self> {
        let display = WidgetExt::display(window);
        if !is_x11(&display) {
            return None;
        }
        let api = api()?;
        let surface = window.surface()?;
        let xdpy = unsafe { (api.get_xdisplay)(display.to_glib_none().0) };
        let xid = unsafe { (api.get_xid)(surface.to_glib_none().0) };
        if xdpy.is_null() || xid == 0 {
            return None;
        }
        let status = unsafe {
            (api.grab_pointer)(
                xdpy,
                xid,
                1, // owner_events: keep GDK's own (XI2) delivery — module docs.
                BUTTON_PRESS_MASK | BUTTON_RELEASE_MASK | POINTER_MOTION_MASK,
                GRAB_MODE_ASYNC,
                GRAB_MODE_ASYNC,
                xid, // confine_to: the pointer cannot leave the window.
                NONE,
                CURRENT_TIME,
            )
        };
        if status != GRAB_SUCCESS {
            return None;
        }
        let lock = X11Lock {
            xdpy,
            xid,
            scale: surface.scale_factor().max(1),
            center: crate::pointer_policy::lock_center(surface.width(), surface.height()),
            restore,
        };
        lock.warp(lock.center.0 as f64, lock.center.1 as f64);
        Some(lock)
    }

    /// The lock point, surface coordinates.
    pub(crate) fn center(&self) -> (i32, i32) {
        self.center
    }

    /// Put the pointer back on the lock point (after each reported motion).
    pub(crate) fn recenter(&self) {
        self.warp(self.center.0 as f64, self.center.1 as f64);
    }

    fn warp(&self, x: f64, y: f64) {
        let Some(api) = api() else { return };
        unsafe {
            (api.warp_pointer)(
                self.xdpy,
                0,
                self.xid,
                0,
                0,
                0,
                0,
                (x * self.scale as f64).round() as c_int,
                (y * self.scale as f64).round() as c_int,
            );
            (api.flush)(self.xdpy);
        }
    }

    /// Ungrab. With `restore_cursor`, also put the cursor back where the
    /// lock began — only on an app-requested release: after a focus loss
    /// the user is working in another window, and warping their pointer
    /// there would be hostile.
    pub(crate) fn release(self, restore_cursor: bool) {
        let Some(api) = api() else { return };
        unsafe {
            (api.ungrab_pointer)(self.xdpy, CURRENT_TIME);
        }
        if let (true, Some((x, y))) = (restore_cursor, self.restore) {
            self.warp(x, y);
        } else {
            unsafe {
                (api.flush)(self.xdpy);
            }
        }
    }
}

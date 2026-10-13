//! `user_activity()` on GTK, driven by REAL X input: a press, a wheel
//! notch and a key press anywhere in the window tick the heartbeat; the
//! widget under the pointer still gets every event (observe-only); a press
//! or key an app listener claims with `PreventDefault` still counts; and
//! the boot path installs (and `stop` removes) the observer.
//!
//! ## The bug this pins
//!
//! The GTK backend fed nothing into `runtime_shared::user_activity`, so an
//! idle clock built on `user_activity()` (kiosk "still there?", session
//! timeout, auto-lock) expired while the person was using the app.
//!
//! Input is injected at the X server with XTest (like `app_pointer.rs`),
//! so the path is the real one: X → GDK → GTK capture propagation → the
//! legacy controller. Needs `xvfb-run` and `libXtst.so.6` (the
//! `idealyst-linux-gtk-test:xtst` image); without either the test prints
//! SKIP and passes — grep the output.
//!
//! ONE GTK `#[test]` in this binary: GTK may only be driven from the thread
//! that ran `gtk::init`, and cargo gives every `#[test]` its own thread.

#![cfg(target_os = "linux")]

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};
use std::rc::Rc;
use std::time::{Duration, Instant};

use backend_linux::user_activity::UserActivitySource;
use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::glib::translate::ToGlibPtr;
use gtk4::prelude::*;
use runtime_shared::pointer_input::{PointerOutcome, PointerPhase, PointerSink};
use runtime_shared::primitives::key::{KeyOutcome, KeyPhase, KeyboardSink};
use runtime_shared::user_activity;
use runtime_vocabulary::caps::AppEnvOps;

/// `XK_a` (`X11/keysymdef.h`).
const XK_A: c_ulong = 0x0061;

/// Minimal Xlib + XTest, resolved at runtime (libX11 is already mapped by
/// GDK; libXtst is loaded here).
struct X {
    dpy: *mut c_void,
    warp: unsafe extern "C" fn(*mut c_void, c_ulong, c_ulong, c_int, c_int, c_uint, c_uint, c_int, c_int) -> c_int,
    sync: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    button: unsafe extern "C" fn(*mut c_void, c_uint, c_int, c_ulong) -> c_int,
    key: unsafe extern "C" fn(*mut c_void, c_uint, c_int, c_ulong) -> c_int,
    keysym_to_keycode: unsafe extern "C" fn(*mut c_void, c_ulong) -> u8,
}

unsafe fn sym<T>(h: *mut c_void, name: &[u8]) -> Option<T> {
    let p = unsafe { libc::dlsym(h, name.as_ptr() as *const c_char) };
    (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
}

impl X {
    fn open() -> Option<X> {
        unsafe {
            let x11 = libc::dlopen(b"libX11.so.6\0".as_ptr() as *const c_char, libc::RTLD_NOW);
            let xtst = libc::dlopen(b"libXtst.so.6\0".as_ptr() as *const c_char, libc::RTLD_NOW);
            if x11.is_null() || xtst.is_null() {
                return None;
            }
            let open: unsafe extern "C" fn(*const c_char) -> *mut c_void = sym(x11, b"XOpenDisplay\0")?;
            let dpy = open(std::ptr::null());
            if dpy.is_null() {
                return None;
            }
            Some(X {
                dpy,
                warp: sym(x11, b"XWarpPointer\0")?,
                sync: sym(x11, b"XSync\0")?,
                button: sym(xtst, b"XTestFakeButtonEvent\0")?,
                key: sym(xtst, b"XTestFakeKeyEvent\0")?,
                keysym_to_keycode: sym(x11, b"XKeysymToKeycode\0")?,
            })
        }
    }
    fn warp_to(&self, xid: c_ulong, x: i32, y: i32) {
        unsafe {
            (self.warp)(self.dpy, 0, xid, 0, 0, 0, 0, x, y);
            (self.sync)(self.dpy, 0);
        }
    }
    fn click(&self, button: u32) {
        unsafe {
            (self.button)(self.dpy, button, 1, 0);
            (self.button)(self.dpy, button, 0, 0);
            (self.sync)(self.dpy, 0);
        }
    }
    fn tap_key(&self, keysym: c_ulong) {
        unsafe {
            let code = (self.keysym_to_keycode)(self.dpy, keysym) as c_uint;
            (self.key)(self.dpy, code, 1, 0);
            (self.key)(self.dpy, code, 0, 0);
            (self.sync)(self.dpy, 0);
        }
    }
}

fn xid_of(window: &gtk4::Window) -> c_ulong {
    unsafe {
        let get: unsafe extern "C" fn(*mut gtk4::gdk::ffi::GdkSurface) -> c_ulong =
            sym(libc::RTLD_DEFAULT, b"gdk_x11_surface_get_xid\0").expect("gdk_x11_surface_get_xid");
        get(window.surface().expect("realized").to_glib_none().0)
    }
}

fn pump_until(mut cond: impl FnMut() -> bool, ms: u64) -> bool {
    let ctx = gtk4::glib::MainContext::default();
    let end = Instant::now() + Duration::from_millis(ms);
    loop {
        while ctx.iteration(false) {}
        if cond() {
            return true;
        }
        if Instant::now() > end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn pump(ms: u64) {
    pump_until(|| false, ms);
}

fn legacy_controllers(window: &gtk4::Window) -> usize {
    window
        .observe_controllers()
        .iter::<gtk4::glib::Object>()
        .filter(|c| c.as_ref().ok().is_some_and(|c| c.is::<gtk4::EventControllerLegacy>()))
        .count()
}

#[test]
fn regression_gtk_user_activity_reports_real_x_input() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    let display = gtk4::gdk::Display::default().expect("display");
    if display.type_().name() != "GdkX11Display" {
        eprintln!("SKIP: not an X11 display (XTest needs X11)");
        return;
    }
    let Some(x) = X::open() else {
        eprintln!("SKIP: libXtst.so.6 not available");
        return;
    };

    // A window whose content has its own press + scroll handlers — the
    // widget under the pointer, which must still see everything.
    let window = gtk4::Window::new();
    window.set_default_size(400, 300);
    let child = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    child.set_hexpand(true);
    child.set_vexpand(true);
    let child_presses = Rc::new(Cell::new(0));
    let click = gtk4::GestureClick::new();
    click.set_button(0);
    let cp = child_presses.clone();
    click.connect_pressed(move |_, _, _, _| cp.set(cp.get() + 1));
    child.add_controller(click);
    let child_scrolls = Rc::new(Cell::new(0));
    let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
    let cs = child_scrolls.clone();
    scroll.connect_scroll(move |_, _, _| {
        cs.set(cs.get() + 1);
        gtk4::glib::Propagation::Proceed
    });
    child.add_controller(scroll);
    window.set_child(Some(&child));
    window.present();
    assert!(pump_until(|| window.is_mapped() && window.width() > 0, 3000), "window never mapped");
    pump(100);
    let xid = xid_of(&window);

    // --- Part 1: the observer itself, on a plain window. ------------------
    let source = UserActivitySource::install(&window);

    // Hover is not activity.
    user_activity::reset_for_tests();
    x.warp_to(xid, 100, 100);
    pump(150);
    assert_eq!(user_activity::count(), 0, "motion / crossing is not activity");

    // A press ticks, and still reaches the widget.
    user_activity::reset_for_tests();
    x.click(1);
    assert!(pump_until(|| user_activity::count() > 0, 2000), "press never reported");
    assert!(pump_until(|| child_presses.get() == 1, 2000), "observe-only: the press reaches the widget");
    pump(50);
    assert_eq!(user_activity::count(), 1, "one press (down + up) is one tick");

    // A wheel notch ticks, and still scrolls the widget.
    user_activity::reset_for_tests();
    x.click(5);
    assert!(pump_until(|| user_activity::count() > 0, 2000), "wheel never reported");
    assert!(pump_until(|| child_scrolls.get() > 0, 2000), "observe-only: the wheel reaches the widget");

    // A key press ticks (needs keyboard focus).
    if pump_until(|| window.is_active(), 1000) {
        user_activity::reset_for_tests();
        x.tap_key(XK_A);
        assert!(pump_until(|| user_activity::count() > 0, 2000), "key press never reported");
        pump(50);
        assert_eq!(user_activity::count(), 1, "press + release is one tick");
    } else {
        eprintln!("SKIP(key section): window never became active, X routes no keys to it");
    }

    // Uninstall: nothing more is reported.
    let with = legacy_controllers(&window);
    source.uninstall();
    assert_eq!(legacy_controllers(&window), with - 1, "uninstall removes the controller");
    user_activity::reset_for_tests();
    x.click(1);
    assert!(pump_until(|| child_presses.get() == 2, 2000));
    pump(50);
    assert_eq!(user_activity::count(), 0, "nothing reported after uninstall");
    window.destroy();
    pump(100);

    // --- Part 2: the boot path, and input an app listener CLAIMS. ---------
    // GTK runs a window's capture controllers newest-first and a claiming
    // app source returns `Stop`, so the observer must be raised above each
    // source as it is installed — without that this section saw 0 claimed
    // presses. (`newcore::start` installs a time source, so the throttle is
    // live from here on: one event per check.)
    let window2 = gtk4::Window::new();
    window2.set_default_size(400, 300);
    let baseline = legacy_controllers(&window2);
    let backend2 = Rc::new(RefCell::new(LinuxBackend::new(window2.clone())));
    backend2.borrow_mut().set_self_ref(Rc::downgrade(&backend2));
    let app = newcore::start(backend2.clone(), |_| {}, || runtime_vocabulary::builders::view().build());
    assert_eq!(legacy_controllers(&window2), baseline + 1, "boot installs the observer, unconditionally");
    window2.present();
    assert!(pump_until(|| window2.is_mapped() && window2.width() > 0, 3000), "window2 never mapped");
    pump(100);
    let xid2 = xid_of(&window2);

    let claims = Rc::new(Cell::new(0));
    let c = claims.clone();
    backend2.borrow_mut().set_pointer_sink(Some(PointerSink::new(
        move |e| {
            if e.phase == PointerPhase::Down {
                c.set(c.get() + 1);
                PointerOutcome::PreventDefault
            } else {
                PointerOutcome::Default
            }
        },
        |_| {},
        || {},
    )));
    x.warp_to(xid2, 120, 120);
    pump(100);
    user_activity::reset_for_tests();
    x.click(1);
    assert!(pump_until(|| claims.get() == 1, 2000), "the pointer sink saw (and claimed) the press");
    pump(50);
    assert_eq!(user_activity::count(), 1, "a claimed press still counts");

    if pump_until(|| window2.is_active(), 1000) {
        let keys = Rc::new(Cell::new(0));
        let k = keys.clone();
        backend2.borrow_mut().set_keyboard_sink(Some(KeyboardSink::new(
            move |e| {
                if e.phase == KeyPhase::Down {
                    k.set(k.get() + 1);
                }
                KeyOutcome::PreventDefault
            },
            || {},
        )));
        user_activity::reset_for_tests();
        x.tap_key(XK_A);
        assert!(pump_until(|| keys.get() == 1, 2000), "the key sink saw (and claimed) the key");
        pump(50);
        assert_eq!(user_activity::count(), 1, "a claimed key still counts");
        backend2.borrow_mut().set_keyboard_sink(None);
    } else {
        eprintln!("SKIP(claimed-key section): window2 never became active");
    }
    backend2.borrow_mut().set_pointer_sink(None);

    app.stop();
    assert_eq!(legacy_controllers(&window2), baseline, "stop removes the observer");
}

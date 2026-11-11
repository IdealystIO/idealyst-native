//! A press-down pressable (`PressActivation::Down`) fires at the primary
//! press, once, and the rest of that press fires nothing — driven by REAL
//! X input.
//!
//! ## What this pins
//!
//! `PressableOps::create_pressable_on_down` (a menu button whose menu is
//! open before the release). The contract every backend keeps:
//!
//! - a primary press fires `on_press` once, at the down;
//! - the release (over the control or off it) fires nothing more and does
//!   not reach ancestors: a release-mode pressable around it stays silent;
//! - a secondary press fires nothing;
//! - its press still lights the node's `PRESSED` state for the whole hold,
//!   like any pressable;
//! - a release-mode pressable is unchanged (nothing at the down, fires at
//!   the release).
//!
//! ## Why real X input
//!
//! The down-mode gesture claims its sequence so GTK cancels it for the
//! ancestors; whether that keeps the release away from an ancestor's
//! `GestureClick`, and whether it cancels the sibling `GestureClick` that
//! drives `PRESSED`, is GTK's propagation and grouping behaviour. Emitting
//! the gesture's signals by hand would skip exactly that, so the press is
//! injected at the X server with XTest (X server → GDK → GTK capture /
//! bubble → the controllers). Needs an X display (`xvfb-run`) and
//! `libXtst.so.6` (the `idealyst-linux-gtk-test:xtst` image); without
//! either the test prints SKIP and passes — grep the output.
//!
//! ONE GTK `#[test]` in this binary: GTK may only be driven from the thread
//! that ran `gtk::init`, and cargo gives every `#[test]` its own thread.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};
use std::rc::Rc;
use std::time::{Duration, Instant};

use backend_linux::{gtk4, LinuxBackend};
use gtk4::glib::translate::ToGlibPtr;
use gtk4::prelude::*;
use runtime_scene::Host;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{Length, Position, StateBits, StyleRules, Tokenized};
use runtime_vocabulary::caps::{LifecycleOps, PressableOps, StyleOps, ViewOps};

/// Minimal Xlib + XTest, resolved at runtime (libX11 is already mapped by
/// GDK; libXtst is loaded here).
struct X {
    dpy: *mut c_void,
    warp: unsafe extern "C" fn(*mut c_void, c_ulong, c_ulong, c_int, c_int, c_uint, c_uint, c_int, c_int) -> c_int,
    sync: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    button: unsafe extern "C" fn(*mut c_void, c_uint, c_int, c_ulong) -> c_int,
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
            })
        }
    }
    fn warp_to(&self, xid: c_ulong, (x, y): (i32, i32)) {
        unsafe {
            (self.warp)(self.dpy, 0, xid, 0, 0, 0, 0, x, y);
            (self.sync)(self.dpy, 0);
        }
    }
    fn button(&self, button: u32, press: bool) {
        unsafe {
            (self.button)(self.dpy, button, press as c_int, 0);
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

/// Lets queued X input reach the controllers; a check that something did
/// NOT fire has no condition to wait on.
fn settle() {
    pump_until(|| false, 250);
}

fn absolute(left: f32, top: f32, w: f32, h: f32) -> Rc<StyleRules> {
    Rc::new(StyleRules {
        position: Some(Position::Absolute),
        left: Some(Tokenized::Literal(Length::Px(left))),
        top: Some(Tokenized::Literal(Length::Px(top))),
        width: Some(Tokenized::Literal(Length::Px(w))),
        height: Some(Tokenized::Literal(Length::Px(h))),
        ..StyleRules::default()
    })
}

type Log = Rc<RefCell<Vec<String>>>;

fn logging(log: &Log, tag: &'static str) -> Rc<dyn Fn()> {
    let log = log.clone();
    Rc::new(move || log.borrow_mut().push(tag.to_string()))
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.borrow_mut())
}

#[test]
fn regression_press_down_pressable_fires_once_at_the_down_on_real_x_input() {
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

    let window = gtk4::Window::new();
    window.set_default_size(400, 300);
    // Behind the same `Rc<RefCell<_>>` it hands out as its self-reference,
    // as the real host mounts it (see `pressable_hit_test.rs`).
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let a11y = AccessibilityProps::default();
    let log: Log = Default::default();

    // root ─┬─ ancestor (release-mode pressable) ── menu (down-mode)
    //       └─ button (release-mode pressable)
    let mut root = backend.borrow_mut().create_view(&a11y);
    let mut ancestor = backend.borrow_mut().create_pressable(logging(&log, "ancestor"), &a11y);
    let menu = backend.borrow_mut().create_pressable_on_down(logging(&log, "menu"), &a11y);
    let button = backend.borrow_mut().create_pressable(logging(&log, "button"), &a11y);
    {
        let mut b = backend.borrow_mut();
        b.apply_style(&ancestor, &absolute(0.0, 0.0, 230.0, 300.0));
        b.apply_style(&menu, &absolute(60.0, 60.0, 80.0, 44.0));
        b.apply_style(&button, &absolute(260.0, 60.0, 80.0, 44.0));
        b.insert(&mut ancestor, menu.clone());
        b.insert(&mut root, ancestor.clone());
        b.insert(&mut root, button.clone());
    }
    // The menu button's interaction states, as the style system attaches
    // them: its own `GestureClick` beside the activation one.
    let menu_pressed: Rc<RefCell<Vec<bool>>> = Default::default();
    {
        let seen = menu_pressed.clone();
        backend.borrow_mut().attach_states(
            &menu,
            Rc::new(move |bit, on| {
                if bit == StateBits::PRESSED {
                    seen.borrow_mut().push(on);
                }
            }),
        );
    }
    backend.borrow_mut().finish(root.clone());
    window.set_child(Some(root.widget()));
    window.present();
    assert!(
        pump_until(|| menu.widget().is_mapped() && menu.widget().allocated_width() > 0, 3000),
        "the tree was never mapped and allocated"
    );
    settle();
    let xid = xid_of(&window);

    // Centre of `node` in X window coordinates: GTK bounds in the window,
    // shifted by where the window's content sits on its surface (CSD).
    let centre = |node: &backend_linux::LinuxNode| -> (i32, i32) {
        let b = node.widget().compute_bounds(&window).expect("bounds within the window");
        let (sx, sy) = window.surface_transform();
        (
            (sx + (b.x() + b.width() / 2.0) as f64).round() as i32,
            (sy + (b.y() + b.height() / 2.0) as f64).round() as i32,
        )
    };
    let (menu_at, button_at) = (centre(&menu), centre(&button));
    // Inside the ancestor, well below the menu button.
    let ancestor_at = {
        let b = ancestor.widget().compute_bounds(&window).expect("ancestor bounds");
        assert!(b.height() > 260.0, "the ancestor must reach below the menu button");
        let (sx, sy) = window.surface_transform();
        ((sx + b.x() as f64) as i32 + 20, (sy + b.y() as f64) as i32 + 250)
    };

    // --- 1. A primary press fires once, at the down. --------------------
    x.warp_to(xid, menu_at);
    settle();
    x.button(1, true);
    assert!(pump_until(|| !log.borrow().is_empty(), 2000), "the press-down pressable never fired");
    settle();
    assert_eq!(take(&log), ["menu"], "exactly once, and the ancestor hears nothing of the press");
    assert_eq!(*menu_pressed.borrow(), [true], "the press lights PRESSED and holds it");

    // --- 2. Its release fires nothing more, here or at the ancestor. ----
    x.button(1, false);
    settle();
    assert!(take(&log).is_empty(), "the release over the control fires nothing");
    assert_eq!(*menu_pressed.borrow(), [true, false], "the release clears PRESSED");

    // --- 3. A press dragged off onto the ancestor: still nothing more. --
    x.button(1, true);
    assert!(pump_until(|| !log.borrow().is_empty(), 2000), "second press never fired");
    x.warp_to(xid, ancestor_at);
    settle();
    x.button(1, false);
    settle();
    assert_eq!(take(&log), ["menu"], "one press, one action; the release off it reaches no ancestor");

    // --- 4. A secondary press fires nothing. ----------------------------
    x.warp_to(xid, menu_at);
    settle();
    x.button(3, true);
    x.button(3, false);
    settle();
    assert!(take(&log).is_empty(), "a right-click activates nothing");

    // --- 5. Release mode is unchanged. ----------------------------------
    x.warp_to(xid, button_at);
    settle();
    x.button(1, true);
    settle();
    assert!(take(&log).is_empty(), "a release-mode pressable fires nothing at the down");
    x.button(1, false);
    assert!(pump_until(|| !log.borrow().is_empty(), 2000), "the release-mode click never fired");
    settle();
    assert_eq!(take(&log), ["button"]);

    // The ancestor is a working release-mode pressable on its own: the
    // silence above is the down-mode claim, not a dead ancestor.
    x.warp_to(xid, ancestor_at);
    settle();
    x.button(1, true);
    x.button(1, false);
    assert!(pump_until(|| !log.borrow().is_empty(), 2000), "the ancestor never fired");
    assert_eq!(take(&log), ["ancestor"]);
}

//! App-level pointer on GTK, driven by REAL X input: the sink receives
//! button down/up (side buttons included), movement with deltas and wheel
//! steps for the whole window; `PreventDefault` keeps a press / wheel from
//! the widget under the pointer; the X11 lock reports `lock_changed`, parks
//! the pointer and turns motion into deltas; cursor hiding swaps the window
//! cursor; `None` tears everything down.
//!
//! ## The bug this pins
//!
//! `LinuxBackend` inherited the trait's no-op `set_pointer_sink` /
//! `set_pointer_lock` / `set_cursor_hidden`: a game's mouse input was dead
//! on Linux and its pointer lock silently never engaged.
//!
//! Unlike the keyboard test (which emits controller signals, because GTK4
//! cannot synthesize a `GdkKeyEvent`), this one injects input at the X
//! server with XTest + `XWarpPointer`, so the whole path is real: X server
//! → GDK's XI2 translation → GTK's capture-phase propagation → the legacy
//! controller → the sink. Needs an X display (`xvfb-run`) and `libXtst.so.6`
//! (the `idealyst-linux-gtk-test:xtst` image adds it); without either the
//! test prints SKIP and passes — grep the output.
//!
//! ONE GTK `#[test]` in this binary: GTK may only be driven from the thread
//! that ran `gtk::init`, and cargo gives every `#[test]` its own thread.

#![cfg(target_os = "linux")]

use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};
use std::rc::Rc;
use std::time::{Duration, Instant};

use backend_linux::{gtk4, LinuxBackend};
use gtk4::glib::translate::ToGlibPtr;
use gtk4::prelude::*;
use runtime_shared::pointer_input::{AppPointerEvent, MouseButton, PointerOutcome, PointerPhase, PointerSink};
use runtime_vocabulary::caps::AppEnvOps;

#[derive(Debug, Clone, PartialEq)]
enum Seen {
    Ev(AppPointerEvent),
    Lock(bool),
    FocusLost,
}

/// Minimal Xlib + XTest, resolved at runtime (libX11 is already mapped by
/// GDK; libXtst is loaded here).
struct X {
    dpy: *mut c_void,
    warp: unsafe extern "C" fn(*mut c_void, c_ulong, c_ulong, c_int, c_int, c_uint, c_uint, c_int, c_int) -> c_int,
    sync: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    #[allow(clippy::type_complexity)]
    query: unsafe extern "C" fn(
        *mut c_void,
        c_ulong,
        *mut c_ulong,
        *mut c_ulong,
        *mut c_int,
        *mut c_int,
        *mut c_int,
        *mut c_int,
        *mut c_uint,
    ) -> c_int,
    button: unsafe extern "C" fn(*mut c_void, c_uint, c_int, c_ulong) -> c_int,
    rel_motion: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_ulong) -> c_int,
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
                query: sym(x11, b"XQueryPointer\0")?,
                button: sym(xtst, b"XTestFakeButtonEvent\0")?,
                rel_motion: sym(xtst, b"XTestFakeRelativeMotionEvent\0")?,
            })
        }
    }
    fn warp_to(&self, xid: c_ulong, x: i32, y: i32) {
        unsafe {
            (self.warp)(self.dpy, 0, xid, 0, 0, 0, 0, x, y);
            (self.sync)(self.dpy, 0);
        }
    }
    fn click(&self, button: u32, press: bool) {
        unsafe {
            (self.button)(self.dpy, button, press as c_int, 0);
            (self.sync)(self.dpy, 0);
        }
    }
    fn nudge(&self, dx: i32, dy: i32) {
        unsafe {
            (self.rel_motion)(self.dpy, dx, dy, 0);
            (self.sync)(self.dpy, 0);
        }
    }
    /// Pointer position relative to `xid`.
    fn pointer_in(&self, xid: c_ulong) -> (i32, i32) {
        let (mut r, mut c) = (0, 0);
        let (mut rx, mut ry, mut wx, mut wy, mut m) = (0, 0, 0, 0, 0);
        unsafe { (self.query)(self.dpy, xid, &mut r, &mut c, &mut rx, &mut ry, &mut wx, &mut wy, &mut m) };
        (wx, wy)
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

fn cursor_name(window: &gtk4::Window) -> Option<String> {
    window.cursor().and_then(|c| c.name()).map(|n| n.to_string())
}

#[test]
fn regression_gtk_app_pointer_delivers_real_x_input_and_locks_on_x11() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    let display = gtk4::gdk::Display::default().expect("display");
    if display.type_().name() != "GdkX11Display" {
        eprintln!("SKIP: not an X11 display (pointer lock + XTest need X11)");
        return;
    }
    let Some(x) = X::open() else {
        eprintln!("SKIP: libXtst.so.6 not available");
        return;
    };

    // A window whose whole content is a box with its own press + scroll
    // handlers — stands in for "the widget under the pointer".
    let window = gtk4::Window::new();
    window.set_default_size(400, 300);
    let child = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    child.set_hexpand(true);
    child.set_vexpand(true);
    let child_presses = Rc::new(RefCell::new(Vec::<u32>::new()));
    let click = gtk4::GestureClick::new();
    click.set_button(0);
    let cp = child_presses.clone();
    click.connect_pressed(move |g, _, _, _| cp.borrow_mut().push(g.current_button()));
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

    let mut backend = LinuxBackend::new(window.clone());
    let log = Rc::new(RefCell::new(Vec::<Seen>::new()));
    let prevent_secondary_and_wheel = Rc::new(Cell::new(true));
    let sink = {
        let (l1, l2, l3) = (log.clone(), log.clone(), log.clone());
        let prevent = prevent_secondary_and_wheel.clone();
        PointerSink::new(
            move |e| {
                l1.borrow_mut().push(Seen::Ev(e.clone()));
                let claim = e.button == Some(MouseButton::Secondary) || e.phase == PointerPhase::Wheel;
                if prevent.get() && claim {
                    PointerOutcome::PreventDefault
                } else {
                    PointerOutcome::Default
                }
            },
            move |v| l2.borrow_mut().push(Seen::Lock(v)),
            move || l3.borrow_mut().push(Seen::FocusLost),
        )
    };
    let baseline_legacy = legacy_controllers(&window);
    backend.set_pointer_sink(Some(sink.clone()));
    assert_eq!(legacy_controllers(&window), baseline_legacy + 1, "Some installs one controller");

    let events = |phase: PointerPhase| -> Vec<AppPointerEvent> {
        log.borrow()
            .iter()
            .filter_map(|s| match s {
                Seen::Ev(e) if e.phase == phase => Some(e.clone()),
                _ => None,
            })
            .collect()
    };

    // --- Movement: a real pointer move inside the window. ---------------
    x.warp_to(xid, 100, 100);
    assert!(pump_until(|| !events(PointerPhase::Move).is_empty(), 2000), "no Move delivered");
    log.borrow_mut().clear();
    x.warp_to(xid, 110, 95);
    assert!(pump_until(|| !events(PointerPhase::Move).is_empty(), 2000), "no second Move");
    let mv = events(PointerPhase::Move).last().cloned().unwrap();
    assert_eq!((mv.dx, mv.dy), (10.0, -5.0), "dx/dy = movement since the previous event: {mv:?}");

    // --- Buttons: Default reaches the child; PreventDefault does not. ----
    log.borrow_mut().clear();
    x.click(1, true);
    x.click(1, false);
    assert!(pump_until(|| events(PointerPhase::Up).len() == 1, 2000), "primary up missing: {:?}", log.borrow());
    let downs = events(PointerPhase::Down);
    assert_eq!(downs.len(), 1);
    assert_eq!(downs[0].button, Some(MouseButton::Primary));
    assert_eq!((downs[0].x, downs[0].y), (mv.x, mv.y), "press at the pointer position");
    assert_eq!(*child_presses.borrow(), vec![1], "Default press reaches the widget under the pointer");

    x.click(3, true);
    x.click(3, false);
    assert!(pump_until(|| events(PointerPhase::Up).len() == 2, 2000), "secondary up missing");
    assert_eq!(events(PointerPhase::Down)[1].button, Some(MouseButton::Secondary));
    assert_eq!(*child_presses.borrow(), vec![1], "prevented secondary press never reaches the child (no context menu)");

    // Side buttons.
    x.click(8, true);
    x.click(8, false);
    x.click(9, true);
    x.click(9, false);
    assert!(pump_until(|| events(PointerPhase::Up).len() == 4, 2000), "side buttons missing: {:?}", log.borrow());
    let downs = events(PointerPhase::Down);
    assert_eq!(downs[2].button, Some(MouseButton::Back));
    assert_eq!(downs[3].button, Some(MouseButton::Forward));

    // --- Wheel: button 5 = toward the user = +dy (web sign). ------------
    log.borrow_mut().clear();
    x.click(5, true);
    x.click(5, false);
    assert!(pump_until(|| !events(PointerPhase::Wheel).is_empty(), 2000), "no Wheel: {:?}", log.borrow());
    let w = events(PointerPhase::Wheel)[0].clone();
    assert!(w.wheel_dy > 0.0 && w.wheel_dx == 0.0, "wheel toward the user scrolls content down: {w:?}");
    assert_eq!(child_scrolls.get(), 0, "prevented wheel never reaches the child");
    prevent_secondary_and_wheel.set(false);
    x.click(4, true);
    x.click(4, false);
    assert!(pump_until(|| child_scrolls.get() > 0, 2000), "Default wheel reaches the child");
    assert!(events(PointerPhase::Wheel).last().unwrap().wheel_dy < 0.0, "wheel away = -dy");

    // --- Cursor hiding. ---------------------------------------------------
    let before = cursor_name(&window);
    backend.set_cursor_hidden(true);
    assert_eq!(cursor_name(&window).as_deref(), Some("none"));
    backend.set_cursor_hidden(false);
    assert_eq!(cursor_name(&window), before, "previous cursor restored");

    // --- Pointer lock (needs the active window). ------------------------
    if !window.is_active() {
        eprintln!("SKIP(lock section): window not active under this display server");
    } else {
        log.borrow_mut().clear();
        backend.set_pointer_lock(true);
        assert!(log.borrow().is_empty(), "lock result is never reported inline (backend is borrowed then)");
        assert!(pump_until(|| log.borrow().contains(&Seen::Lock(true)), 2000), "no lock_changed(true): {:?}", log.borrow());
        assert_eq!(cursor_name(&window).as_deref(), Some("none"), "a lock hides the cursor");
        pump(100);
        let center = x.pointer_in(xid);
        log.borrow_mut().clear();

        x.nudge(7, 3);
        assert!(pump_until(|| !events(PointerPhase::Move).is_empty(), 2000), "no locked Move");
        pump(100);
        let moves = events(PointerPhase::Move);
        let (sx, sy) = moves.iter().fold((0.0, 0.0), |a, m| (a.0 + m.dx, a.1 + m.dy));
        assert!(sx > 0.0 && sy > 0.0, "locked motion reported as deltas: {moves:?}");
        assert!(moves.windows(2).all(|w| (w[0].x, w[0].y) == (w[1].x, w[1].y)), "position stays at the lock point");
        assert_eq!(x.pointer_in(xid), center, "the pointer is parked back on the lock point");

        x.nudge(-6, 0);
        pump(150);
        let total_dx: f32 = events(PointerPhase::Move).iter().map(|m| m.dx).sum();
        assert!(total_dx < sx, "leftward motion reports negative dx");

        // Release on request.
        log.borrow_mut().clear();
        backend.set_pointer_lock(false);
        assert!(pump_until(|| log.borrow().contains(&Seen::Lock(false)), 2000), "no lock_changed(false)");
        assert_eq!(cursor_name(&window), before, "cursor restored after the lock");

        // The platform drops it: another window takes focus.
        backend.set_pointer_lock(true);
        assert!(pump_until(|| log.borrow().contains(&Seen::Lock(true)), 2000));
        log.borrow_mut().clear();
        let other = gtk4::Window::new();
        other.present();
        if pump_until(|| !window.is_active(), 2000) {
            assert_eq!(
                *log.borrow(),
                vec![Seen::Lock(false), Seen::FocusLost],
                "deactivation releases the lock, then reports focus loss"
            );
        } else {
            eprintln!("SKIP(focus-loss section): focus did not move between windows");
        }
        other.destroy();
    }

    // --- Teardown. --------------------------------------------------------
    backend.set_pointer_sink(None);
    assert_eq!(legacy_controllers(&window), baseline_legacy, "None removes the controller");
    log.borrow_mut().clear();
    x.warp_to(xid, 50, 50);
    x.click(1, true);
    x.click(1, false);
    pump(200);
    assert!(log.borrow().is_empty(), "nothing delivered after None: {:?}", log.borrow());
}

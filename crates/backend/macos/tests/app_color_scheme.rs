//! The color scheme against real AppKit (`src/imp/app_state.rs`):
//! `install_app_state_observers` key-value observes
//! `NSApp.effectiveAppearance`, and each change reports the new scheme
//! (`runtime_shared::color_scheme()`) and runs the flush hook. Dropping the
//! backend removes the observer, so a later change reaches nothing.
//!
//! The appearance is changed through `NSApp.appearance`, the app's own
//! override: it moves `effectiveAppearance` the way the system switch does,
//! without touching the user's system setting.
//!
//! `harness = false`: AppKit wants the main thread.
//!
//! ```sh
//! cargo test -p backend-macos --test app_color_scheme
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::rc::Rc;

    use backend_macos::MacosBackend;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_foundation::{MainThreadMarker, NSString};
    use runtime_shared::ColorScheme;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after: u8) -> i32;
    }

    fn pump_until(mut done: impl FnMut() -> bool) -> bool {
        for _ in 0..100 {
            if done() {
                return true;
            }
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
        done()
    }

    fn set_appearance(app: &AnyObject, name: &str) {
        let name = NSString::from_str(name);
        let appearance: *mut AnyObject = unsafe { msg_send![class!(NSAppearance), appearanceNamed: &*name] };
        assert!(!appearance.is_null(), "{name} exists");
        let _: () = unsafe { msg_send![app, setAppearance: appearance] };
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "app_color_scheme must run on the main thread (harness = false)");
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let app: Retained<AnyObject> = unsafe { msg_send_id![class!(NSApplication), sharedApplication] };
        set_appearance(&app, "NSAppearanceNameAqua");

        let flushes = Rc::new(Cell::new(0usize));
        let mut backend = MacosBackend::new(mtm);
        let f = flushes.clone();
        backend.install_app_state_observers(Rc::new(move || f.set(f.get() + 1)));
        runtime_shared::backend::report_color_scheme(ColorScheme::Light);

        // REGRESSION (CrewForge): an app in Auto kept its launch theme when
        // macOS switched to dark — the scheme was read once at mount.
        let before = flushes.get();
        set_appearance(&app, "NSAppearanceNameDarkAqua");
        assert!(pump_until(|| runtime_shared::color_scheme() == ColorScheme::Dark), "dark is reported");
        assert!(flushes.get() > before, "and the flush follows");
        println!("test regression_a_switch_to_dark_is_reported ... ok");

        set_appearance(&app, "NSAppearanceNameAqua");
        assert!(pump_until(|| runtime_shared::color_scheme() == ColorScheme::Light), "and back to light");
        println!("test a_switch_back_to_light_is_reported ... ok");

        // Dropped with the backend: AppKit would otherwise message a freed
        // observer on the next change.
        drop(backend);
        let after_drop = flushes.get();
        set_appearance(&app, "NSAppearanceNameDarkAqua");
        pump_until(|| false);
        assert_eq!(runtime_shared::color_scheme(), ColorScheme::Light, "nothing observes after the drop");
        assert_eq!(flushes.get(), after_drop);
        println!("test dropping_the_backend_removes_the_observer ... ok");

        let none: *mut AnyObject = std::ptr::null_mut();
        let _: () = unsafe { msg_send![&*app, setAppearance: none] };
        println!("\ntest result: ok. 3 passed");
    }
}

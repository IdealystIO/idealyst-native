//! App situation on GTK: reduce motion follows `gtk-enable-animations`
//! live, the primary pointer comes from the seat, visibility follows
//! map/unmap, focus follows `is-active` — and `uninstall` disconnects.
//!
//! ## The bug this pins
//!
//! The GTK backend reported nothing, so `reduce_motion()` stayed `false`
//! for a user who turned animations off, and `app_visible()` stayed `true`
//! for a hidden window — a game could not auto-pause.
//!
//! ONE GTK `#[test]` in this binary (GTK is single-threaded; cargo runs each
//! `#[test]` on its own thread).

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use backend_linux::app_situation::AppSituation;
use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_shared::app_state::{self, AppStateChange, PointerKind};

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

#[test]
fn regression_gtk_reports_reduce_motion_visibility_and_pointer_kind() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    app_state::reset_for_tests();
    let settings = gtk4::Settings::default().expect("settings");
    settings.set_gtk_enable_animations(true);

    let window = gtk4::Window::new();
    let (situation, boot) = AppSituation::install(&window);
    // Unmapped: focus / visibility are not guessed at boot.
    assert!(boot.contains(&AppStateChange::ReduceMotion(false)), "{boot:?}");
    assert!(boot.iter().any(|c| matches!(c, AppStateChange::PrimaryPointer(_))), "{boot:?}");
    assert!(!boot.iter().any(|c| matches!(c, AppStateChange::Visible(_) | AppStateChange::Focused(_))));
    for c in boot {
        app_state::report(c);
    }
    // Xvfb's seat has a core pointer.
    assert_eq!(app_state::current().primary_pointer, PointerKind::Fine);

    // Reduce motion, live.
    settings.set_gtk_enable_animations(false);
    assert!(app_state::current().reduce_motion, "animations off → reduce motion");
    settings.set_gtk_enable_animations(true);
    assert!(!app_state::current().reduce_motion);

    // Visibility: hide → false, present → true.
    window.present();
    assert!(pump_until(|| window.is_mapped(), 3000));
    assert!(app_state::current().visible);
    window.set_visible(false);
    assert!(pump_until(|| !app_state::current().visible, 2000), "unmap → Visible(false)");
    window.present();
    assert!(pump_until(|| app_state::current().visible, 2000), "map → Visible(true)");
    if pump_until(|| window.is_active(), 1000) {
        assert!(app_state::current().focused, "active window → Focused(true)");
    } else {
        eprintln!("SKIP(focus section): window never became active");
    }

    // Uninstall: later changes are not reported.
    situation.uninstall();
    settings.set_gtk_enable_animations(false);
    assert!(!app_state::current().reduce_motion, "uninstall disconnects the settings observer");
    settings.set_gtk_enable_animations(true);
}

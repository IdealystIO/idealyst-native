//! Keep the **display on**, at its normal brightness, while some part of the
//! app needs it: a kiosk, a recipe, a navigation screen, a dashboard.
//!
//! ```ignore
//! // For as long as the guard lives.
//! let awake = keep_awake::keep_screen_on();
//!
//! // For as long as the component is mounted and the condition holds.
//! keep_awake::use_keep_screen_on(rx!(shift_open.get()));
//! ```
//!
//! - **Counted.** The display stays on until the last hold is dropped.
//! - **Only while the app is on screen.** A hold has no effect while the app
//!   is in the background, minimized or in a hidden tab, and applies again
//!   when it returns ([`runtime_shared::app_state`]'s `visible`).
//! - **Not a lock.** It stops the idle timeout. The power button, the lid and
//!   the lock shortcut work as ever.
//! - **Best-effort.** A platform that can refuse (a browser on low battery)
//!   refuses silently; [`is_supported`] says whether there is anything to ask.
//!
//! Call it on the UI thread, where the app's visibility is known; the guard
//! is not `Send`.

#![deny(missing_docs)]

mod labels;
mod policy;

/// Compile-checked usage recipes (docs / MCP catalog).
#[cfg(feature = "catalog")]
pub mod recipes;

pub use labels::{install_keep_awake_labels, keep_awake_labels, KeepAwakeLabels};

use std::cell::RefCell;
use std::marker::PhantomData;

use policy::Policy;
use runtime_core::{effect, Reactive};

#[cfg(target_os = "android")]
#[path = "android.rs"]
mod backend;
#[cfg(target_os = "ios")]
#[path = "ios.rs"]
mod backend;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod backend;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod backend;
#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod backend;
#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod backend;
#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_arch = "wasm32"
)))]
#[path = "unsupported.rs"]
mod backend;

runtime_tls::thread_local! {
    static POLICY: RefCell<Option<Policy>> = const { RefCell::new(None) };
}

/// Run `f` on this thread's policy, then tell the platform what it decided.
/// `dyn` because it passes itself a closure.
fn update(f: &dyn Fn(&mut Policy)) {
    let first = POLICY.with(|p| p.borrow().is_none());
    if first {
        let visible = runtime_shared::app_state::current().visible;
        POLICY.with(|p| *p.borrow_mut() = Some(Policy::new(visible)));
        runtime_shared::app_state::observe(|state| {
            let visible = state.visible;
            update(&move |p| p.set_visible(visible));
        });
    }
    let Some(engaged) = POLICY.with(|p| {
        let mut p = p.borrow_mut();
        let p = p.as_mut().expect("installed above");
        f(p);
        p.take_call()
    }) else {
        return;
    };
    // Outside the borrow: the platform call may re-enter (a synchronous
    // focus or visibility report).
    backend::set(engaged);
}

/// A hold on the display, from [`keep_screen_on`]. Dropping it releases the
/// hold; the display's normal timeout returns with the last one.
#[must_use = "dropping a KeepAwake releases it: keep it for as long as the display must stay on"]
#[derive(Debug)]
pub struct KeepAwake {
    /// `false` where a server renders: there is no display to keep on.
    counted: bool,
    /// The count and the app's visibility live on the UI thread.
    _ui_thread: PhantomData<*const ()>,
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        if self.counted {
            update(&Policy::release);
        }
    }
}

/// Keep the display on until the returned guard is dropped.
///
/// Holds are counted, and have no effect while the app is off screen. Where
/// [`is_supported`] is `false` this does nothing.
pub fn keep_screen_on() -> KeepAwake {
    let counted = !renders_on_a_server();
    if counted {
        update(&Policy::hold);
    }
    KeepAwake { counted, _ui_thread: PhantomData }
}

/// Keep the display on while `on` is true and the calling component's scope
/// is alive. `on` is a `bool`, a signal, a memo or an `rx!`.
///
/// Call it in a component body, like an effect.
pub fn use_keep_screen_on(on: impl Into<Reactive<bool>>) {
    let on = on.into();
    // Owned by the effect, so the scope's teardown drops it.
    let hold: RefCell<Option<KeepAwake>> = RefCell::new(None);
    effect!({
        let on = on.get();
        if hold.borrow().is_some() != on {
            hold.replace(on.then(keep_screen_on));
        }
    });
}

/// Whether this platform can keep the display on at all. `false` means every
/// hold does nothing.
///
/// On Linux this asks the session bus the first time it is called and can
/// take up to a second when the desktop is slow to answer.
pub fn is_supported() -> bool {
    !renders_on_a_server() && backend::is_supported()
}

/// A native process rendering for the web is a server: the display that
/// shows the page is not its own.
fn renders_on_a_server() -> bool {
    cfg!(not(target_arch = "wasm32")) && runtime_shared::host::platform() == runtime_shared::host::Platform::Web
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use host_mock::Harness;
    use runtime_core::{signal, ui, Element, Signal};
    use runtime_shared::app_state::{report, AppStateChange};
    use std::cell::Cell;
    use std::rc::Rc;

    fn holds() -> usize {
        POLICY.with(|p| p.borrow().as_ref().map_or(0, Policy::holds))
    }

    fn engaged() -> bool {
        POLICY.with(|p| p.borrow().as_ref().is_some_and(Policy::told))
    }

    #[runtime_core::component]
    fn Awake(on: Signal<bool>) -> Element {
        use_keep_screen_on(on);
        ui! { view {} }
    }

    #[test]
    fn guards_count_and_follow_visibility() {
        runtime_shared::app_state::reset_for_tests();
        let a = keep_screen_on();
        let b = keep_screen_on();
        assert!(engaged());
        drop(a);
        assert!(engaged(), "one hold left");
        report(AppStateChange::Visible(false));
        assert!(!engaged(), "off screen");
        report(AppStateChange::Visible(true));
        assert!(engaged(), "back on screen");
        drop(b);
        assert!(!engaged());
        assert_eq!(holds(), 0);
    }

    #[test]
    fn a_server_render_holds_nothing() {
        runtime_shared::backend::install_current_platform(runtime_shared::host::Platform::Web);
        assert!(!is_supported());
        let held = keep_screen_on();
        assert_eq!(holds(), 0);
        drop(held);
        assert_eq!(holds(), 0);
    }

    /// The scope-owned form follows its condition, and unmounting releases
    /// it without the condition turning false.
    #[test]
    fn the_scope_owns_the_hold() {
        host_mock::pump::install_scheduler();
        let h = Harness::new();
        let out: Rc<Cell<Option<(Signal<bool>, Signal<bool>)>>> = Rc::default();
        let tree = {
            let out = out.clone();
            h.world.enter(move || {
                let (shown, on) = (signal(true), signal(false));
                out.set(Some((shown, on)));
                ui! {
                    view {
                        if shown.get() {
                            Awake(on = on)
                        }
                    }
                }
            })
        };
        let _realized = h.mount(tree);
        h.flush();
        let (shown, on) = out.get().expect("built");
        let set = |s: Signal<bool>, v: bool| {
            h.world.enter(|| s.set(v));
            h.flush();
        };
        assert_eq!(holds(), 0, "the condition is false");
        set(on, true);
        assert_eq!(holds(), 1);
        set(on, false);
        assert_eq!(holds(), 0);
        set(on, true);
        set(shown, false);
        assert_eq!(holds(), 0, "unmounted");
    }
}

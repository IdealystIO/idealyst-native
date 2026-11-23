//! `keep-awake-demo` — two guards, a component-scoped hold and one kept
//! outside the UI, each on its own button, to try the `keep-awake` SDK on a
//! device.

use std::cell::RefCell;
use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme, Button, Stack, StackGap, StackPadding, Typography};
use keep_awake::{is_supported, keep_screen_on, use_keep_screen_on, KeepAwake};
use runtime_core::{component, rx, signal, ui, Element, Signal};

/// SDK-handler registration seam. `keep-awake` is a capability crate, not a
/// rendered primitive, so there is nothing to register.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}

/// Recorder-side seam for the runtime-server sidecar.
#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(_registry: &mut dev_server::newcore::SceneRegistry) {}

/// Android entry: the generated wrapper mounts `scene_app()`.
pub fn scene_app() -> Element {
    app()
}

thread_local! {
    /// A hold the UI does not own: Android may recreate the Activity, which
    /// mounts the app again, and this one is still held.
    static OUTSIDE: RefCell<Option<KeepAwake>> = const { RefCell::new(None) };
}

/// A button that takes a guard and drops it.
#[component]
fn GuardButton(name: String, test_id: &'static str) -> Element {
    let held = signal(false);
    let guard: Rc<RefCell<Option<KeepAwake>>> = Rc::default();
    let toggle: Rc<dyn Fn()> = Rc::new(move || {
        let on = !held.peek();
        guard.replace(on.then(keep_screen_on));
        held.set(on);
    });
    ui! {
        Button(
            label = rx!(format!("{}: {}", name.get(), if held.get() { "held" } else { "free" })),
            on_click = toggle,
            test_id = Some(test_id),
        )
    }
}

/// Holds while it is mounted.
#[component]
fn Scoped() -> Element {
    use_keep_screen_on(true);
    ui! { Typography(content = "This component keeps the screen on.".to_string()) }
}

/// The demo screen.
pub fn app() -> Element {
    install_idea_theme(light_theme());
    let mounted: Signal<bool> = signal(false);
    let toggle: Rc<dyn Fn()> = Rc::new(move || mounted.set(!mounted.peek()));
    let outside = signal(OUTSIDE.with(|o| o.borrow().is_some()));
    let toggle_outside: Rc<dyn Fn()> = Rc::new(move || {
        let on = !outside.peek();
        OUTSIDE.with(|o| o.replace(on.then(keep_screen_on)));
        outside.set(on);
    });
    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Xl) {
            Typography(content = format!("Supported: {}", is_supported()))
            GuardButton(name = "Guard A".to_string(), test_id = "guard-a")
            GuardButton(name = "Guard B".to_string(), test_id = "guard-b")
            Button(
                label = rx!(if mounted.get() { "Unmount the component" } else { "Mount the component" }.to_string()),
                on_click = toggle,
                test_id = Some("scoped"),
            )
            Button(
                label = rx!(format!("Outside the UI: {}", if outside.get() { "held" } else { "free" })),
                on_click = toggle_outside,
                test_id = Some("outside"),
            )
            if mounted.get() {
                Scoped()
            }
        }
    }
}

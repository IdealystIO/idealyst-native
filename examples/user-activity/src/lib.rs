//! `user-activity` — a kiosk-style idle clock driven by `user_activity()`.
//!
//! The header counts down from [`IDLE_SECONDS`] and restarts on every
//! interaction anywhere in the app: tapping the button (which keeps its
//! touch), dragging the scroll view, typing in the field, or touching
//! anything inside the overlay — none of which a parent `on_touch` sees.
//! The app adds no handler of its own for any of that; the one effect
//! below reads `user_activity()` and nothing else.

use runtime_core::{
    after_ms_scoped, signal, stylesheet, ui, user_activity, Color, Element, SafeAreaSides,
};

/// How long the idle clock runs before it would "return to the keypad".
const IDLE_SECONDS: u32 = 10;
/// Countdown granularity.
const TICK_MS: i32 = 1_000;

stylesheet! {
    pub Screen<()> {
        base(_t) {
            flex_grow: 1.0,
            padding: 16,
            gap: 12,
            background: Color("#f7f8fb".into()),
        }
    }
}

stylesheet! {
    pub Field<()> {
        base(_t) {
            padding: 10,
            border_width: 1.0,
            border_color: Color("#9ca3af".into()),
            border_radius: 8,
        }
    }
}

stylesheet! {
    pub Card<()> {
        base(_t) {
            padding: 24,
            gap: 12,
            margin: 40,
            border_radius: 12,
            background: Color("#ffffff".into()),
        }
    }
}

pub fn app() -> Element {
    let activity = user_activity();
    let remaining = signal(IDLE_SECONDS);
    let draft = signal(String::new());
    let presses = signal(0u32);
    let dialog_open = signal(false);

    // Every tick of `user_activity()` restarts the idle clock.
    runtime_core::effect!({
        let n = activity.get();
        remaining.set(IDLE_SECONDS);
        runtime_core::logging::log(runtime_core::logging::LogLevel::Info, &format!("[user-activity] tick {n}"));
    });
    // One second off per tick. The chain starts in the component body
    // (its scope anchors the timer) and re-arms from inside the callback,
    // which re-enters that same anchor; it dies with the screen.
    fn count_down(remaining: runtime_core::Signal<u32>) {
        after_ms_scoped(TICK_MS, move || {
            remaining.set(remaining.get().saturating_sub(1));
            count_down(remaining);
        });
    }
    count_down(remaining);

    ui! {
        view(style = Screen(), safe_area = SafeAreaSides::TOP) {
            text { move || format!("Idle in {}s · activity {}", remaining.get(), activity.get()) }
            button(label = "Press me".to_string(), on_click = move || presses.set(presses.get() + 1))
            text { move || format!("Pressed {}", presses.get()) }
            button(label = "Open dialog".to_string(), on_click = move || dialog_open.set(true))
            text_input(value = draft, on_change = move |s| draft.set(s), placeholder = "Type here", style = Field())
            scroll_view {
                view {
                    for i in 1..=60 {
                        text { format!("Row {i}") }
                    }
                }
            }
            if dialog_open.get() {
                overlay {
                    view(style = Card()) {
                        text { "A dialog in its own portal" }
                        button(label = "Close".to_string(), on_click = move || dialog_open.set(false))
                    }
                }
            }
        }
    }
}

/// The Android wrapper's entry (see `examples/README.md`).
pub fn scene_app() -> Element {
    app()
}

/// SDK-handler registration seam. This app registers nothing.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}

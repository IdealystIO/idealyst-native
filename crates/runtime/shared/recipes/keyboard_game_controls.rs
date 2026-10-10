/// Keyboard controls for a game: move a player with WASD / arrow keys
/// while they're HELD, and jump on Space. Two complementary APIs:
///
/// - `key_state()` returns a handle you POLL from a frame loop —
///   `is_down` / `any_down` take physical key codes (`"KeyA"`,
///   `"ArrowLeft"`, `"Space"`), so WASD stays in place on any keyboard
///   layout and a key released after Shift lets go still reads as up.
/// - `on_key` delivers every key DOWN and UP as an event; `e.is_press()`
///   is "down and not an auto-repeat". Returning `PreventDefault` stops
///   the platform default (Space scrolling the page on web).
///
/// Both live exactly as long as the component (`raf_loop_scoped` too),
/// and held keys are released automatically when the window loses
/// focus, so nothing stays stuck after an alt-tab.
pub fn keyboard_game_controls() -> ::runtime_core::Element {
    use ::runtime_core::{key_state, on_key, raf_loop_scoped, signal, ui, KeyOutcome};

    let x = signal(0.0_f32);
    let jumps = signal(0u32);

    // Polling: held keys move the player every frame.
    let keys = key_state();
    raf_loop_scoped(move || {
        if keys.any_down(&["KeyA", "ArrowLeft"]) {
            x.update(|v| v - 2.0);
        }
        if keys.any_down(&["KeyD", "ArrowRight"]) {
            x.update(|v| v + 2.0);
        }
    });

    // Events: one jump per press (auto-repeat ignored).
    on_key(move |e| {
        if e.code == "Space" {
            if e.is_press() {
                jumps.update(|n| n + 1);
            }
            return KeyOutcome::PreventDefault;
        }
        KeyOutcome::Default
    });

    ui! {
        view {
            text { "x = {x}" }
            text { "jumps = {jumps}" }
        }
    }
}

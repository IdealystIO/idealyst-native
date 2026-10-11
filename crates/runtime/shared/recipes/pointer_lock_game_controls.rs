/// Mouse controls for a first-person game: click to lock the pointer,
/// turn with mouse movement, fire with the left button, switch weapons
/// with the wheel, and pause automatically when the window loses focus.
///
/// - `request_pointer_lock()` must be called from a click or key handler
///   (browsers refuse it otherwise). `pointer_locked()` is a signal: it
///   flips back to `false` when the user presses Esc or alt-tabs, so the
///   game shows its "click to resume" overlay from that alone.
/// - `pointer_state()` is polled from the frame loop. `take_movement()`
///   returns the mouse motion since the last frame — while locked that is
///   the only motion there is (the cursor doesn't move). Each handle keeps
///   its own accumulator.
/// - `on_pointer` sees buttons anywhere in the app; returning
///   `PreventDefault` for the right button suppresses the context menu.
/// - `app_focused()` drives auto-pause.
pub fn pointer_lock_game_controls() -> ::runtime_core::Element {
    use ::runtime_core::{
        app_focused, effect, on_pointer, pointer_locked, pointer_state, raf_loop_scoped,
        request_pointer_lock, signal, ui, MouseButton, PointerOutcome, PointerPhase,
    };

    let yaw = signal(0.0_f32);
    let weapon = signal(0i32);
    let shots = signal(0u32);
    let paused = signal(false);

    let locked = pointer_locked();
    let focused = app_focused();
    // Auto-pause when the window loses focus or the lock is dropped.
    effect(move || paused.set(!focused.get() || !locked.get()));

    let mouse = pointer_state();
    raf_loop_scoped(move || {
        let (dx, _dy) = mouse.take_movement();
        if !paused.get() {
            yaw.update(|y| y + dx * 0.1);
        }
    });

    on_pointer(move |e| {
        if !locked.get() {
            return PointerOutcome::Default;
        }
        match (e.phase, e.button) {
            (PointerPhase::Down, Some(MouseButton::Primary)) => {
                shots.update(|n| n + 1);
                PointerOutcome::PreventDefault
            }
            (PointerPhase::Down, Some(MouseButton::Secondary)) => PointerOutcome::PreventDefault,
            (PointerPhase::Wheel, _) => {
                weapon.update(|w| (w + e.wheel_dy.signum() as i32).rem_euclid(3));
                PointerOutcome::PreventDefault
            }
            _ => PointerOutcome::Default,
        }
    });

    ui! {
        view {
            if locked.get() {
                text { "yaw {yaw}  weapon {weapon}  shots {shots}" }
            } else {
                button(label = "Click to play", on_click = move || request_pointer_lock())
            }
        }
    }
}

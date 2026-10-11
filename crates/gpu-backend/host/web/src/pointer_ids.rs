//! DOM pointer → engine pointer identity. Un-gated so it tests on the host.

use render_api::PointerId;

/// The engine id for a DOM pointer. A mouse becomes [`PointerId::MOUSE`]
/// — the id every desktop shell (winit) uses for the mouse, and the one
/// the engine routes to the app-level pointer API (`on_pointer`,
/// `pointer_state`, pointer lock). Touch and pen keep the browser's
/// `pointerId`, offset by one so no finger can ever collide with
/// `MOUSE` (0).
///
/// Before this, every DOM pointer kept its raw `pointerId`; Chrome gives
/// the mouse `pointerId` 1, so on the GPU-in-browser host mouse events
/// never reached app-level pointer listeners.
pub(crate) fn engine_pointer_id(pointer_type: &str, dom_pointer_id: i32) -> PointerId {
    if pointer_type == "mouse" {
        PointerId::MOUSE
    } else {
        PointerId(dom_pointer_id as u64 + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_gpu_web_mouse_maps_to_engine_mouse_id() {
        assert_eq!(engine_pointer_id("mouse", 1), PointerId::MOUSE);
        // A touch whose DOM id happens to be 0 must not alias the mouse.
        assert_ne!(engine_pointer_id("touch", 0), PointerId::MOUSE);
        assert_ne!(engine_pointer_id("pen", 0), PointerId::MOUSE);
        assert_ne!(engine_pointer_id("touch", 2), engine_pointer_id("touch", 3));
    }
}

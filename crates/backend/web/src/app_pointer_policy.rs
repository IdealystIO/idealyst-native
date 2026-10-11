//! Pure translation rules for the app-level pointer source
//! (`primitives::app_pointer`) and the app-state observers
//! (`app_state_source`): DOM numbers in, framework values out. No DOM
//! access here, so every rule is checked directly by the tests at the
//! bottom (which run in the browser test binary only because this crate
//! compiles for `wasm32` alone).

use runtime_shared::app_state::PointerKind;
use runtime_shared::pointer_input::MouseButton;

/// Pixels per wheel LINE (`WheelEvent.deltaMode == 1`). Firefox reports
/// line deltas for real mouse wheels; 16 px is one line of default body
/// text (the CSS initial `font-size`) and what Chrome itself scrolls per
/// line, so a wheel "click" moves the same distance on every browser.
pub(crate) const WHEEL_LINE_PX: f64 = 16.0;

/// `MouseEvent.button` → the framework button. The DOM numbering is
/// 0 primary, 1 auxiliary (middle), 2 secondary, 3 back, 4 forward;
/// anything else (exotic mice report 5+) is not a framework button.
pub(crate) fn mouse_button(dom_button: i16) -> Option<MouseButton> {
    match dom_button {
        0 => Some(MouseButton::Primary),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Secondary),
        3 => Some(MouseButton::Back),
        4 => Some(MouseButton::Forward),
        _ => None,
    }
}

/// A wheel delta in logical pixels. `delta_mode` is the DOM's
/// `WheelEvent.deltaMode` (0 pixel, 1 line, 2 page); `page_px` is the
/// height of one page (the viewport height). The sign is kept: positive
/// `deltaY` already means "content scrolls down" on the web, which is the
/// framework's convention.
pub(crate) fn wheel_px(delta: f64, delta_mode: u32, page_px: f64) -> f32 {
    let scale = match delta_mode {
        1 => WHEEL_LINE_PX,
        2 => page_px,
        _ => 1.0,
    };
    (delta * scale) as f32
}

/// The primary pointer from the CSS `(pointer: fine)` / `(pointer:
/// coarse)` media features. A device with both (a touch laptop) reports
/// the PRIMARY one as matching — `fine` wins when both somehow match.
/// Neither matching is `(pointer: none)`.
pub(crate) fn primary_pointer(fine: bool, coarse: bool) -> PointerKind {
    if fine {
        PointerKind::Fine
    } else if coarse {
        PointerKind::Coarse
    } else {
        PointerKind::None
    }
}

/// `document.visibilityState` → visible. Only `"hidden"` is hidden;
/// `"visible"` and the legacy `"prerender"` count as on screen.
pub(crate) fn is_visible(visibility_state: &str) -> bool {
    visibility_state != "hidden"
}

/// Whether a DOM `pointerType` is a finger touch — those belong to the
/// view-level touch APIs, not the app-level pointer source.
pub(crate) fn is_touch(pointer_type: &str) -> bool {
    pointer_type == "touch"
}

/// Whether a prevented down must also cancel the browser's
/// `contextmenu` that follows it: a secondary press, or a ctrl+primary
/// press (macOS turns ctrl-click into a context-menu click and fires
/// `contextmenu` for it with `button == 0`).
pub(crate) fn down_opens_context_menu(button: MouseButton, ctrl: bool) -> bool {
    button == MouseButton::Secondary || (button == MouseButton::Primary && ctrl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn dom_buttons_map_to_framework_buttons() {
        assert_eq!(mouse_button(0), Some(MouseButton::Primary));
        assert_eq!(mouse_button(1), Some(MouseButton::Middle));
        assert_eq!(mouse_button(2), Some(MouseButton::Secondary));
        assert_eq!(mouse_button(3), Some(MouseButton::Back));
        assert_eq!(mouse_button(4), Some(MouseButton::Forward));
        assert_eq!(mouse_button(5), None);
        assert_eq!(mouse_button(-1), None);
    }

    #[wasm_bindgen_test]
    fn wheel_units_convert_to_pixels_and_keep_sign() {
        assert_eq!(wheel_px(53.0, 0, 800.0), 53.0);
        assert_eq!(wheel_px(3.0, 1, 800.0), 48.0);
        assert_eq!(wheel_px(-1.0, 1, 800.0), -16.0);
        assert_eq!(wheel_px(1.0, 2, 800.0), 800.0);
        assert_eq!(wheel_px(-0.5, 2, 600.0), -300.0);
    }

    #[wasm_bindgen_test]
    fn pointer_media_features_pick_primary_pointer() {
        assert_eq!(primary_pointer(true, false), PointerKind::Fine);
        assert_eq!(primary_pointer(false, true), PointerKind::Coarse);
        assert_eq!(primary_pointer(false, false), PointerKind::None);
        assert_eq!(primary_pointer(true, true), PointerKind::Fine);
    }

    #[wasm_bindgen_test]
    fn only_hidden_visibility_state_is_hidden() {
        assert!(is_visible("visible"));
        assert!(is_visible("prerender"));
        assert!(!is_visible("hidden"));
    }

    #[wasm_bindgen_test]
    fn touch_pointer_type_is_filtered() {
        assert!(is_touch("touch"));
        assert!(!is_touch("mouse"));
        assert!(!is_touch("pen"));
        assert!(!is_touch(""));
    }

    #[wasm_bindgen_test]
    fn secondary_and_ctrl_click_open_context_menu() {
        assert!(down_opens_context_menu(MouseButton::Secondary, false));
        assert!(down_opens_context_menu(MouseButton::Primary, true));
        assert!(!down_opens_context_menu(MouseButton::Primary, false));
        assert!(!down_opens_context_menu(MouseButton::Middle, true));
    }
}

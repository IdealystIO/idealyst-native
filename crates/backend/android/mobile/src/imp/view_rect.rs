//! `view_screen_rect` lives here (not in `backend-android-core`)
//! because it needs the mobile crate's `with_env` / `JAVA_VM` state
//! — those are tied to `JNI_OnLoad`, which is a single per-cdylib
//! symbol owned by this crate.
//!
//! The signature is preserved (no `JNIEnv` parameter) so callers
//! under `imp::primitives::*` don't have to thread an env through.

use jni::objects::{JObject, JValue};

/// Read a View's screen-relative bounding rect, in physical pixels.
/// Origin is the top-left of the device screen, including the
/// status bar (the same coordinate space `PopupWindow.showAtLocation`
/// uses).
///
/// Returns the zero rect if the view has no width/height yet (not
/// laid out) — which gives overlay positioning code a sensible
/// fallback (it'll center on the viewport instead of anchoring to
/// nowhere).
///
/// Synchronous JNI calls. Cheap enough to call once per overlay
/// open; not suitable for per-frame use. (`getLocationOnScreen`
/// internally walks the view ancestry.)
pub(crate) fn view_screen_rect(
    node: &jni::objects::GlobalRef,
) -> framework_core::primitives::overlay::ViewportRect {
    super::with_env(|env| {
        let Ok(loc) = env.new_int_array(2) else {
            return framework_core::primitives::overlay::ViewportRect::default();
        };
        let loc_obj: &JObject = loc.as_ref();
        if env
            .call_method(
                node.as_obj(),
                "getLocationOnScreen",
                "([I)V",
                &[JValue::Object(loc_obj)],
            )
            .is_err()
        {
            return framework_core::primitives::overlay::ViewportRect::default();
        }
        let mut buf = [0i32; 2];
        if env.get_int_array_region(&loc, 0, &mut buf).is_err() {
            return framework_core::primitives::overlay::ViewportRect::default();
        }
        let width = env
            .call_method(node.as_obj(), "getWidth", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        let height = env
            .call_method(node.as_obj(), "getHeight", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        framework_core::primitives::overlay::ViewportRect {
            x: buf[0] as f32,
            y: buf[1] as f32,
            width: width as f32,
            height: height as f32,
        }
    })
}

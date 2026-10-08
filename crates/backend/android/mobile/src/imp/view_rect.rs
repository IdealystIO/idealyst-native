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
/// status bar (the coordinate space `adb shell input tap` uses —
/// `device_frame`). Anchoring uses [`view_viewport_rect`] instead.
///
/// Returns the zero rect if the view has no width/height yet (not
/// laid out).
///
/// Synchronous JNI calls. Cheap enough to call once per overlay
/// open; not suitable for per-frame use. (`getLocationOnScreen`
/// internally walks the view ancestry.)
pub(crate) fn view_screen_rect(
    node: &jni::objects::GlobalRef,
) -> runtime_shared::primitives::portal::ViewportRect {
    super::with_env(|env| {
        let Ok(loc) = env.new_int_array(2) else {
            return runtime_shared::primitives::portal::ViewportRect::default();
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
            return runtime_shared::primitives::portal::ViewportRect::default();
        }
        let mut buf = [0i32; 2];
        if env.get_int_array_region(&loc, 0, &mut buf).is_err() {
            return runtime_shared::primitives::portal::ViewportRect::default();
        }
        let width = env
            .call_method(node.as_obj(), "getWidth", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        let height = env
            .call_method(node.as_obj(), "getHeight", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0);
        runtime_shared::primitives::portal::ViewportRect {
            x: buf[0] as f32,
            y: buf[1] as f32,
            width: width as f32,
            height: height as f32,
        }
    })
}

thread_local! {
    /// The Activity root the app tree and every portal overlay mount in —
    /// the "viewport" of `ViewportRect`. Set by `AndroidBackend::new`; read
    /// by [`view_viewport_rect`], which runs inside handle ops (`rect()`)
    /// that have no backend reference and may run while the backend is
    /// mutably borrowed (the layout pass places anchored portals).
    static VIEWPORT_ROOT: std::cell::RefCell<Option<jni::objects::GlobalRef>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn set_viewport_root(root: &jni::objects::GlobalRef) {
    VIEWPORT_ROOT.with(|r| *r.borrow_mut() = Some(root.clone()));
}

/// A view's rect in viewport space: dp, origin at the Activity root's
/// top-left — the `ViewportRect` contract `AnchorableHandle::rect` has on
/// every backend, and the space the anchored portal's overlay frames live
/// in (it fills the root). Zero rect when the view isn't laid out (the
/// "not measured" sentinel the anchored placer skips).
pub(crate) fn view_viewport_rect(
    node: &jni::objects::GlobalRef,
) -> runtime_shared::primitives::portal::ViewportRect {
    let view = view_screen_rect(node);
    if view.width <= 0.0 && view.height <= 0.0 {
        return runtime_shared::primitives::portal::ViewportRect::default();
    }
    let Some(root) = VIEWPORT_ROOT.with(|r| r.borrow().clone()) else {
        return runtime_shared::primitives::portal::ViewportRect::default();
    };
    let root_rect = view_screen_rect(&root);
    let density = super::with_env(|env| super::density_of(env, &root.as_obj())).unwrap_or(1.0);
    crate::anchored_portal_policy::screen_px_to_viewport_dp(view, (root_rect.x, root_rect.y), density)
}

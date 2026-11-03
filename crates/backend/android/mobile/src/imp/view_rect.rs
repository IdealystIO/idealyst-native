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

runtime_tls::thread_local! {
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


/// Opacity at or below which a view counts as invisible — the painted
/// backends' hit-test bar (`effective_opacity() <= 0.003`).
const TRANSPARENT_ALPHA: f32 = 0.003;

/// Whether Android shows `node` (`IntrospectionOps::is_shown`):
/// `View.isShown()` — it and every ancestor `VISIBLE`, and attached to a
/// window — and not faded out (the product of `getAlpha()` up the parent
/// chain above ~0). `None` if the JNI calls fail.
pub(crate) fn view_is_shown(node: &jni::objects::GlobalRef) -> Option<bool> {
    super::with_env(|env| {
        let shown = env.call_method(node.as_obj(), "isShown", "()Z", &[]).and_then(|v| v.z()).ok()?;
        if !shown {
            return Some(false);
        }
        let view_class = env.find_class("android/view/View").ok()?;
        let mut alpha = 1.0f32;
        let mut cur = env.new_local_ref(node.as_obj()).ok()?;
        loop {
            alpha *= env.call_method(&cur, "getAlpha", "()F", &[]).and_then(|v| v.f()).ok()?;
            let parent = env.call_method(&cur, "getParent", "()Landroid/view/ViewParent;", &[]).and_then(|v| v.l()).ok()?;
            if parent.is_null() || !env.is_instance_of(&parent, &view_class).ok()? {
                break;
            }
            let _ = env.delete_local_ref(cur);
            cur = parent;
        }
        Some(alpha > TRANSPARENT_ALPHA)
    })
}

/// `IntrospectionOps::content_clip`: whether the view can scroll now and,
/// for a `ViewGroup`, `getClipChildren()`, decided by
/// `crate::content_clip_policy`.
pub(crate) fn content_clip(node: &jni::objects::GlobalRef) -> Option<runtime_vocabulary::caps::ContentClip> {
    super::with_env(|env| {
        let mut can_scroll = false;
        for (method, dir) in [("canScrollVertically", 1), ("canScrollVertically", -1), ("canScrollHorizontally", 1), ("canScrollHorizontally", -1)] {
            can_scroll |= env.call_method(node.as_obj(), method, "(I)Z", &[JValue::Int(dir)]).and_then(|v| v.z()).ok()?;
            if can_scroll {
                break;
            }
        }
        let group = env.find_class("android/view/ViewGroup").ok()?;
        let clip_children = if env.is_instance_of(node.as_obj(), &group).ok()? {
            Some(env.call_method(node.as_obj(), "getClipChildren", "()Z", &[]).and_then(|v| v.z()).ok()?)
        } else {
            None
        };
        Some(crate::content_clip_policy::content_clip(can_scroll, clip_children))
    })
}

runtime_tls::thread_local! {
    /// The `downTime` of the gesture whose `ACTION_DOWN` was injected and
    /// not yet lifted, carried to the later injections of the same gesture.
    static GESTURE_DOWN_TIME: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// `IntrospectionOps::scroll_port` on Android: any view that can scroll
/// now (`canScrollVertically` / `canScrollHorizontally`, the test
/// `content_clip` makes) — a `ScrollView`, a `RecyclerView`. Offset and
/// range from `View.computeVertical/HorizontalScroll{Offset,Range,Extent}`,
/// which every scrolling view overrides (they are `protected`, which JNI
/// does not enforce), in px → dp. A window does not scroll: no `Host`.
pub(crate) fn scroll_port(
    node: &jni::objects::GlobalRef,
    target: runtime_vocabulary::caps::ScrollTarget,
) -> Option<runtime_vocabulary::caps::ScrollPort> {
    if target != runtime_vocabulary::caps::ScrollTarget::Node {
        return None;
    }
    if content_clip(node)? != runtime_vocabulary::caps::ContentClip::Scroll {
        return None;
    }
    super::with_env(|env| {
        let mut int = |m: &str| env.call_method(node.as_obj(), m, "()I", &[]).and_then(|v| v.i()).ok();
        let (x, y) = (int("computeHorizontalScrollOffset")?, int("computeVerticalScrollOffset")?);
        let max_x = int("computeHorizontalScrollRange")? - int("computeHorizontalScrollExtent")?;
        let max_y = int("computeVerticalScrollRange")? - int("computeVerticalScrollExtent")?;
        let density = super::density_of(env, node.as_obj()).unwrap_or(1.0);
        Some(runtime_vocabulary::caps::ScrollPort {
            x: x as f32 / density,
            y: y as f32 / density,
            max_x: max_x.max(0) as f32 / density,
            max_y: max_y.max(0) as f32 / density,
            port: None,
        })
    })
}

/// `IntrospectionOps::set_scroll_offset` on Android: `scrollBy` the
/// difference — a `RecyclerView` refuses an absolute `scrollTo`, and
/// `scrollBy` is what both it and a `ScrollView` honor. An injection,
/// since a scroll runs the view's scroll listeners (`on_scroll`).
pub(crate) fn set_scroll_offset(
    node: &jni::objects::GlobalRef,
    target: runtime_vocabulary::caps::ScrollTarget,
    x: f32,
    y: f32,
) -> Option<runtime_vocabulary::caps::NativeInjection> {
    scroll_port(node, target)?;
    let node = node.clone();
    Some(Box::new(move || {
        let now = scroll_port(&node, target).ok_or("the view no longer scrolls")?;
        super::with_env(|env| {
            let density = super::density_of(env, node.as_obj()).unwrap_or(1.0);
            let dx = ((x - now.x) * density).round() as i32;
            let dy = ((y - now.y) * density).round() as i32;
            env.call_method(node.as_obj(), "scrollBy", "(II)V", &[JValue::Int(dx), JValue::Int(dy)])
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    }))
}

/// `IntrospectionOps::keyboard_kind`: an `EditText`'s `inputType`, read back.
pub(crate) fn keyboard_kind(
    node: &jni::objects::GlobalRef,
) -> Option<runtime_shared::primitives::text_input::KeyboardKind> {
    super::primitives::text_input::read_keyboard_kind(node)
}

/// `IntrospectionOps::native_pointer`: each step as a `MotionEvent`
/// (`ACTION_DOWN` / `ACTION_MOVE` / `ACTION_UP`, touchscreen source) handed
/// to the decor view's `dispatchTouchEvent` — the entry the window's input
/// stage calls for a real finger, so the view tree's own dispatch, hit
/// testing (`onInterceptTouchEvent`, clickable views, disabled state) and
/// gesture detectors all run. Delivered synchronously on the UI thread.
/// Points are viewport dp (the `absolute_frame` space); the conversion
/// reads the root's position and density when the injection runs.
pub(crate) fn native_pointer(
    steps: &[runtime_vocabulary::caps::PointerStep],
) -> Option<runtime_vocabulary::caps::NativeInjection> {
    use runtime_vocabulary::caps::PointerStepPhase;
    // A finger does not hover (`PointerStepPhase::Hover`) and has no
    // secondary button (`SecondaryClick`): the robot takes those through
    // its hit-test tier instead.
    if steps.iter().any(|s| matches!(s.phase, PointerStepPhase::Hover | PointerStepPhase::SecondaryClick)) {
        return None;
    }
    let root = VIEWPORT_ROOT.with(|r| r.borrow().clone())?;
    let steps = steps.to_vec();
    Some(Box::new(move || {
        let root_rect = view_screen_rect(&root);
        let decor = super::with_env(|env| -> Result<jni::objects::GlobalRef, String> {
            let v = env
                .call_method(root.as_obj(), "getRootView", "()Landroid/view/View;", &[])
                .and_then(|v| v.l());
            match v {
                Ok(v) if !v.is_null() => env.new_global_ref(v).map_err(|e| e.to_string()),
                Ok(_) => Err("the app root has no root view".into()),
                Err(e) => {
                    let _ = env.exception_clear();
                    Err(format!("getRootView: {e}"))
                }
            }
        })?;
        let decor_rect = view_screen_rect(&decor);
        let density = super::with_env(|env| super::density_of(env, &root.as_obj())).unwrap_or(1.0);
        super::with_env(|env| {
            let fail = |env: &mut jni::JNIEnv, what: &str, e: jni::errors::Error| {
                let _ = env.exception_clear();
                format!("{what}: {e}")
            };
            let now = |env: &mut jni::JNIEnv| {
                env.call_static_method("android/os/SystemClock", "uptimeMillis", "()J", &[]).and_then(|v| v.j())
            };
            // One gesture keeps one `downTime` across injections: a held
            // press (`press_native`, then real time, then `release_native`)
            // lifts with the down's time, as a real finger's events carry
            // it — what `GestureDetector`'s long-press check reads.
            let mut down_time = GESTURE_DOWN_TIME.with(|d| d.get());
            for s in steps {
                let action = match s.phase {
                    PointerStepPhase::Down => 0,
                    PointerStepPhase::Up => 1,
                    PointerStepPhase::Move => 2,
                    PointerStepPhase::Hover | PointerStepPhase::SecondaryClick => unreachable!("refused above"),
                };
                if action == 0 || down_time.is_none() {
                    down_time = Some(now(env).map_err(|e| fail(env, "uptimeMillis", e))?);
                }
                let down = down_time.expect("set above");
                GESTURE_DOWN_TIME.with(|d| d.set(if action == 1 { None } else { Some(down) }));
                let (x, y) = crate::anchored_portal_policy::viewport_dp_to_view_px(
                    (s.x, s.y),
                    (root_rect.x, root_rect.y),
                    (decor_rect.x, decor_rect.y),
                    density,
                );
                let t = now(env).map_err(|e| fail(env, "uptimeMillis", e))?;
                let ev = env
                    .call_static_method(
                        "android/view/MotionEvent",
                        "obtain",
                        "(JJIFFI)Landroid/view/MotionEvent;",
                        &[
                            JValue::Long(down),
                            JValue::Long(t),
                            JValue::Int(action),
                            JValue::Float(x),
                            JValue::Float(y),
                            JValue::Int(0),
                        ],
                    )
                    .and_then(|v| v.l())
                    .map_err(|e| fail(env, "MotionEvent.obtain", e))?;
                // InputDevice.SOURCE_TOUCHSCREEN: a finger, not a mouse.
                let _ = env.call_method(&ev, "setSource", "(I)V", &[JValue::Int(0x1002)]);
                let dispatched = env.call_method(
                    decor.as_obj(),
                    "dispatchTouchEvent",
                    "(Landroid/view/MotionEvent;)Z",
                    &[JValue::Object(&ev)],
                );
                let _ = env.call_method(&ev, "recycle", "()V", &[]);
                let _ = env.delete_local_ref(ev);
                dispatched.map_err(|e| fail(env, "dispatchTouchEvent", e))?;
            }
            Ok(())
        })
    }))
}

//! `IntrospectionOps::native_key`: the robot's key press as the `KeyEvent`s
//! a hardware keyboard delivers, handed to the window's decor view
//! `dispatchKeyEvent` — the entry the input stage calls for a real key. The
//! activity's own dispatch runs from there: the focused view's
//! `OnKeyListener` (a field's `on_key_down`, a modal overlay's back
//! handling), the `EditText`'s editing (characters, Backspace, arrows via
//! its key listener and the virtual keyboard's character map), the root's
//! app-level key listener, and the activity's unhandled-key fallbacks.
//! Delivered synchronously on the UI thread.
//!
//! ## Tab moves focus
//!
//! A real key reaches the decor view through `ViewRootImpl`'s input stage,
//! which — when the view tree leaves a Tab down unhandled — runs its focus
//! navigation: `findFocus()`, `focusSearch(FOCUS_FORWARD / FOCUS_BACKWARD)`
//! and `requestFocus(direction)` on the result. Calling `dispatchKeyEvent`
//! enters below that stage, so the injection runs the same navigation
//! itself for an unhandled Tab, through the same public calls: the
//! platform's `FocusFinder` picks the next view, a multi-line `EditText`
//! still takes the Tab as text (it handles the key), and a field's
//! `on_key_down` that claims it stops the move.

use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;
use runtime_vocabulary::caps::{KeyPress, NativeInjection};

use crate::native_key_policy::{plan, tab_focus_direction, KeyPlan};

// KeyEvent.ACTION_* / InputDevice.SOURCE_KEYBOARD.
const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;
const SOURCE_KEYBOARD: i32 = 0x101;
/// `KeyCharacterMap.VIRTUAL_KEYBOARD`: the device whose character map turns
/// keycode + meta state into text (what `adb shell input` uses too).
const VIRTUAL_KEYBOARD: i32 = -1;

pub(crate) fn native_key(node: &GlobalRef, press: &KeyPress) -> Option<NativeInjection> {
    let plan = plan(press)?;
    let node = node.clone();
    Some(Box::new(move || {
        super::with_env(|env| {
            let decor = decor_of(env, &node)?;
            let out = deliver(env, &decor, &plan);
            let _ = env.delete_local_ref(decor);
            out
        })
    }))
}

fn fail(env: &mut JNIEnv, what: &str, e: jni::errors::Error) -> String {
    // A thrown Java exception left pending poisons every later JNI call.
    let _ = env.exception_clear();
    format!("{what}: {e}")
}

/// The window's decor view: `node.getRootView()`.
fn decor_of<'a>(env: &mut JNIEnv<'a>, node: &GlobalRef) -> Result<JObject<'a>, String> {
    match env.call_method(node.as_obj(), "getRootView", "()Landroid/view/View;", &[]).and_then(|v| v.l()) {
        Ok(v) if !v.is_null() => Ok(v),
        Ok(_) => Err("the element has no root view (not attached to a window)".into()),
        Err(e) => Err(fail(env, "getRootView", e)),
    }
}

fn deliver(env: &mut JNIEnv, decor: &JObject, plan: &KeyPlan) -> Result<(), String> {
    let now = env
        .call_static_method("android/os/SystemClock", "uptimeMillis", "()J", &[])
        .and_then(|v| v.j())
        .map_err(|e| fail(env, "uptimeMillis", e))?;
    match plan {
        KeyPlan::Key { key_code, meta_state } => {
            for action in [ACTION_DOWN, ACTION_UP] {
                // KeyEvent(downTime, eventTime, action, code, repeat, metaState,
                //          deviceId, scancode, flags, source)
                let ev = env
                    .new_object(
                        "android/view/KeyEvent",
                        "(JJIIIIIIII)V",
                        &[
                            JValue::Long(now),
                            JValue::Long(now),
                            JValue::Int(action),
                            JValue::Int(*key_code),
                            JValue::Int(0),
                            JValue::Int(*meta_state),
                            JValue::Int(VIRTUAL_KEYBOARD),
                            JValue::Int(0),
                            JValue::Int(0),
                            JValue::Int(SOURCE_KEYBOARD),
                        ],
                    )
                    .map_err(|e| fail(env, "new KeyEvent", e))?;
                let handled = dispatch(env, decor, ev)?;
                if action == ACTION_DOWN && !handled {
                    if let Some(direction) = tab_focus_direction(*key_code, *meta_state) {
                        navigate_focus(env, decor, direction)?;
                    }
                }
            }
        }
        KeyPlan::Characters(text) => {
            let chars = env.new_string(text).map_err(|e| fail(env, "NewString", e))?;
            // KeyEvent(time, characters, deviceId, flags): ACTION_MULTIPLE,
            // KEYCODE_UNKNOWN — committed text with no key behind it.
            let ev = env
                .new_object(
                    "android/view/KeyEvent",
                    "(JLjava/lang/String;II)V",
                    &[JValue::Long(now), JValue::Object(&chars), JValue::Int(VIRTUAL_KEYBOARD), JValue::Int(0)],
                )
                .map_err(|e| fail(env, "new KeyEvent(characters)", e));
            let _ = env.delete_local_ref(chars);
            dispatch(env, decor, ev?)?;
        }
    }
    Ok(())
}

/// Whether the view tree handled the event.
fn dispatch(env: &mut JNIEnv, decor: &JObject, ev: JObject) -> Result<bool, String> {
    let out = env.call_method(decor, "dispatchKeyEvent", "(Landroid/view/KeyEvent;)Z", &[JValue::Object(&ev)]);
    let _ = env.delete_local_ref(ev);
    out.and_then(|v| v.z()).map_err(|e| fail(env, "dispatchKeyEvent", e))
}

/// `ViewRootImpl.performFocusNavigation` for an unhandled Tab (module docs,
/// "Tab moves focus"): from the focused view, the next one in `direction`
/// takes focus; with nothing focused, the window's default focus.
fn navigate_focus(env: &mut JNIEnv, decor: &JObject, direction: i32) -> Result<(), String> {
    let focused = env
        .call_method(decor, "findFocus", "()Landroid/view/View;", &[])
        .and_then(|v| v.l())
        .map_err(|e| fail(env, "findFocus", e))?;
    if focused.is_null() {
        env.call_method(decor, "restoreDefaultFocus", "()Z", &[]).map_err(|e| fail(env, "restoreDefaultFocus", e))?;
        return Ok(());
    }
    let next = env
        .call_method(&focused, "focusSearch", "(I)Landroid/view/View;", &[JValue::Int(direction)])
        .and_then(|v| v.l())
        .map_err(|e| fail(env, "focusSearch", e))?;
    let same = !next.is_null() && env.is_same_object(&next, &focused).map_err(|e| fail(env, "IsSameObject", e))?;
    if !next.is_null() && !same {
        env.call_method(&next, "requestFocus", "(I)Z", &[JValue::Int(direction)])
            .map_err(|e| fail(env, "requestFocus", e))?;
    }
    let _ = env.delete_local_ref(next);
    let _ = env.delete_local_ref(focused);
    Ok(())
}

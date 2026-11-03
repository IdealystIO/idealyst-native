//! `IntrospectionOps::native_key`: the robot's key press as the `KeyEvent`s
//! a hardware keyboard delivers, handed to the window's decor view
//! `dispatchKeyEvent` — the entry the input stage calls for a real key. The
//! activity's own dispatch runs from there: the focused view's
//! `OnKeyListener` (a field's `on_key_down`, a modal overlay's back
//! handling), the `EditText`'s editing (characters, Backspace, arrows via
//! its key listener and the virtual keyboard's character map), the root's
//! app-level key listener, and the activity's unhandled-key fallbacks.
//! Delivered synchronously on the UI thread.

use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;
use runtime_vocabulary::caps::{KeyPress, NativeInjection};

use crate::native_key_policy::{plan, KeyPlan};

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
                dispatch(env, decor, ev)?;
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

fn dispatch(env: &mut JNIEnv, decor: &JObject, ev: JObject) -> Result<(), String> {
    let out = env.call_method(decor, "dispatchKeyEvent", "(Landroid/view/KeyEvent;)Z", &[JValue::Object(&ev)]);
    let _ = env.delete_local_ref(ev);
    out.map(|_| ()).map_err(|e| fail(env, "dispatchKeyEvent", e))
}

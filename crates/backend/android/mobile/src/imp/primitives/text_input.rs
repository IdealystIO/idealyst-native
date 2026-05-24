//! `Primitive::TextInput` and `Primitive::TextArea` — both backed by
//! `android.widget.EditText`. TextArea calls [`create_multiline`]
//! which sets `inputType` to `TYPE_CLASS_TEXT |
//! TYPE_TEXT_FLAG_MULTI_LINE`; otherwise the wiring is identical
//! (same `RustTextWatcher` for change, same `RustKeyListener` for
//! keydown). Keeping both primitives in one module mirrors that
//! shared substrate.

use crate::imp::callbacks::{leak, KeyDownCallback, TextChangeCallback};
use backend_android_core::helpers::{apply_default_layout_params, set_text};
use crate::imp::{with_env, AndroidBackend};
use runtime_core::primitives::text_area::{TextAreaHandle, TextAreaOps};
use runtime_core::primitives::text_input::{TextInputHandle, TextInputOps};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::sys::jlong;
use std::any::Any;
use std::rc::Rc;

pub(crate) fn create(
    b: &AndroidBackend,
    initial_value: &str,
    placeholder: Option<&str>,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
) -> GlobalRef {
    create_inner(b, initial_value, placeholder, on_change, on_key_down, false)
}

/// Multi-line variant — same widget, multiline input-type flag set.
/// `Primitive::TextArea` on Android materialises through here.
pub(crate) fn create_multiline(
    b: &AndroidBackend,
    initial_value: &str,
    placeholder: Option<&str>,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
) -> GlobalRef {
    create_inner(b, initial_value, placeholder, on_change, on_key_down, true)
}

fn create_inner(
    b: &AndroidBackend,
    initial_value: &str,
    placeholder: Option<&str>,
    on_change: Rc<dyn Fn(String)>,
    on_key_down: Option<runtime_core::primitives::key::KeyDownHandler>,
    multiline: bool,
) -> GlobalRef {
    // EditText with a TextWatcher dispatched through Kotlin
    // `RustTextWatcher`. Same lifecycle/leak pattern as
    // RustClickListener: box + leak the on_change closure. The native
    // widget calls back into `Java_io_idealyst_runtime_RustTextWatcher_nativeChanged`
    // on every keystroke.
    with_env(|env| {
        let class = env.find_class("android/widget/EditText").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        set_text(env, &local, initial_value);
        if multiline {
            // InputType bits: TYPE_CLASS_TEXT (1) |
            // TYPE_TEXT_FLAG_MULTI_LINE (0x00020000) = 0x00020001.
            // The framework's `Primitive::TextArea` contract is
            // "newlines are content, Enter inserts \n"; that's
            // exactly what the multiline flag delivers.
            let _ = env.call_method(
                &local,
                "setInputType",
                "(I)V",
                &[JValue::Int(0x00020001)],
            );
            // Default gravity for an EditText with multi-line text
            // is bottom-aligned (carryover from single-line layout);
            // flip to top so text grows downward like a textarea.
            // Gravity.TOP|START = 0x30|0x800003 = 0x00800033.
            let _ = env.call_method(
                &local,
                "setGravity",
                "(I)V",
                &[JValue::Int(0x00800033)],
            );
        }
        if let Some(p) = placeholder {
            let java_str = env.new_string(p).unwrap();
            let _ = env.call_method(
                &local,
                "setHint",
                "(Ljava/lang/CharSequence;)V",
                &[JValue::Object(&JObject::from(java_str))],
            );
        }
        // Wire the watcher.
        let ptr: jlong = leak(TextChangeCallback(on_change));
        let watcher_class = env
            .find_class("io/idealyst/runtime/RustTextWatcher")
            .unwrap();
        let watcher = env
            .new_object(&watcher_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        let _ = env.call_method(
            &local,
            "addTextChangedListener",
            "(Landroid/text/TextWatcher;)V",
            &[JValue::Object(&watcher)],
        );
        // Stash the watcher on the EditText's tag so update_value
        // can retrieve it and flip `suppress` for programmatic
        // setText calls. See `update_value` below.
        let _ = env.call_method(
            &local,
            "setTag",
            "(Ljava/lang/Object;)V",
            &[JValue::Object(&watcher)],
        );
        // Wire the key listener. Same leak-pointer pattern as the
        // text watcher: box + leak the handler, instantiate
        // `RustKeyListener(ptr)`, and call `setOnKeyListener`. The
        // Kotlin listener dispatches into Rust via `nativeKey` (see
        // `jni_exports::Java_io_idealyst_runtime_RustKeyListener_nativeKey`).
        if let Some(handler) = on_key_down {
            let key_ptr: jlong = leak(KeyDownCallback(handler));
            let listener_class = env
                .find_class("io/idealyst/runtime/RustKeyListener")
                .unwrap();
            let listener = env
                .new_object(&listener_class, "(J)V", &[JValue::Long(key_ptr)])
                .unwrap();
            let _ = env.call_method(
                &local,
                "setOnKeyListener",
                "(Landroid/view/View$OnKeyListener;)V",
                &[JValue::Object(&listener)],
            );
        }
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

/// Apply a programmatic text value to the EditText. Suppresses
/// `RustTextWatcher` during the `setText` call so runtime-server-driven wire
/// replays don't echo back to the server as an `EventOccurred` and
/// create a feedback loop (see `RustTextWatcher.suppress` for the
/// loop shape).
///
/// Same-string short-circuit is retained to avoid cursor jumps when
/// the framework re-fires an effect that wrote the same value back.
pub(crate) fn update_value(node: &GlobalRef, value: &str) {
    with_env(|env| {
        // Only update if the text differs, to avoid cursor jumps when
        // our own listener wrote back to the signal.
        let current = env
            .call_method(node.as_obj(), "getText", "()Landroid/text/Editable;", &[])
            .ok()
            .and_then(|v| v.l().ok());
        let same = current
            .as_ref()
            .map(|cur| {
                env.call_method(cur, "toString", "()Ljava/lang/String;", &[])
                    .ok()
                    .and_then(|v| v.l().ok())
                    .and_then(|s| {
                        let jstr: jni::objects::JString = s.into();
                        env.get_string(&jstr)
                            .ok()
                            .map(|js| js.to_str().unwrap_or("").to_string())
                    })
                    .map(|s| s == value)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if same {
            return;
        }
        let tag = env
            .call_method(node.as_obj(), "getTag", "()Ljava/lang/Object;", &[])
            .ok()
            .and_then(|v| v.l().ok());
        if let Some(ref watcher) = tag {
            if !watcher.is_null() {
                let _ = env.set_field(watcher, "suppress", "Z", JValue::Bool(1));
            }
        }
        set_text(env, &node.as_obj(), value);
        if let Some(ref watcher) = tag {
            if !watcher.is_null() {
                let _ = env.set_field(watcher, "suppress", "Z", JValue::Bool(0));
            }
        }
    });
}

// =============================================================================
// Handles + Ops — imperative surface exposed via `Ref<TextInputHandle>` /
// `Ref<TextAreaHandle>`. Same widget (EditText) drives both, so the impls
// share the underlying JNI helpers.
// =============================================================================

pub(crate) fn make_text_input_handle(node: &GlobalRef) -> TextInputHandle {
    TextInputHandle::new(Rc::new(node.clone()), &ANDROID_TEXT_INPUT_OPS)
}

pub(crate) fn make_text_area_handle(node: &GlobalRef) -> TextAreaHandle {
    TextAreaHandle::new(Rc::new(node.clone()), &ANDROID_TEXT_AREA_OPS)
}

/// `requestFocus` + force-show the soft keyboard via
/// `InputMethodManager.SHOW_FORCED` (legacy but the most reliable
/// signal — system honors it from arbitrary contexts; the modern
/// SHOW_IMPLICIT is a hint that can be vetoed by recent focus state).
fn focus_edit_text(node: &GlobalRef) {
    with_env(|env| {
        let _ = env.call_method(node.as_obj(), "requestFocus", "()Z", &[]);
        let context = env
            .call_method(
                node.as_obj(),
                "getContext",
                "()Landroid/content/Context;",
                &[],
            )
            .ok()
            .and_then(|v| v.l().ok());
        let Some(context) = context else { return; };
        let imm_class_name = env.new_string("input_method").unwrap();
        let imm = env
            .call_method(
                &context,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&JObject::from(imm_class_name))],
            )
            .ok()
            .and_then(|v| v.l().ok());
        if let Some(imm) = imm {
            // SHOW_IMPLICIT = 1
            let _ = env.call_method(
                &imm,
                "showSoftInput",
                "(Landroid/view/View;I)Z",
                &[JValue::Object(node.as_obj()), JValue::Int(1)],
            );
        }
    });
}

fn blur_edit_text(node: &GlobalRef) {
    with_env(|env| {
        let _ = env.call_method(node.as_obj(), "clearFocus", "()V", &[]);
        let context = env
            .call_method(
                node.as_obj(),
                "getContext",
                "()Landroid/content/Context;",
                &[],
            )
            .ok()
            .and_then(|v| v.l().ok());
        let Some(context) = context else { return; };
        let imm_class_name = env.new_string("input_method").unwrap();
        let imm = env
            .call_method(
                &context,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&JObject::from(imm_class_name))],
            )
            .ok()
            .and_then(|v| v.l().ok());
        if let Some(imm) = imm {
            let token = env
                .call_method(
                    node.as_obj(),
                    "getWindowToken",
                    "()Landroid/os/IBinder;",
                    &[],
                )
                .ok()
                .and_then(|v| v.l().ok());
            if let Some(token) = token {
                let _ = env.call_method(
                    &imm,
                    "hideSoftInputFromWindow",
                    "(Landroid/os/IBinder;I)Z",
                    &[JValue::Object(&token), JValue::Int(0)],
                );
            }
        }
    });
}

fn select_all_edit_text(node: &GlobalRef) {
    with_env(|env| {
        let _ = env.call_method(node.as_obj(), "selectAll", "()V", &[]);
    });
}

/// Splice `text` into the EditText's current selection. Mirrors the
/// web `setRangeText` path: replace [selStart, selEnd) with `text`,
/// then place caret at selStart + text.len(). The framework's
/// `RustTextWatcher` fires its `afterTextChanged` callback as a
/// side-effect of `getText().replace(...)`, so the controlling
/// `Signal` updates without us touching `setText` directly.
fn insert_text_edit_text(node: &GlobalRef, text: &str) {
    with_env(|env| {
        let start = env
            .call_method(node.as_obj(), "getSelectionStart", "()I", &[])
            .ok()
            .and_then(|v| v.i().ok())
            .unwrap_or(0);
        let end = env
            .call_method(node.as_obj(), "getSelectionEnd", "()I", &[])
            .ok()
            .and_then(|v| v.i().ok())
            .unwrap_or(start);
        // Editable.replace(int start, int end, CharSequence text).
        let editable = env
            .call_method(node.as_obj(), "getText", "()Landroid/text/Editable;", &[])
            .ok()
            .and_then(|v| v.l().ok());
        let Some(editable) = editable else { return; };
        let java_str = env.new_string(text).unwrap();
        let _ = env.call_method(
            &editable,
            "replace",
            "(IILjava/lang/CharSequence;)Landroid/text/Editable;",
            &[
                JValue::Int(start),
                JValue::Int(end),
                JValue::Object(&JObject::from(java_str)),
            ],
        );
        // Caret to end of inserted text. `setSelection(int)` is the
        // single-index variant — places the caret with no range.
        let new_caret = start + text.chars().count() as i32;
        let _ = env.call_method(
            node.as_obj(),
            "setSelection",
            "(I)V",
            &[JValue::Int(new_caret)],
        );
    });
}

pub(crate) struct AndroidTextInputOps;
impl TextInputOps for AndroidTextInputOps {
    fn focus(&self, node: &dyn Any) {
        if let Some(g) = node.downcast_ref::<GlobalRef>() {
            focus_edit_text(g);
        }
    }
    fn blur(&self, node: &dyn Any) {
        if let Some(g) = node.downcast_ref::<GlobalRef>() {
            blur_edit_text(g);
        }
    }
    fn select_all(&self, node: &dyn Any) {
        if let Some(g) = node.downcast_ref::<GlobalRef>() {
            select_all_edit_text(g);
        }
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        if let Some(g) = node.downcast_ref::<GlobalRef>() {
            insert_text_edit_text(g, text);
        }
    }
}
pub(crate) static ANDROID_TEXT_INPUT_OPS: AndroidTextInputOps = AndroidTextInputOps;

pub(crate) struct AndroidTextAreaOps;
impl TextAreaOps for AndroidTextAreaOps {
    fn focus(&self, node: &dyn Any) {
        if let Some(g) = node.downcast_ref::<GlobalRef>() {
            focus_edit_text(g);
        }
    }
    fn blur(&self, node: &dyn Any) {
        if let Some(g) = node.downcast_ref::<GlobalRef>() {
            blur_edit_text(g);
        }
    }
    fn select_all(&self, node: &dyn Any) {
        if let Some(g) = node.downcast_ref::<GlobalRef>() {
            select_all_edit_text(g);
        }
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        if let Some(g) = node.downcast_ref::<GlobalRef>() {
            insert_text_edit_text(g, text);
        }
    }
}
pub(crate) static ANDROID_TEXT_AREA_OPS: AndroidTextAreaOps = AndroidTextAreaOps;

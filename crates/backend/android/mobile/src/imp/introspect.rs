//! Platform-native render introspection for the Android backend.
//!
//! Reads the **live `android.view.View`** over JNI — its alpha, visibility and
//! background drawable, the `RustBorderDrawable` on its foreground, a
//! `TextView`'s text, color and typeface — normalized to the canonical
//! [`runtime_shared::introspect`] schema. Nothing here comes from the
//! framework's `StyleRules`, so a value a style dropped and the widget kept
//! shows up.
//!
//! `font_family` is present only where the platform names it
//! (`Typeface.getSystemFontFamilyName`, API 34, system faces); `font_weight`
//! needs API 28. An absent key means the platform doesn't say.

use std::collections::HashSet;

use jni::objects::{GlobalRef, JFloatArray, JObject, JString, JValue, JValueOwned};
use jni::JNIEnv;
use runtime_shared::introspect::{collect_native_tree, keys, NativeNode, NativeRect, NativeValue};

use super::view_rect::view_viewport_rect;
use super::{density_of, with_env, AndroidBackend};
use crate::native_readback_policy::{argb_to_rgba, border_keyword, corner_radius_dp};

/// Local references one view's read holds: drawable, color state list, typeface,
/// text, and the classes they are tested against.
const LOCAL_FRAME_CAPACITY: i32 = 32;

impl AndroidBackend {
    /// Read the native render tree for `node`. `None` until the view is
    /// attached: its geometry is window-relative, and an element that isn't
    /// there yet has none to report.
    pub(crate) fn introspect_native_impl(&self, node: &GlobalRef) -> Option<NativeNode> {
        let attached = with_env(|env| call(env, node.as_obj(), "isAttachedToWindow", "()Z", &[])?.z().ok());
        if attached != Some(true) {
            return None;
        }
        let roots = self.framework_root_identities();
        Some(collect_native_tree(node, &read_view, &children, &|view| {
            with_env(|env| identity_hash(env, view.as_obj())).is_some_and(|hash| roots.contains(&hash))
        }))
    }

    /// `System.identityHashCode` of every registered primitive root. A descendant
    /// in here is a separate framework element with its own read. Identity, not
    /// `view_to_layout`'s key: that key is the address of one global reference,
    /// and a view reached by walking the tree comes back as a different reference.
    fn framework_root_identities(&self) -> HashSet<i32> {
        with_env(|env| {
            self.view_to_layout
                .values()
                .filter_map(|(view, _)| identity_hash(env, view.as_obj()))
                .collect()
        })
    }
}

fn identity_hash(env: &mut JNIEnv, view: &JObject) -> Option<i32> {
    let hash = env.call_static_method(
        "java/lang/System",
        "identityHashCode",
        "(Ljava/lang/Object;)I",
        &[JValue::Object(view)],
    );
    settled(env, hash)?.i().ok()
}

/// A call's value, with the exception a missing method or an older API level
/// leaves pending cleared: the next JNI call would abort the process.
fn settled<'l>(env: &mut JNIEnv, result: jni::errors::Result<JValueOwned<'l>>) -> Option<JValueOwned<'l>> {
    match result {
        Ok(v) => Some(v),
        Err(_) => {
            let _ = env.exception_clear();
            None
        }
    }
}

fn call<'l>(
    env: &mut JNIEnv<'l>,
    obj: &JObject,
    name: &str,
    sig: &str,
    args: &[JValue],
) -> Option<JValueOwned<'l>> {
    let result = env.call_method(obj, name, sig, args);
    settled(env, result)
}

fn is_instance(env: &mut JNIEnv, obj: &JObject, class: &str) -> bool {
    let result = env.is_instance_of(obj, class);
    match result {
        Ok(is) => is,
        Err(_) => {
            let _ = env.exception_clear();
            false
        }
    }
}

fn java_string(env: &mut JNIEnv, s: JObject) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let s = JString::from(s);
    let out = env.get_string(&s).ok().map(String::from);
    if out.is_none() {
        let _ = env.exception_clear();
    }
    out
}

/// A `ViewGroup`'s direct children; empty for any other view.
fn children(view: &GlobalRef) -> Vec<GlobalRef> {
    with_env(|env| {
        if !is_instance(env, view.as_obj(), "android/view/ViewGroup") {
            return Vec::new();
        }
        let count = call(env, view.as_obj(), "getChildCount", "()I", &[]).and_then(|v| v.i().ok()).unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let child = call(env, view.as_obj(), "getChildAt", "(I)Landroid/view/View;", &[JValue::Int(i)])?.l().ok()?;
                (!child.is_null()).then(|| env.new_global_ref(child).ok()).flatten()
            })
            .collect()
    })
}

/// Shallow read of one view.
fn read_view(view: &GlobalRef) -> NativeNode {
    let rect = view_viewport_rect(view);
    let frame = NativeRect { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
    with_env(|env| {
        let read = env.with_local_frame(LOCAL_FRAME_CAPACITY, |env| -> jni::errors::Result<NativeNode> {
            Ok(read_view_in_frame(env, view.as_obj(), frame))
        });
        read.unwrap_or_else(|_| {
            let _ = env.exception_clear();
            NativeNode::leaf("android.view.View", frame)
        })
    })
}

fn read_view_in_frame(env: &mut JNIEnv, view: &JObject, frame: NativeRect) -> NativeNode {
    let class = class_name(env, view).unwrap_or_else(|| "android.view.View".to_string());
    let mut node = NativeNode::leaf(class, frame);

    // `View.VISIBLE` is 0; `INVISIBLE` and `GONE` both draw nothing.
    let visibility = call(env, view, "getVisibility", "()I", &[]).and_then(|v| v.i().ok());
    node.set(keys::HIDDEN, visibility.map(|v| NativeValue::Flag(v != 0)));
    let alpha = call(env, view, "getAlpha", "()F", &[]).and_then(|v| v.f().ok());
    node.set(keys::OPACITY, alpha.map(NativeValue::Number));

    let density = density_of(env, view).unwrap_or(1.0);
    let translation = call(env, view, "getTranslationY", "()F", &[]).and_then(|v| v.f().ok());
    node.set(keys::TRANSLATION_Y, translation.map(|px| NativeValue::Length(px / density)));
    read_background(env, view, density, &mut node);
    read_border(env, view, density, &mut node);
    if is_instance(env, view, "android/widget/TextView") {
        node.role = Some("text".to_string());
        read_text(env, view, density, &mut node);
    }
    node
}

fn class_name(env: &mut JNIEnv, view: &JObject) -> Option<String> {
    let class = call(env, view, "getClass", "()Ljava/lang/Class;", &[])?.l().ok()?;
    let name = call(env, &class, "getName", "()Ljava/lang/String;", &[])?.l().ok()?;
    java_string(env, name)
}

/// `setBackgroundColor` installs a `ColorDrawable`; the style's border / radius /
/// gradient path a `GradientDrawable`. A widget's own background (an `EditText`'s
/// underline, a button's ripple) has no single color, so it reports none.
fn read_background(env: &mut JNIEnv, view: &JObject, density: f32, node: &mut NativeNode) {
    let Some(background) = call(env, view, "getBackground", "()Landroid/graphics/drawable/Drawable;", &[])
        .and_then(|v| v.l().ok())
        .filter(|d| !d.is_null())
    else {
        return;
    };
    if is_instance(env, &background, "android/graphics/drawable/ColorDrawable") {
        let color = call(env, &background, "getColor", "()I", &[]).and_then(|v| v.i().ok());
        node.set(keys::BACKGROUND_COLOR, color.map(|c| NativeValue::Color(argb_to_rgba(c))));
    } else if is_instance(env, &background, "android/graphics/drawable/GradientDrawable") {
        // `getColor` is null while the fill is a gradient.
        let states = call(env, &background, "getColor", "()Landroid/content/res/ColorStateList;", &[])
            .and_then(|v| v.l().ok())
            .filter(|c| !c.is_null());
        if let Some(states) = states {
            let color = call(env, &states, "getDefaultColor", "()I", &[]).and_then(|v| v.i().ok());
            node.set(keys::BACKGROUND_COLOR, color.map(|c| NativeValue::Color(argb_to_rgba(c))));
        }
        let radii = call(env, &background, "getCornerRadii", "()[F", &[])
            .and_then(|v| v.l().ok())
            .filter(|r| !r.is_null());
        if let Some(radii) = radii {
            let radii = JFloatArray::from(radii);
            let mut buf = [0f32; 8];
            if env.get_float_array_region(&radii, 0, &mut buf).is_ok() {
                node.set(keys::CORNER_RADIUS, corner_radius_dp(&buf, density).map(NativeValue::Length));
            } else {
                let _ = env.exception_clear();
            }
        }
    }
}

/// The top side of the `RustBorderDrawable` on the view's foreground: a single
/// canonical key can't carry four sides, and every reader answers for the top.
fn read_border(env: &mut JNIEnv, view: &JObject, density: f32, node: &mut NativeNode) {
    let Some(foreground) = call(env, view, "getForeground", "()Landroid/graphics/drawable/Drawable;", &[])
        .and_then(|v| v.l().ok())
        .filter(|d| !d.is_null())
    else {
        return;
    };
    if !is_instance(env, &foreground, "io/idealyst/runtime/RustBorderDrawable") {
        return;
    }
    let width = call(env, &foreground, "topWidthPx", "()I", &[]).and_then(|v| v.i().ok()).unwrap_or(0);
    if width <= 0 {
        return;
    }
    node.set(keys::BORDER_WIDTH, Some(NativeValue::Length(width as f32 / density)));
    let color = call(env, &foreground, "topColorArgb", "()I", &[]).and_then(|v| v.i().ok());
    node.set(keys::BORDER_COLOR, color.map(|c| NativeValue::Color(argb_to_rgba(c))));
    let style = call(env, &foreground, "borderStyleCode", "()I", &[]).and_then(|v| v.i().ok());
    node.set(keys::BORDER_STYLE, style.map(|code| NativeValue::Text(border_keyword(code).to_string())));
}

fn read_text(env: &mut JNIEnv, view: &JObject, density: f32, node: &mut NativeNode) {
    let text = call(env, view, "getText", "()Ljava/lang/CharSequence;", &[]).and_then(|v| v.l().ok());
    if let Some(text) = text.filter(|t| !t.is_null()) {
        let string = call(env, &text, "toString", "()Ljava/lang/String;", &[]).and_then(|v| v.l().ok());
        node.set(keys::TEXT, string.and_then(|s| java_string(env, s)).map(NativeValue::Text));
    }
    let color = call(env, view, "getCurrentTextColor", "()I", &[]).and_then(|v| v.i().ok());
    node.set(keys::TEXT_COLOR, color.map(|c| NativeValue::Color(argb_to_rgba(c))));
    let size = call(env, view, "getTextSize", "()F", &[]).and_then(|v| v.f().ok());
    node.set(keys::FONT_SIZE, size.map(|px| NativeValue::Length(px / density)));

    let Some(typeface) = call(env, view, "getTypeface", "()Landroid/graphics/Typeface;", &[])
        .and_then(|v| v.l().ok())
        .filter(|t| !t.is_null())
    else {
        return;
    };
    let weight = call(env, &typeface, "getWeight", "()I", &[]).and_then(|v| v.i().ok());
    node.set(keys::FONT_WEIGHT, weight.map(|w| NativeValue::Number(w as f32)));
    let family = call(env, &typeface, "getSystemFontFamilyName", "()Ljava/lang/String;", &[]).and_then(|v| v.l().ok());
    node.set(keys::FONT_FAMILY, family.and_then(|f| java_string(env, f)).map(NativeValue::Text));
}

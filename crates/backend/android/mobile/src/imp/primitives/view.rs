//! `Primitive::View` — `android.widget.LinearLayout` in vertical
//! orientation (matches the framework's default flex-column).

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(b: &AndroidBackend) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/LinearLayout").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        // Vertical orientation (1) so children stack top-to-bottom,
        // matching the framework's default flex-column layout.
        env.call_method(&local, "setOrientation", "(I)V", &[JValue::Int(1)])
            .unwrap();
        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

/// Insert `child` into `parent`. Two special cases:
///
/// - If `parent` is a registered ScrollView outer, the addView call
///   is redirected to its inner LinearLayout (the actual multi-child
///   container) — see [`super::scroll_view`] for the rationale.
/// - If `child` is a registered overlay content holder, the insert
///   is **skipped**. The overlay's content holder is already parented
///   to the dialog window via `Dialog.setContentView`; attempting
///   `parent.addView(overlay_child)` would throw
///   `IllegalStateException("The specified child already has a parent")`.
///   The walker calls `insert(presence_placeholder, overlay_node)`
///   for every overlay because the walker doesn't know about portals;
///   we filter here.
pub(crate) fn insert(b: &AndroidBackend, parent: &mut GlobalRef, child: GlobalRef) {
    if super::overlay::is_overlay_node(b, &child) {
        return;
    }
    let target = super::scroll_view::inner_for(b, parent).unwrap_or_else(|| parent.clone());
    with_env(|env| {
        env.call_method(
            target.as_obj(),
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&child.as_obj())],
        )
        .unwrap();
    });
}

/// Remove every child of `node`. If `node` is a registered
/// ScrollView outer, only its inner LinearLayout is cleared (the
/// outer's single child — the inner itself — must remain attached).
pub(crate) fn clear_children(b: &AndroidBackend, node: &GlobalRef) {
    let target = super::scroll_view::inner_for(b, node).unwrap_or_else(|| node.clone());
    with_env(|env| {
        env.call_method(target.as_obj(), "removeAllViews", "()V", &[])
            .unwrap();
    });
}

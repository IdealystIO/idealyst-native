//! `Element::View` — `android.widget.FrameLayout`. FrameLayout
//! is chosen (over LinearLayout) because the backend now drives
//! layout entirely through Taffy: every child gets its position
//! and size written directly onto its `FrameLayout.LayoutParams`
//! (`leftMargin`/`topMargin`/`width`/`height`) by
//! `AndroidBackend::run_layout_pass`. FrameLayout's own
//! `onMeasure`/`onLayout` honor those margins-as-offsets and let
//! children overlap, which is what `Position::Absolute` needs
//! (e.g. the welcome example's dark wash + sun-glare + content
//! layers all stacked over the page).

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(b: &AndroidBackend) -> GlobalRef {
    with_env(|env| {
        let class = env.find_class("android/widget/FrameLayout").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
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
/// - If `child` is a registered portal content holder, the insert
///   is **skipped**. The portal's content holder is already parented
///   (a viewport overlay was added to the Activity `root`; a popup
///   owns its own content view); attempting
///   `parent.addView(portal_child)` would throw
///   `IllegalStateException("The specified child already has a parent")`.
///   The walker calls `insert(presence_placeholder, portal_node)`
///   for every portal; we filter here.
pub(crate) fn insert(b: &mut AndroidBackend, parent: &mut GlobalRef, child: GlobalRef) {
    if super::overlay::is_portal_node(b, &child) {
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
    // Mirror the parent-child link into the Taffy tree so the
    // layout pass on `finish` produces a frame for `child` in the
    // parent's coordinate space. Use the Taffy-tracked parent
    // (which may be the ScrollView's inner LinearLayout when the
    // user-visible parent is a ScrollView outer), and `mark_dirty`
    // the parent so cached measurements are invalidated after the
    // child-set changes.
    let parent_layout = b.layout_for_view(&target);
    let child_layout = b.layout_for_view(&child);
    b.layout.add_child(parent_layout, child_layout);
    b.layout.mark_dirty(parent_layout);
}

/// Remove every child of `node`. If `node` is a registered
/// ScrollView outer, only its inner LinearLayout is cleared (the
/// outer's single child — the inner itself — must remain attached).
pub(crate) fn clear_children(b: &mut AndroidBackend, node: &GlobalRef) {
    let target = super::scroll_view::inner_for(b, node).unwrap_or_else(|| node.clone());
    with_env(|env| {
        env.call_method(target.as_obj(), "removeAllViews", "()V", &[])
            .unwrap();
    });
    let parent_layout = b.layout_for_view(&target);
    let children = b.layout.children_of(parent_layout);
    for c in children {
        b.layout.remove_child(parent_layout, c);
    }
    b.layout.mark_dirty(parent_layout);
}

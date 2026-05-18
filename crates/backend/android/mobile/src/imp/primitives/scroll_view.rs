//! `Primitive::ScrollView` — a `ScrollView` (or `HorizontalScrollView`)
//! wrapping a `LinearLayout`.
//!
//! We return the *outer* ScrollView as the framework's node so the
//! framework's `insert(parent, scrollview)` call works (the inner
//! LinearLayout is already a child of the outer — re-parenting it
//! would trip `addViewInner`'s "child already has a parent" guard).
//!
//! Child insertions still need to land on the inner LinearLayout
//! though — that's where multiple children belong. We register the
//! outer→inner mapping in `AndroidBackend::scroll_view_inner` and
//! [`super::view::insert`] / [`super::view::clear_children`] redirect
//! to the inner when the parent is a registered ScrollView outer.

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JValue};

pub(crate) fn create(b: &mut AndroidBackend, horizontal: bool) -> GlobalRef {
    // ScrollView is a single-child ViewGroup. To accept multiple
    // children we wrap a LinearLayout inside the ScrollView; the
    // inner LinearLayout is what receives child `addView` calls (via
    // the outer→inner indirection in `view::insert`).
    let (outer_ref, inner_ref) = with_env(|env| {
        let outer_class = if horizontal {
            env.find_class("android/widget/HorizontalScrollView").unwrap()
        } else {
            env.find_class("android/widget/ScrollView").unwrap()
        };
        let outer = env
            .new_object(
                &outer_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        let inner_class = env.find_class("android/widget/LinearLayout").unwrap();
        let inner = env
            .new_object(
                &inner_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        let orient = if horizontal { 0 } else { 1 };
        let _ = env.call_method(&inner, "setOrientation", "(I)V", &[JValue::Int(orient)]);
        let _ = env.call_method(
            &outer,
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&inner)],
        );
        // Defaults on both so width/height behave before any style
        // applies. Style application targets the outer (the
        // framework node) — that's what users size with `width:
        // 100%` etc. The inner is just a content holder.
        apply_default_layout_params(env, &outer);
        apply_default_layout_params(env, &inner);
        (
            env.new_global_ref(outer).unwrap(),
            env.new_global_ref(inner).unwrap(),
        )
    });

    // Register outer→inner so `view::insert(parent=outer, child)`
    // routes through the inner. See module doc.
    b.scroll_view_inner
        .insert(AndroidBackend::node_key_of(&outer_ref), inner_ref);

    outer_ref
}

/// Look up the child container for a parent node — used by
/// [`super::view::insert`] and [`super::view::clear_children`] to
/// transparently redirect operations targeting a ScrollView outer
/// onto its inner LinearLayout.
pub(crate) fn inner_for(b: &AndroidBackend, parent: &GlobalRef) -> Option<GlobalRef> {
    b.scroll_view_inner
        .get(&AndroidBackend::node_key_of(parent))
        .cloned()
}

/// Drop the outer→inner mapping when the outer is unstyled (the
/// framework's lifecycle hook for "this node is going away"). The
/// inner GlobalRef held in the map is the only thing keeping the
/// inner LinearLayout alive in our state once Java releases its own
/// reference, so dropping it lets the JVM GC the inner.
pub(crate) fn forget_inner(b: &mut AndroidBackend, parent: &GlobalRef) {
    b.scroll_view_inner.remove(&AndroidBackend::node_key_of(parent));
}

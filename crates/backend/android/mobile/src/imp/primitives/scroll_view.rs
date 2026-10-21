//! `ScrollViewPrim` — an outer scroller wrapping an inner `FrameLayout`.
//! The outer's class follows the [`ScrollAxis`]: `ScrollView`,
//! `HorizontalScrollView`, or the runtime's `RustTwoAxisScrollView` for
//! `Both` (see `crate::scroll_axis_policy` for why a nested pair of stock
//! scrollers can't stand in for it).
//!
//! We return the *outer* scroller as the framework's node so the
//! framework's `insert(parent, scrollview)` call works (the inner
//! FrameLayout is already a child of the outer — re-parenting it
//! would trip `addViewInner`'s "child already has a parent" guard).
//!
//! Child insertions still need to land on the inner FrameLayout
//! though — that's where multiple children belong. We register the
//! outer→inner mapping in `AndroidBackend::scroll_view_inner` and
//! [`super::view::insert`] / [`super::view::clear_children`] redirect
//! to the inner when the parent is a registered scroller outer.

use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use jni::objects::{GlobalRef, JClass, JObject, JValue};
use jni::JNIEnv;
use runtime_shared::ScrollAxis;

pub(crate) fn create(b: &mut AndroidBackend, axis: ScrollAxis) -> GlobalRef {
    // Every outer shape is a single-child ViewGroup. To accept multiple
    // children we wrap a FrameLayout inside the outer; the
    // inner FrameLayout is what receives child `addView` calls (via
    // the outer→inner indirection in `view::insert`).
    //
    // FrameLayout (not LinearLayout) because every other framework
    // container is absolute-positioned via `topMargin` / `leftMargin`
    // set by Taffy's apply_frames. LinearLayout stacks children
    // sequentially AND adds their topMargin on top of the stacking
    // offset, double-counting Taffy's y coordinate: a child Taffy
    // placed at y=705 ends up at y=(prev_bottom + 705) instead of
    // y=705. Visible as massive gaps between sidebar sections.
    // FrameLayout treats topMargin as the absolute y within the
    // container — matching how Taffy + apply_frames model positions.
    let (outer_ref, inner_ref) = with_env(|env| {
        let outer_class = env
            .find_class(crate::scroll_axis_policy::scroller_class(axis))
            .expect("scroller class — the two-axis one is staged with the Kotlin runtime");
        let outer = env
            .new_object(
                &outer_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        let inner_class = env.find_class("android/widget/FrameLayout").unwrap();
        let inner = env
            .new_object(
                &inner_class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        // Apply defaults BEFORE addView so the parent's
        // `generateLayoutParams` can convert the MarginLayoutParams
        // shape into its expected subtype (FrameLayout.LayoutParams
        // for ScrollView, etc.). Applying after addView would
        // overwrite the freshly-converted subclass LP with a bare
        // MarginLayoutParams and crash FrameLayout.onMeasure on the
        // downcast.
        apply_default_layout_params(env, &outer);
        apply_default_layout_params(env, &inner);
        let _ = env.call_method(
            &outer,
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&inner)],
        );
        (
            env.new_global_ref(outer).unwrap(),
            env.new_global_ref(inner).unwrap(),
        )
    });

    // Register outer→inner so `view::insert(parent=outer, child)`
    // routes through the inner. See module doc.
    b.scroll_view_inner
        .insert(AndroidBackend::node_key_of(&outer_ref), inner_ref.clone());

    // Wire the inner as a Taffy CHILD of the outer. `view::insert`
    // adds user-visible children under `inner_for(parent) = inner` in
    // Taffy, which makes the inner a Taffy root unless we link it
    // back to the outer here. Without that link, any author style on
    // the outer ScrollView — `Sidebar { padding: 16, width: 260dp }`
    // is the canonical case — never reaches the children: Taffy
    // computes the inner against viewport size and the children sit
    // at the inner's (0, 0) with no padding, ignoring the outer's
    // constraints entirely.
    //
    // Width/height resolution still works correctly: the outer
    // imposes its `style.width` (260dp), Taffy gives the inner the
    // outer's content area (260dp − padding), and children laid out
    // inside the inner inherit the padding offset.
    let outer_layout = b.layout_for_view(&outer_ref);
    let inner_layout = b.layout_for_view(&inner_ref);
    b.layout.add_child(outer_layout, inner_layout);
    b.layout.mark_dirty(outer_layout);

    // Mark the outer as a scroll container on its scroll axis. Because the
    // inner (scroll content) is a Taffy CHILD of the outer (above), the
    // content's height contributes to the outer's *automatic minimum size*
    // (a flex item's auto-min is its min-content). For the sidebar — an
    // `flex_grow:1 / flex_basis:0` child of a bounded panel — that floor
    // means flexbox can't shrink the outer below its 800px content: the
    // outer grows to content, ends up exactly as tall as what it holds, and
    // the native ScrollView has zero scrollable overflow ("I can't scroll
    // the sidebar"). `overflow:scroll` suppresses the auto-min floor (CSS
    // rule), so the parent bounds the outer to the panel height while the
    // content overflows — which is what makes a ScrollView scroll. iOS
    // doesn't need this (its scroll content is a separate Taffy root, not a
    // child), but Android parents content under the scroll node, so it does
    // — same reason macOS/terminal call it. Regression:
    // `regression_scroll_node_bounded_by_overflow_scroll_not_content`.
    // A `Both` scroller is marked on both axes, which also exempts the
    // inner from the `max-width: 100%` clamp so it can be wider than the
    // viewport.
    b.layout.set_overflow_scroll_axis(outer_layout, axis);

    // The inner's direction + cross-axis sizing per axis — see
    // `scroll_axis_policy::inner_content_rules` (Column for vertical and
    // two-axis content, Row for horizontal; a two-axis inner sized to its
    // content width so wide rows aren't clipped at the viewport edge).
    b.layout
        .set_style(inner_layout, &crate::scroll_axis_policy::inner_content_rules(axis));

    outer_ref
}

/// Look up the child container for a parent node — used by
/// [`super::view::insert`] and [`super::view::clear_children`] to
/// transparently redirect operations targeting a scroller outer
/// onto its inner FrameLayout.
pub(crate) fn inner_for(b: &AndroidBackend, parent: &GlobalRef) -> Option<GlobalRef> {
    b.scroll_view_inner
        .get(&AndroidBackend::node_key_of(parent))
        .cloned()
}

/// Drop the outer→inner mapping when the outer is unstyled (the
/// framework's lifecycle hook for "this node is going away"). The
/// inner GlobalRef held in the map is the only thing keeping the
/// inner FrameLayout alive in our state once Java releases its own
/// reference, so dropping it lets the JVM GC the inner.
pub(crate) fn forget_inner(b: &mut AndroidBackend, parent: &GlobalRef) {
    b.scroll_view_inner.remove(&AndroidBackend::node_key_of(parent));
}

/// The JNI classes of every scroller shape the backend creates
/// ([`crate::scroll_axis_policy::SCROLL_CONTAINER_CLASSES`]), for
/// [`is_scroll_container`]. A lookup that fails (it cannot for these
/// framework classes) is skipped with its exception cleared, so it can't
/// poison the next JNI call.
pub(crate) fn scroll_container_classes<'a>(env: &mut JNIEnv<'a>) -> Vec<JClass<'a>> {
    crate::scroll_axis_policy::SCROLL_CONTAINER_CLASSES
        .iter()
        .filter_map(|name| match env.find_class(*name) {
            Ok(class) => Some(class),
            Err(_) => {
                let _ = env.exception_clear();
                None
            }
        })
        .collect()
}

/// Whether `view` is one of the backend's scroller outers (any axis).
pub(crate) fn is_scroll_container(env: &mut JNIEnv, view: &JObject, classes: &[JClass]) -> bool {
    classes
        .iter()
        .any(|class| env.is_instance_of(view, class).unwrap_or(false))
}

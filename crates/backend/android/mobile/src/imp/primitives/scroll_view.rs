//! `Element::ScrollView` — a `ScrollView` (or `HorizontalScrollView`)
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
    // children we wrap a FrameLayout inside the ScrollView; the
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
    b.layout.set_overflow_scroll(outer_layout, horizontal);

    // Inner needs an explicit flex_direction. With Taffy's default
    // (`Row`) the inner only takes `max(child.height)` as its
    // intrinsic — children stack horizontally and the inner's
    // height collapses to one row. For a vertical scroller (the
    // common case: sidebar, page content) we want `Column` so the
    // children stack downward and the inner's height = sum of
    // children. Without this, only one row of items shows and the
    // rest get clipped because the inner doesn't grow tall enough
    // to scroll past the viewport. Horizontal scrollers
    // (`HorizontalScrollView`) get `Row` instead.
    let mut inner_rules = runtime_core::StyleRules::default();
    inner_rules.flex_direction = Some(if horizontal {
        runtime_core::FlexDirection::Row
    } else {
        runtime_core::FlexDirection::Column
    });
    inner_rules.align_items = Some(runtime_core::AlignItems::Stretch);
    b.layout.set_style(inner_layout, &inner_rules);

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

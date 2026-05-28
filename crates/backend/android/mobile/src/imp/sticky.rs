//! `Position::Sticky` on Android — pins views to their enclosing
//! `ScrollView` / `HorizontalScrollView` as the user scrolls. Mirror
//! of `backend/ios/mobile/src/imp/sticky.rs`; see that file for the
//! shared design rationale (side registry, layout-y caching, v1
//! vertical-only scope). Differences below are Android-specific.
//!
//! ## Hosting choice — side registry, not subclass
//!
//! The iOS reference picks side-registry over UIScrollView subclass
//! because UIKit has multiple scroll-view shapes (`UIScrollView`,
//! `UICollectionView`, third-party subclasses) and forcing them all
//! into a sticky-aware subclass is invasive. Android has the same
//! shape problem (`ScrollView`, `HorizontalScrollView`,
//! `NestedScrollView`, `RecyclerView`) and an additional reason: the
//! framework's existing scroll-view primitive already returns a
//! plain `android.widget.ScrollView`, and replacing it with a
//! sticky-aware Kotlin subclass would require shipping the new
//! class through `RUNTIME_KOTLIN_FILES` AND retroactively updating
//! every author who wrote `let s = backend.create_scroll_view(...)`
//! to expect that subclass back. Keep the scroll view plain; drive
//! the per-scroll-event recompute from a `View.OnScrollChangeListener`
//! installed on demand.
//!
//! ## Per-event vs per-vsync
//!
//! iOS uses a `CADisplayLink` (vsync tick) because UIScrollView's
//! `contentOffset` only changes when scroll is in progress — but the
//! tick still runs every frame, idle or not. We chose per-event
//! instead: Android's `View.OnScrollChangeListener.onScrollChange`
//! fires only when the scroll position actually changes. Strictly
//! cheaper than per-vsync, and the user-visible behavior is the
//! same (any frame where the scroll has moved is a frame where we
//! need to recompute the pin).
//!
//! ## Coordinate space
//!
//! All registry state is stored in `dp` (the dimension Taffy
//! produces and `StyleRules` reason in). The scroll listener
//! receives device pixels from Android; we convert at the entry
//! point. `View.setTranslationY` takes device pixels per
//! `[[project_android_setTranslation_device_px]]` — we convert with
//! `dp_to_px` at the apply site.
//!
//! ## Layout-y caching
//!
//! Same rationale as iOS: `View.setTranslationY` shifts the view's
//! drawn position without changing its layout frame, so reading
//! `view.getY()` after applying a translation returns a corrupted
//! natural-y. We walk Taffy parents from the sticky child to the
//! scroll view summing per-node `frame_of(node).y`, exactly like
//! iOS. Taffy frames are pure layout output, unaffected by Android
//! transforms.

use std::collections::HashMap;

use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

/// Sub-pixel threshold below which the sticky scroll-event handler
/// treats the child as already in the right place. Avoids per-event
/// `setTranslationY` churn when the scroll position isn't actually
/// changing the translation. Matches iOS's `STICKY_EPSILON`.
const STICKY_EPSILON: f32 = 0.5;

/// One sticky child registered against a scroll view.
pub(crate) struct StickyChild {
    /// Global ref to the child's `View`. Held so the per-scroll
    /// recompute can call `setTranslationY` even if the framework's
    /// own retain is released — the `deregister` path drops this
    /// entry well before that happens.
    pub(crate) view: GlobalRef,
    /// Pin threshold, in dp, read from `StyleRules.top`. The view
    /// pins when `scroll_y_dp + threshold_top > layout_y_dp`.
    pub(crate) threshold_top: f32,
    /// Natural y of the child in the scroll view's content
    /// coordinate space, in dp. Refreshed after every layout pass
    /// by [`refresh_layout_positions`]. Initialized to 0; the first
    /// layout pass replaces it with a real value.
    pub(crate) layout_y: f32,
    /// Last applied translation, in dp. Used to epsilon-skip
    /// redundant `setTranslationY` writes — matches the iOS
    /// implementation's `current_translate_y` read.
    pub(crate) last_translate: f32,
}

/// Per-scroll-view sticky state. Listener lifecycle lives in the
/// `AndroidBackend::scroll_listeners` map \u{2014} the `Element::
/// ScrollView` `on_scroll` callback rides the same Android
/// `setOnScrollChangeListener` slot, so both subsystems coordinate
/// install/detach through that shared registry.
pub(crate) struct StickyScrollEntry {
    /// Global ref to the outer ScrollView/HorizontalScrollView.
    /// Retained so the listener-driven path can read its current
    /// scroll position via `getScrollY()` without holding the
    /// `view_to_layout` reference.
    pub(crate) scroll_view: GlobalRef,
    pub(crate) children: HashMap<usize, StickyChild>,
}

/// Map from scroll view's JObject pointer → sticky bookkeeping.
pub(crate) type StickyRegistry = HashMap<usize, StickyScrollEntry>;

/// Pure compute used by the per-scroll-event handler and the unit
/// tests.
///
/// Returns the translation (in dp) that should be applied to the
/// sticky child given its natural layout y in the scroll view's
/// content space, the configured pin threshold (the `top` value),
/// and the scroll view's current scroll position. All inputs are
/// in dp; output is in dp.
///
/// TODO: horizontal sticky via `left` mirrors this shape with
/// `(layout_x, threshold_left, scroll_x)`. Wire it once an author
/// asks for it; matches the iOS module's same-shape TODO.
#[inline]
pub(crate) fn compute_translate_dp(
    layout_y_dp: f32,
    threshold_dp: f32,
    scroll_y_dp: f32,
) -> f32 {
    // Pin condition: the natural top of the child has scrolled
    // above the threshold band measured from the scroll view's
    // top edge. Translate the child *down* by the overshoot so its
    // rendered position stays at `scroll_y + threshold_top`.
    let pinned_y = scroll_y_dp + threshold_dp;
    if pinned_y > layout_y_dp {
        pinned_y - layout_y_dp
    } else {
        0.0
    }
}

/// Walk `view`'s parent chain looking for a `ScrollView` or
/// `HorizontalScrollView` ancestor. Returns the matching parent's
/// `GlobalRef` (a fresh one we own) or `None` if no scroll-view
/// ancestor exists. The caller can treat `None` as the
/// fall-back-to-relative path — sticky-in-non-scrolling-parent is
/// equivalent to relative per CSS.
///
/// We accept both Android scroll-view shapes (`ScrollView` and
/// `HorizontalScrollView`) because the framework's
/// `create_scroll_view(horizontal)` chooses based on direction;
/// the sticky implementation only cares about the vertical scroll
/// position today, so a HorizontalScrollView ancestor still
/// satisfies "you're inside a scroll container" — it just won't
/// pin (compute_translate's `scroll_y` will always be 0). Future
/// horizontal-sticky support reuses the same ancestor walk.
pub(crate) fn find_enclosing_scroll_view(
    env: &mut JNIEnv,
    view: &JObject,
) -> Option<GlobalRef> {
    // Class lookups for the two scroll-view shapes we recognise.
    // Resolving them once per call is cheap (the JNI class cache
    // hands back the same `jclass` across calls).
    let sv_class = env.find_class("android/widget/ScrollView").ok()?;
    let hsv_class = env.find_class("android/widget/HorizontalScrollView").ok()?;

    // Step up via `view.getParent()` — returns a `ViewParent`, which
    // is an interface. `ScrollView`/`HorizontalScrollView` both
    // implement it (because they extend `ViewGroup`, which extends
    // `View`, which implements ViewParent indirectly via casts).
    // `isInstanceOf` on the returned object handles the casts for us.
    let mut current = env
        .call_method(view, "getParent", "()Landroid/view/ViewParent;", &[])
        .ok()
        .and_then(|v| v.l().ok())?;

    // Defensive depth cap — a corrupted view tree (cycle, detached
    // parent loop) would otherwise hang the JNI thread.
    let mut steps = 0;
    while !current.is_null() {
        if env.is_instance_of(&current, &sv_class).unwrap_or(false)
            || env.is_instance_of(&current, &hsv_class).unwrap_or(false)
        {
            return env.new_global_ref(&current).ok();
        }
        let next = env
            .call_method(&current, "getParent", "()Landroid/view/ViewParent;", &[])
            .ok()
            .and_then(|v| v.l().ok());
        current = match next {
            Some(p) => p,
            None => return None,
        };
        steps += 1;
        if steps > 256 {
            return None;
        }
    }
    None
}

/// Stable JObject-pointer key for the global ref. Matches the
/// node-key scheme used elsewhere in the Android backend (animation
/// state, scroll-view inner map, etc.).
fn key_of(node: &GlobalRef) -> usize {
    node.as_obj().as_raw() as usize
}

/// Register a sticky child against its enclosing scroll view. If
/// `view` isn't inside any scroll view, this is a no-op (CSS's
/// sticky-in-non-scrolling-parent is equivalent to relative).
/// Returns `true` if the child was actually registered, `false` if
/// it fell back to relative-equivalent.
///
/// Idempotent: if the same view is already registered against a
/// (possibly different) scroll view, we deregister first so the
/// re-registration picks up any threshold or scroll-ancestor
/// changes (e.g. the view moved between scroll containers).
///
/// `scroll_listeners` is the backend's shared "what scroll views
/// already have the Kotlin listener attached" map. We consult it
/// here and through `Element::ScrollView::on_scroll`'s wiring so
/// both subsystems use a single `setOnScrollChangeListener` slot.
pub(crate) fn register(
    env: &mut JNIEnv,
    registry: &mut StickyRegistry,
    scroll_listeners: &mut HashMap<usize, GlobalRef>,
    view: &GlobalRef,
    threshold_top: f32,
    on_scroll_observers: &HashMap<usize, std::rc::Rc<dyn Fn(f32, f32)>>,
) -> bool {
    let child_key = key_of(view);

    // Drop any stale registration first.
    deregister(env, registry, scroll_listeners, view, on_scroll_observers);

    let Some(scroll_view) = find_enclosing_scroll_view(env, &view.as_obj()) else {
        return false;
    };
    let scroll_key = key_of(&scroll_view);

    let entry = registry.entry(scroll_key).or_insert_with(|| StickyScrollEntry {
        scroll_view: scroll_view.clone(),
        children: HashMap::new(),
    });

    entry.children.insert(
        child_key,
        StickyChild {
            view: view.clone(),
            threshold_top,
            layout_y: 0.0,
            last_translate: 0.0,
        },
    );

    ensure_scroll_listener(env, scroll_listeners, &scroll_view, scroll_key);

    true
}

/// Remove `view` from any scroll view's sticky registry it's a
/// member of. Resets the view's `translationY` to 0 so a
/// previously-pinned view doesn't leave a translate behind when its
/// `position` changes from `Sticky` to something else.
///
/// If removing this child empties the scroll view's child set, the
/// scroll view's `OnScrollChangeListener` is detached \u{2014} but
/// only if no `on_scroll` observer is still registered for the same
/// scroll view (the listener is shared with the user-facing scroll
/// callback).
pub(crate) fn deregister(
    env: &mut JNIEnv,
    registry: &mut StickyRegistry,
    scroll_listeners: &mut HashMap<usize, GlobalRef>,
    view: &GlobalRef,
    on_scroll_observers: &HashMap<usize, std::rc::Rc<dyn Fn(f32, f32)>>,
) {
    let child_key = key_of(view);

    let mut emptied_scrolls = Vec::new();
    for (scroll_key, entry) in registry.iter_mut() {
        if entry.children.remove(&child_key).is_some() {
            // Reset translation on the freshly-deregistered view so
            // a previously-pinned translate doesn't persist. Pass
            // 0.0 dp; the JVM-side `setTranslationY` takes device
            // pixels but converting 0 → 0 doesn't need density.
            let _ = env.call_method(
                view.as_obj(),
                "setTranslationY",
                "(F)V",
                &[JValue::Float(0.0)],
            );
            if entry.children.is_empty() {
                emptied_scrolls.push(*scroll_key);
            }
        }
    }
    for scroll_key in emptied_scrolls {
        if let Some(entry) = registry.remove(&scroll_key) {
            release_scroll_listener_if_unused(
                env,
                scroll_listeners,
                &entry.scroll_view,
                scroll_key,
                registry,
                on_scroll_observers,
            );
        }
    }
}

/// Remove an entire scroll view's sticky bookkeeping. Used when the
/// scroll view itself unmounts — clears each child's translation
/// and releases the listener if nothing else is using it.
pub(crate) fn deregister_scroll_view(
    env: &mut JNIEnv,
    registry: &mut StickyRegistry,
    scroll_listeners: &mut HashMap<usize, GlobalRef>,
    scroll_view: &GlobalRef,
    on_scroll_observers: &HashMap<usize, std::rc::Rc<dyn Fn(f32, f32)>>,
) {
    let scroll_key = key_of(scroll_view);
    let Some(mut entry) = registry.remove(&scroll_key) else {
        return;
    };
    for (_, child) in entry.children.drain() {
        let _ = env.call_method(
            child.view.as_obj(),
            "setTranslationY",
            "(F)V",
            &[JValue::Float(0.0)],
        );
    }
    release_scroll_listener_if_unused(
        env,
        scroll_listeners,
        &entry.scroll_view,
        scroll_key,
        registry,
        on_scroll_observers,
    );
}

/// Install the Kotlin `RustStickyScrollListener` on `scroll_view`
/// if it isn't already attached. Idempotent: the second call from
/// either subsystem (sticky-child registration or `on_scroll`
/// wiring on `create_scroll_view`) is a no-op. Android allows only
/// one listener per `setOnScrollChangeListener` slot, so both
/// subsystems share the same Kotlin object \u{2014} the JNI dispatch
/// fans out to both registries on every scroll event.
pub(crate) fn ensure_scroll_listener(
    env: &mut JNIEnv,
    scroll_listeners: &mut HashMap<usize, GlobalRef>,
    scroll_view: &GlobalRef,
    scroll_key: usize,
) {
    if scroll_listeners.contains_key(&scroll_key) {
        return;
    }
    let Some(listener_class) = env
        .find_class("io/idealyst/runtime/RustStickyScrollListener")
        .ok()
    else {
        return;
    };
    // Constructor takes a long (the scroll-view key). The key is
    // the JObject raw pointer — stable for the duration of the
    // GlobalRef the registry holds.
    let Some(listener) = env
        .new_object(
            &listener_class,
            "(J)V",
            &[JValue::Long(scroll_key as jni::sys::jlong)],
        )
        .ok()
    else {
        return;
    };
    let Some(global) = env.new_global_ref(&listener).ok() else {
        return;
    };
    // Wire it as the scroll view's `OnScrollChangeListener`.
    // `View.setOnScrollChangeListener` has been on `View` since API
    // 23; ScrollView inherits it. The JNI sig uses the interface
    // type.
    if env
        .call_method(
            scroll_view.as_obj(),
            "setOnScrollChangeListener",
            "(Landroid/view/View$OnScrollChangeListener;)V",
            &[JValue::Object(&global.as_obj())],
        )
        .is_ok()
    {
        scroll_listeners.insert(scroll_key, global);
    }
}

/// Detach the Kotlin listener from `scroll_view` if no subsystem
/// still needs it \u{2014} i.e. no sticky child is registered AND no
/// `on_scroll` observer is registered for the same scroll view. The
/// listener slot is shared, so we have to consult both before
/// pulling the plug.
pub(crate) fn release_scroll_listener_if_unused(
    env: &mut JNIEnv,
    scroll_listeners: &mut HashMap<usize, GlobalRef>,
    scroll_view: &GlobalRef,
    scroll_key: usize,
    sticky_registry: &StickyRegistry,
    on_scroll_observers: &HashMap<usize, std::rc::Rc<dyn Fn(f32, f32)>>,
) {
    let sticky_in_use = sticky_registry
        .get(&scroll_key)
        .map(|e| !e.children.is_empty())
        .unwrap_or(false);
    let on_scroll_in_use = on_scroll_observers.contains_key(&scroll_key);
    if sticky_in_use || on_scroll_in_use {
        return;
    }
    detach_scroll_listener(env, scroll_view);
    let _ = scroll_listeners.remove(&scroll_key);
}

/// Detach the scroll listener from a scroll view by passing `null`
/// to `setOnScrollChangeListener`. Best-effort — if the scroll view
/// has already been GC'd the JNI call fails harmlessly.
fn detach_scroll_listener(env: &mut JNIEnv, scroll_view: &GlobalRef) {
    let null = JObject::null();
    let _ = env.call_method(
        scroll_view.as_obj(),
        "setOnScrollChangeListener",
        "(Landroid/view/View$OnScrollChangeListener;)V",
        &[JValue::Object(&null)],
    );
    // Clear any pending exception so a later JNI call doesn't see
    // it; `call_method` already propagates the Err.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

/// Per-scroll-event recompute. Called from the JNI trampoline that
/// `RustStickyScrollListener.onScrollChange` dispatches to.
///
/// `scroll_x_px` / `scroll_y_px` are device-pixel scroll positions
/// as Android's `View` reports them; we convert to dp via the
/// scroll view's display density.
pub(crate) fn on_scroll_event(
    env: &mut JNIEnv,
    registry: &mut StickyRegistry,
    scroll_key: usize,
    _scroll_x_px: f32,
    scroll_y_px: f32,
) {
    let Some(entry) = registry.get_mut(&scroll_key) else {
        return;
    };
    // Read precise display density off the scroll view's
    // resources (`getResources().getDisplayMetrics().density`).
    // Failure paths fall back to 1.0 — the pin position will be
    // off on high-density displays for one frame; the next scroll
    // event retries the read. Note: using the integer
    // `dp_to_px(env, ..., 1.0)` shortcut here would discard the
    // fractional component of densities like 2.625 (round to 3)
    // and produce visibly-wrong pin positions on those devices.
    let density = super::density_of(env, &entry.scroll_view.as_obj()).unwrap_or(1.0);
    let density = if density <= 0.0 { 1.0 } else { density };
    let scroll_y_dp = scroll_y_px / density;

    for (_, child) in entry.children.iter_mut() {
        let translate_dp =
            compute_translate_dp(child.layout_y, child.threshold_top, scroll_y_dp);
        if (translate_dp - child.last_translate).abs() < STICKY_EPSILON {
            continue;
        }
        // `setTranslationY` takes device pixels —
        // [[project_android_setTranslation_device_px]].
        let translate_px = translate_dp * density;
        let _ = env.call_method(
            child.view.as_obj(),
            "setTranslationY",
            "(F)V",
            &[JValue::Float(translate_px)],
        );
        child.last_translate = translate_dp;
    }
}

/// Refresh the cached `layout_y` for every sticky child after a
/// layout pass. Walks Taffy parents from the child up to its
/// registered scroll view, summing per-node `frame_of(...).y`.
///
/// Same rationale as the iOS counterpart: `setTranslationY` shifts
/// the view's drawn position without changing its layout frame, so
/// reading `view.getY()` after applying a translation gives the
/// wrong natural-y. Taffy frames are pure layout output,
/// unaffected by Android transforms.
///
/// After refreshing each child's layout-y we run a fresh recompute
/// against the scroll view's *current* scroll position. Without
/// this, a tree rebuild (route switch, theme change) that didn't
/// move the scroll bar leaves stale translations on still-pinned
/// children — visible as the pinned-headers-snap-to-the-wrong-spot
/// regression that iOS's matching refresh path also guards against.
pub(crate) fn refresh_layout_positions(
    env: &mut JNIEnv,
    registry: &mut StickyRegistry,
    layout: &runtime_layout::LayoutTree,
    view_to_layout: &HashMap<usize, (GlobalRef, runtime_layout::LayoutNode)>,
) {
    // Take a snapshot of the keys so we can mutate `registry` per
    // entry without holding an outer iterator borrow.
    let scroll_keys: Vec<usize> = registry.keys().copied().collect();
    for scroll_key in scroll_keys {
        // Re-fetch the entry's scroll_view + child layout-y map
        // first (immutable read), then apply translations
        // (mutable write). Splitting the borrow lets us run the
        // per-entry update without rewriting the outer loop.
        let Some(entry) = registry.get_mut(&scroll_key) else {
            continue;
        };
        // Read live scroll position from the scroll view. Same
        // density-fallback as `on_scroll_event`. See that function
        // for why we read precise density via `density_of` rather
        // than the integer-rounded `dp_to_px(env, ..., 1.0)`
        // shortcut.
        let density = super::density_of(env, &entry.scroll_view.as_obj()).unwrap_or(1.0);
        let density = if density <= 0.0 { 1.0 } else { density };
        let scroll_y_px = env
            .call_method(entry.scroll_view.as_obj(), "getScrollY", "()I", &[])
            .and_then(|v| v.i())
            .unwrap_or(0) as f32;
        let scroll_y_dp = scroll_y_px / density;

        for (child_key, child) in entry.children.iter_mut() {
            if let Some(layout_y) = compute_layout_y_in_scroll(
                *child_key,
                scroll_key,
                layout,
                view_to_layout,
            ) {
                child.layout_y = layout_y;
            }
            // Recompute against the live scroll position so a
            // tree-rebuild-without-scroll picks up the corrected
            // layout-y immediately. Skips the JNI write when
            // unchanged (epsilon-gated).
            let translate_dp =
                compute_translate_dp(child.layout_y, child.threshold_top, scroll_y_dp);
            if (translate_dp - child.last_translate).abs() < STICKY_EPSILON {
                continue;
            }
            let translate_px = translate_dp * density;
            let _ = env.call_method(
                child.view.as_obj(),
                "setTranslationY",
                "(F)V",
                &[JValue::Float(translate_px)],
            );
            child.last_translate = translate_dp;
        }
    }
}

/// Sum Taffy frame y values from `child_key` up to (but not
/// including) `scroll_key`. Returns `None` if we can't trace the
/// chain (child or an ancestor isn't in `view_to_layout`, or we
/// walked off the root without finding the scroll view).
fn compute_layout_y_in_scroll(
    child_key: usize,
    scroll_key: usize,
    layout: &runtime_layout::LayoutTree,
    view_to_layout: &HashMap<usize, (GlobalRef, runtime_layout::LayoutNode)>,
) -> Option<f32> {
    let (_, child_node) = view_to_layout.get(&child_key)?;
    let (_, scroll_node) = view_to_layout.get(&scroll_key)?;

    let mut sum_y = 0.0_f32;
    let mut cursor = *child_node;

    let mut steps = 0;
    while cursor != *scroll_node {
        sum_y += layout.frame_of(cursor).y;
        let Some(parent) = layout.parent_of(cursor) else {
            return None;
        };
        cursor = parent;
        steps += 1;
        if steps > 256 {
            return None;
        }
    }
    Some(sum_y)
}

// =========================================================================
// Tests
// =========================================================================
//
// Host-runnable regression coverage lives in
// `crate::sticky_compute::tests` (a sibling module outside the
// `cfg(target_os = "android")` gate). That module duplicates
// `compute_translate_dp` so the math regression and the
// shrink-on-empty registry invariant run from `cargo test
// -p backend-android-mobile` without needing a JVM. See the doc on
// `crate::sticky_compute` for the rationale (this `imp::sticky`
// module is target-gated because of the `jni` dep, so its tests
// would never reach host).
//
// A visual integration test ("View { position: Sticky } pins as
// the user scrolls a docs page on a real device") isn't included
// — `cargo test` on host can't drive an Android emulator, same
// blocker iOS's UIKit gesture tests face. Verified on-device by
// the docs example's sticky-header demo.

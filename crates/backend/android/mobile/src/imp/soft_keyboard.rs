//! Soft keyboard (IME) — the JNI half.
//!
//! - `RustKeyboardInsets` (installed on the host root) owns window-insets
//!   handling and the system IME animation callbacks; it reports where the
//!   keyboard is heading to author code ([`on_target`] →
//!   `keyboard_inset()`) and forwards every animation phase to the
//!   registered avoiders. The app viewport itself never shrinks.
//! - Each `keyboard_avoiding_view` gets a Kotlin `RustKeyboardAvoider`
//!   ([`mark`]). Per-frame work happens entirely in Kotlin (`translationY`
//!   writes). Rust is called at most at the START and END of a keyboard
//!   move for a `Padding` avoider: [`begin_padding`] lays out once and hands
//!   Kotlin the views that moved; [`commit_padding`] applies a deferred
//!   layout. See `crate::soft_keyboard_policy` for why.

use crate::imp::{density_of, with_env, AndroidBackend};
use crate::soft_keyboard_policy::{moved_views, padding_plan, PaddingPlan};
use jni::objects::{GlobalRef, JObject, JValue};

/// A registered `keyboard_avoiding_view`.
pub(crate) struct Avoider {
    /// Its `RustKeyboardAvoider` (Kotlin) instance.
    kotlin: GlobalRef,
    node: runtime_layout::LayoutNode,
}

/// Install `RustKeyboardInsets` on the host root. Best-effort like
/// [`AndroidBackend::install_viewport_resize_listener`]: a staged Kotlin
/// runtime without the class (CLI not reinstalled since this landed) throws
/// from `find_class`; the pending exception is cleared and the app keeps
/// the previous behavior.
pub(crate) fn install(backend: &AndroidBackend) {
    let root = backend.root.clone();
    let context = backend.context.clone();
    with_env(|env| {
        let class = match env.find_class("io/idealyst/runtime/RustKeyboardInsets") {
            Ok(c) => c,
            Err(_) => {
                let _ = env.exception_clear();
                return;
            }
        };
        let _ = env.call_static_method(
            &class,
            "install",
            "(Landroid/content/Context;Landroid/view/View;)Z",
            &[JValue::Object(context.as_obj()), JValue::Object(root.as_obj())],
        );
        let _ = env.exception_clear();
    });
}

/// Make `node` a `keyboard_avoiding_view`. The Kotlin avoider registers
/// with `RustKeyboardInsets` and, if the keyboard is already up, applies
/// its overlap on the next main-thread turn (not now: the backend is
/// borrowed for this mount, and a `Padding` apply calls back into it).
pub(crate) fn mark(
    backend: &mut AndroidBackend,
    node: &GlobalRef,
    avoid: runtime_shared::KeyboardAvoid,
) {
    let key = AndroidBackend::node_key_of(node);
    let layout_node = backend.layout_for_view(node);
    let behavior = match avoid.behavior {
        runtime_shared::KeyboardAvoidBehavior::Padding => 0,
        runtime_shared::KeyboardAvoidBehavior::Translate => 1,
    };
    let kotlin = with_env(|env| -> Option<GlobalRef> {
        let class = match env.find_class("io/idealyst/runtime/RustKeyboardAvoider") {
            Ok(c) => c,
            Err(_) => {
                let _ = env.exception_clear();
                return None;
            }
        };
        let obj = env
            .call_static_method(
                &class,
                "attach",
                "(Landroid/view/View;JIZ)Lio/idealyst/runtime/RustKeyboardAvoider;",
                &[
                    JValue::Object(node.as_obj()),
                    JValue::Long(key as i64),
                    JValue::Int(behavior),
                    JValue::Bool(avoid.animated as u8),
                ],
            )
            .and_then(|v| v.l());
        let obj = match obj {
            Ok(o) if !o.is_null() => o,
            _ => {
                let _ = env.exception_clear();
                return None;
            }
        };
        env.new_global_ref(obj).ok()
    });
    if let Some(kotlin) = kotlin {
        backend.keyboard_avoiders.insert(key, Avoider { kotlin, node: layout_node });
    }
}

/// The view behind `key` was released: detach its Kotlin avoider so the
/// IME callbacks stop reaching it.
pub(crate) fn unregister(backend: &mut AndroidBackend, key: usize) {
    let Some(a) = backend.keyboard_avoiders.remove(&key) else { return };
    with_env(|env| {
        let _ = env.call_method(a.kotlin.as_obj(), "detach", "()V", &[]);
        let _ = env.exception_clear();
    });
}

/// `Padding` avoider `key` moves from `from_dp` to `to_dp` of keyboard
/// padding, animated by the system IME animation that is starting now.
/// Lays out once per [`padding_plan`] and hands the Kotlin avoider the
/// views that move, with each one's OLD and NEW parent-relative top in
/// device px (rounded exactly as `RustLayoutApply` writes them, so they
/// compare equal to `View.getTop()`), and the mode (0 = `GrowNow`: the new
/// layout is applied now; 1 = `ShrinkAtEnd`: Kotlin calls
/// [`commit_padding`] when the animation ends). Kotlin animates each
/// view's VISUAL top from old to new and translates by the difference from
/// wherever its layout currently has it: the `LayoutParams` a pass writes
/// only take effect at the next traversal, so the view can sit at either
/// position on any given frame. Returns `false` when there is nothing to
/// animate (the Kotlin side then just commits at the end).
pub(crate) fn begin_padding(backend: &mut AndroidBackend, key: usize, from_dp: f32, to_dp: f32) -> bool {
    let Some(node) = backend.keyboard_avoiders.get(&key).map(|a| a.node) else { return false };
    let (vw, vh) = backend.viewport_size();
    if vw <= 0.0 || vh <= 0.0 {
        return false;
    }
    let plan = padding_plan(from_dp, to_dp, true);
    let subtree = subtree_views(backend, node);
    let before = ys(backend, &subtree);
    let mode = match plan {
        PaddingPlan::Unchanged | PaddingPlan::Instant => return false,
        PaddingPlan::GrowNow => {
            backend.layout.set_keyboard_padding(node, to_dp);
            backend.run_layout_pass();
            0
        }
        PaddingPlan::ShrinkAtEnd => {
            // Measure the final layout, then put the current one back: the
            // views stay at the larger size until the keyboard covers the
            // difference; any layout pass in between keeps the old padding.
            backend.layout.set_keyboard_padding(node, to_dp);
            backend.compute_layout_roots(vw, vh);
            1
        }
    };
    let after = ys(backend, &subtree);
    if mode == 1 {
        backend.layout.set_keyboard_padding(node, from_dp);
        backend.compute_layout_roots(vw, vh);
    }
    let moved = moved_views(&before, &after);
    let Some(kotlin) = backend.keyboard_avoiders.get(&key).map(|a| a.kotlin.clone()) else { return false };
    let root = backend.root.clone();
    let views: Vec<GlobalRef> = moved
        .iter()
        .filter_map(|(k, _)| backend.view_to_layout.get(k).map(|(v, _)| v.clone()))
        .collect();
    with_env(|env| {
        let density = density_of(env, root.as_obj()).unwrap_or(1.0);
        let Ok(arr) = env.new_object_array(views.len() as i32, "android/view/View", JObject::null())
        else {
            let _ = env.exception_clear();
            return;
        };
        for (i, v) in views.iter().enumerate() {
            let _ = env.set_object_array_element(&arr, i as i32, v.as_obj());
        }
        let (old_tops, new_tops): (Vec<i32>, Vec<i32>) = moved
            .iter()
            .map(|(k, dy)| {
                let new_y = after.iter().find(|(ka, _)| ka == k).map(|(_, y)| *y).unwrap_or(0.0);
                crate::soft_keyboard_policy::tops_px(new_y, *dy, density)
            })
            .unzip();
        let (Ok(old_arr), Ok(new_arr)) =
            (env.new_int_array(old_tops.len() as i32), env.new_int_array(new_tops.len() as i32))
        else {
            let _ = env.exception_clear();
            return;
        };
        let _ = env.set_int_array_region(&old_arr, 0, &old_tops);
        let _ = env.set_int_array_region(&new_arr, 0, &new_tops);
        let _ = env.call_method(
            kotlin.as_obj(),
            "setTargets",
            "([Landroid/view/View;[I[II)V",
            &[JValue::Object(&arr), JValue::Object(&old_arr), JValue::Object(&new_arr), JValue::Int(mode)],
        );
        let _ = env.exception_clear();
    });
    true
}

/// Apply `dp` of keyboard padding to avoider `key` and lay out now — an
/// unanimated change, or the end of a `ShrinkAtEnd` animation.
pub(crate) fn commit_padding(backend: &mut AndroidBackend, key: usize, dp: f32) {
    let Some(node) = backend.keyboard_avoiders.get(&key).map(|a| a.node) else { return };
    if backend.layout.set_keyboard_padding(node, dp) {
        backend.run_layout_pass();
    }
}

/// The IME started moving (or settled): report where it is heading, with
/// the animation's duration, to author code (`keyboard_inset()`).
pub(crate) fn on_target(height_dp: f32, duration_ms: i64) {
    crate::newcore::forward_keyboard_inset(runtime_shared::KeyboardInset::new(
        height_dp,
        crate::soft_keyboard_policy::keyboard_transition(duration_ms),
    ));
}

/// Run `f` with the global backend if it is free. A JNI entry from the IME
/// animation runs between frames, so the backend is normally free; when it
/// isn't, the Kotlin side falls back to committing at the end.
pub(crate) fn with_free_backend<R>(f: impl FnOnce(&mut AndroidBackend) -> R) -> Option<R> {
    let rc = super::ANDROID_BACKEND_SELF.with(|s| s.borrow().clone())?.upgrade()?;
    let mut b = rc.try_borrow_mut().ok()?;
    Some(f(&mut b))
}

/// View keys whose layout node lies strictly inside `root`'s subtree.
fn subtree_views(backend: &AndroidBackend, root: runtime_layout::LayoutNode) -> Vec<usize> {
    backend
        .view_to_layout
        .iter()
        .filter(|(_, (_, n))| *n != root && is_descendant(backend, *n, root))
        .map(|(k, _)| *k)
        .collect()
}

fn is_descendant(
    backend: &AndroidBackend,
    mut node: runtime_layout::LayoutNode,
    ancestor: runtime_layout::LayoutNode,
) -> bool {
    while let Some(p) = backend.layout.parent_of(node) {
        if p == ancestor {
            return true;
        }
        node = p;
    }
    false
}

/// Each view's current parent-relative y (dp) from the layout tree.
fn ys(backend: &AndroidBackend, keys: &[usize]) -> Vec<(usize, f32)> {
    keys.iter()
        .filter_map(|k| {
            let (_, n) = backend.view_to_layout.get(k)?;
            Some((*k, backend.layout.frame_of(*n).y))
        })
        .collect()
}

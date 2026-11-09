//! Android-mobile backend accessibility translation —
//! [`AccessibilityProps`] → `android.view.View` setter calls via JNI.
//!
//! Strategy mirrors the iOS / web backends: a single [`apply`]
//! function takes a node + resolved props and writes (or clears) every
//! relevant accessibility property. `create_*` paths call it after
//! constructing the view; the dynamic
//! `update_accessibility` path (`runtime_vocabulary::caps::A11yOps`)
//! reuses it identically. Clearing on `None` is intentional —
//! reactive a11y prop changes must not leak stale labels onto a view.
//!
//! TalkBack walks the platform `View` tree directly via
//! `AccessibilityNodeInfo`, so the Android backend doesn't maintain a
//! parallel semantics tree;
//! `A11yOps::dump_accessibility_tree` stays at its `None` default for
//! this backend.
//!
//! ### API-level gating
//!
//! Several setters here require API levels above the framework's
//! minSdk floor (21). We call them unconditionally and rely on JNI's
//! `NoSuchMethodError` — caught as a Java exception, cleared, ignored
//! — to no-op on older devices. This keeps the call sites simple and
//! avoids burning a `Build.VERSION.SDK_INT` check on every node:
//!
//! - `setTooltipText` — API 26+
//! - `setAccessibilityPaneTitle` — API 28+
//! - `setAccessibilityHeading` — API 28+
//!
//! ### Trait mapping subtleties
//!
//! Most Android first-class state mapping lives on
//! `AccessibilityNodeInfo`, which is normally only mutated via a
//! custom `AccessibilityDelegate`. We use the View-level
//! setters that exist (label / hint / hidden / live region / heading
//! / selected / enabled), and fold remaining state flags
//! (CHECKED / EXPANDED / COLLAPSED / REQUIRED / READONLY / INVALID /
//! MIXED / BUSY) into the `contentDescription` tail string so TalkBack
//! still announces them. A future Kotlin `IdealystA11y` delegate shim
//! could promote these to `setStateDescription` (API 30+) and
//! `setRoleDescription` for richer TalkBack output.
//!
//! ### Live regions
//!
//! `View.setAccessibilityLiveRegion(int)` exists at View level (API
//! 19+). `Polite` → `ACCESSIBILITY_LIVE_REGION_POLITE (1)`,
//! `Assertive` → `ACCESSIBILITY_LIVE_REGION_ASSERTIVE (2)`. The
//! framework re-applies the label on signal change; the
//! TalkBack engine observes the mutation and announces with the
//! configured priority.

use runtime_shared::accessibility::{AccessibilityProps, LiveRegionPriority, Role};

use crate::a11y_policy::A11yWrites;
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

use super::with_env;

// ---------------------------------------------------------------------------
// Public surface.
// ---------------------------------------------------------------------------

/// Apply / refresh every accessibility property on `view` from `props`.
///
/// `inferred_role` is the primitive's default role (see
/// [`runtime_shared::accessibility::default_role`]). If
/// `props.role.is_none()` and `inferred_role.is_some()`, the inferred
/// role drives the heading / role-description fallback; if both are
/// `None`, no role-derived bits are added (the caller's trait flags
/// still apply).
///
/// Idempotent: every property is either written or explicitly
/// cleared. Calling twice with the same `props` produces the same
/// View state; calling with different `props` always converges.
pub(crate) fn apply(node: &GlobalRef, props: &AccessibilityProps, inferred_role: Option<Role>) {
    let writes = A11yWrites::of(props, inferred_role);
    with_env(|env| write(env, &node.as_obj(), &writes));
}

/// [`apply`] for a view just constructed: skips the setters entirely when
/// they would leave it as a fresh view already is (no accessibility props,
/// a role that forces nothing), see `crate::a11y_policy`.
///
/// Only for views whose construction leaves `View`'s accessibility
/// defaults in place: a `FrameLayout`, and the text view
/// `RustViewFactory.text` makes (which resets the importance `TextView`'s
/// constructor raises). A `RecyclerView` or a bare `EditText` raises its
/// own importance, so its create path goes through [`apply`] to keep the
/// importance this module decides.
pub(crate) fn apply_new(node: &GlobalRef, props: &AccessibilityProps, inferred_role: Option<Role>) {
    let writes = A11yWrites::of(props, inferred_role);
    if writes.is_fresh_view_state() {
        return;
    }
    with_env(|env| write(env, &node.as_obj(), &writes));
}

fn write(env: &mut JNIEnv, view: &JObject, w: &A11yWrites) {
    // TalkBack reads contentDescription verbatim, so the composed
    // string is the user-observable announcement.
    set_content_description(env, view, w.content_description.as_deref());

    // Tooltip / hint — API 26+. The setter throws NoSuchMethodError on
    // older devices; we swallow it (see module doc on API-level
    // gating).
    set_tooltip_text(env, view, w.tooltip.as_deref());

    // Identifier — Android has no first-class `accessibilityIdentifier`
    // on plain `View`. We surface it via `setAccessibilityPaneTitle`
    // (API 28+) so UIAutomator / on-device test harnesses that walk
    // for "pane title" can find the element. On older API levels the
    // call is a no-op (NoSuchMethodError swallowed).
    set_accessibility_pane_title(env, view, w.pane_title.as_deref());

    let _ = env.call_method(
        view,
        "setImportantForAccessibility",
        "(I)V",
        &[JValue::Int(w.important)],
    );
    clear_exception(env);

    // Live region — written at the View level (API 19+).
    let _ = env.call_method(
        view,
        "setAccessibilityLiveRegion",
        "(I)V",
        &[JValue::Int(w.live_region)],
    );
    clear_exception(env);

    // Heading — `View.setAccessibilityHeading(boolean)` (API 28+).
    // Cross-backend behavior is: TalkBack announces "heading" before
    // the label, parity with iOS `UIAccessibilityTraitHeader` and web
    // `role="heading"`. NoSuchMethodError on older devices is
    // swallowed and the element falls back to a plain text node, same
    // graceful-degradation policy as tooltip.
    let _ = env.call_method(
        view,
        "setAccessibilityHeading",
        "(Z)V",
        &[JValue::Bool(w.heading as u8)],
    );
    clear_exception(env);

    // SELECTED — `View.setSelected(boolean)`. Note: this also affects
    // the View's visual selected state (drawable state set), which is
    // the intended behavior — Tab / ListItem styling drives off the
    // same flag, so a11y and visual selection stay in sync.
    let _ = env.call_method(view, "setSelected", "(Z)V", &[JValue::Bool(w.selected as u8)]);
    clear_exception(env);

    // DISABLED — `View.setEnabled(false)` when the trait is set. Never
    // `true`: the explicit author-driven disable path is `set_disabled`,
    // and writing `true` here would re-enable a view some other layer
    // disabled.
    if w.disabled {
        let _ = env.call_method(view, "setEnabled", "(Z)V", &[JValue::Bool(0)]);
        clear_exception(env);
    }
}

// ---------------------------------------------------------------------------
// announce_for_accessibility — `View.announceForAccessibility(CharSequence)`.
// ---------------------------------------------------------------------------

/// Post a one-shot accessibility announcement via the root View's
/// `announceForAccessibility` method.
///
/// TalkBack reads `msg` aloud immediately if no other speech is in
/// flight; otherwise it queues behind the current utterance. Android
/// has no public per-announcement priority API — `Polite` and
/// `Assertive` both route through `announceForAccessibility` for v1.
/// The framework still distinguishes them in the signal so a future
/// Kotlin shim can post `TYPE_ANNOUNCEMENT` events with the right
/// `getEventTime` ordering if assertive interruption ever becomes a
/// real need.
pub(crate) fn announce(
    env: &mut JNIEnv,
    root_view: &JObject,
    msg: &str,
    _priority: LiveRegionPriority,
) {
    let java_msg = match env.new_string(msg) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[backend-android] announce: new_string failed: {e}");
            let _ = env.exception_clear();
            return;
        }
    };
    let _ = env.call_method(
        root_view,
        "announceForAccessibility",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&JObject::from(java_msg))],
    );
    clear_exception(env);
}

// ---------------------------------------------------------------------------
// String setters with nil/clear semantics.
// ---------------------------------------------------------------------------

/// `setContentDescription(CharSequence)` — pass `null` to clear so a
/// reactive a11y prop that drops the label actually reverts TalkBack
/// to the View's natural announcement (visible text on a TextView,
/// "Button" for Button, etc.).
fn set_content_description(env: &mut JNIEnv, view: &JObject, value: Option<&str>) {
    let arg: JObject = match value {
        Some(v) => match env.new_string(v) {
            Ok(s) => JObject::from(s),
            Err(e) => {
                log::warn!("[backend-android] setContentDescription new_string failed: {e}");
                let _ = env.exception_clear();
                return;
            }
        },
        None => JObject::null(),
    };
    let _ = env.call_method(
        view,
        "setContentDescription",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&arg)],
    );
    clear_exception(env);
}

/// `setTooltipText(CharSequence)` (API 26+). NoSuchMethodError on
/// older devices is caught + cleared so the call site stays uniform.
fn set_tooltip_text(env: &mut JNIEnv, view: &JObject, value: Option<&str>) {
    let arg: JObject = match value {
        Some(v) => match env.new_string(v) {
            Ok(s) => JObject::from(s),
            Err(e) => {
                let _ = env.exception_clear();
                log::warn!("[backend-android] setTooltipText new_string failed: {e}");
                return;
            }
        },
        None => JObject::null(),
    };
    let _ = env.call_method(
        view,
        "setTooltipText",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&arg)],
    );
    clear_exception(env);
}

/// `setAccessibilityPaneTitle(CharSequence)` (API 28+). Surfaces the
/// author's `props.identifier` as a pane title — the closest match
/// Android exposes on plain View for "this element has a stable
/// external identifier". NoSuchMethodError on older devices is
/// caught + cleared.
fn set_accessibility_pane_title(env: &mut JNIEnv, view: &JObject, value: Option<&str>) {
    let arg: JObject = match value {
        Some(v) => match env.new_string(v) {
            Ok(s) => JObject::from(s),
            Err(e) => {
                let _ = env.exception_clear();
                log::warn!("[backend-android] setAccessibilityPaneTitle new_string failed: {e}");
                return;
            }
        },
        None => JObject::null(),
    };
    let _ = env.call_method(
        view,
        "setAccessibilityPaneTitle",
        "(Ljava/lang/CharSequence;)V",
        &[JValue::Object(&arg)],
    );
    clear_exception(env);
}

/// Drain any pending Java exception. JNI leaves a thread-local
/// "exception in flight" after a throwing call; the next JNI call
/// will fail if we don't clear it. We log+clear so the next setter
/// runs even if a prior one (e.g. `setTooltipText` on API 25) threw
/// NoSuchMethodError.
fn clear_exception(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

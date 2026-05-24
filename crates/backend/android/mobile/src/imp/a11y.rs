//! Android-mobile backend accessibility translation —
//! [`AccessibilityProps`] → `android.view.View` setter calls via JNI.
//!
//! Strategy mirrors the iOS / web backends: a single [`apply`]
//! function takes a node + resolved props and writes (or clears) every
//! relevant accessibility property. `create_*` paths call it after
//! constructing the view; the dynamic
//! [`update_accessibility`](runtime_core::Backend::update_accessibility)
//! path reuses it identically. Clearing on `None` is intentional —
//! reactive a11y prop changes must not leak stale labels onto a view.
//!
//! TalkBack walks the platform `View` tree directly via
//! `AccessibilityNodeInfo`, so the Android backend doesn't maintain a
//! parallel semantics tree;
//! [`dump_accessibility_tree`](runtime_core::Backend::dump_accessibility_tree)
//! stays `None` for this backend.
//!
//! ### API-level gating
//!
//! Several setters here require API levels above the framework's
//! minSdk floor (24/N). We call them unconditionally and rely on JNI's
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
//! custom `AccessibilityDelegate`. For v1 we use the View-level
//! setters that exist (label / hint / hidden / live region / heading
//! / selected / enabled), and fold remaining state flags
//! (CHECKED / EXPANDED / COLLAPSED / REQUIRED / READONLY / INVALID /
//! MIXED / BUSY) into the `contentDescription` tail string so TalkBack
//! still announces them. A future Kotlin `IdealystA11y` delegate shim
//! could promote these to `setStateDescription` (API 30+) and
//! `setRoleDescription` for richer TalkBack output — kept out of v1
//! to land the cross-platform contract without a Kotlin runtime
//! change.
//!
//! ### Live regions
//!
//! `View.setAccessibilityLiveRegion(int)` exists at View level (API
//! 19+). `Polite` → `ACCESSIBILITY_LIVE_REGION_POLITE (1)`,
//! `Assertive` → `ACCESSIBILITY_LIVE_REGION_ASSERTIVE (2)`. The
//! framework walker re-applies the label on signal change; the
//! TalkBack engine observes the mutation and announces with the
//! configured priority.

use runtime_core::accessibility::{
    AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

use super::with_env;

// ---------------------------------------------------------------------------
// Android constants.
// ---------------------------------------------------------------------------

/// `View.IMPORTANT_FOR_ACCESSIBILITY_AUTO` — let the platform decide.
/// We use this as the "no override" baseline so the platform's
/// heuristic for unlabelled containers (drop them from the tree)
/// continues to work.
const IMPORTANT_AUTO: i32 = 0;
/// `View.IMPORTANT_FOR_ACCESSIBILITY_YES` — force TalkBack focus
/// regardless of subtree content. We flip to this when the author
/// supplied a `label` or when the resolved role implies a focusable
/// element (Button, Link, Slider, …).
const IMPORTANT_YES: i32 = 1;
/// `View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS` — drop this
/// element AND every descendant from the a11y tree. The
/// "hide an entire subtree" semantics matches iOS
/// `accessibilityElementsHidden` and web `aria-hidden="true"`.
const IMPORTANT_NO_HIDE_DESCENDANTS: i32 = 4;

/// `View.ACCESSIBILITY_LIVE_REGION_NONE` (0) / `POLITE` (1) / `ASSERTIVE` (2).
/// Documented in the public Android SDK; the integer values are
/// stable across releases.
const LIVE_REGION_NONE: i32 = 0;
const LIVE_REGION_POLITE: i32 = 1;
const LIVE_REGION_ASSERTIVE: i32 = 2;

// ---------------------------------------------------------------------------
// Public surface.
// ---------------------------------------------------------------------------

/// Apply / refresh every accessibility property on `view` from `props`.
///
/// `inferred_role` is the primitive's default role (see
/// [`runtime_core::accessibility::default_role`]). If
/// `props.role.is_none()` and `inferred_role.is_some()`, the inferred
/// role drives the heading / role-description fallback; if both are
/// `None`, no role-derived bits are added (the caller's trait flags
/// still apply).
///
/// Idempotent: every property is either written or explicitly
/// cleared. Calling twice with the same `props` produces the same
/// View state; calling with different `props` always converges.
pub(crate) fn apply(node: &GlobalRef, props: &AccessibilityProps, inferred_role: Option<Role>) {
    with_env(|env| {
        apply_in_env(env, node, props, inferred_role);
    });
}

/// Variant for callers that already hold a `&mut JNIEnv` — avoids the
/// nested `with_env` re-attach cost when batching apply over many
/// nodes. The public `apply` entry point is the usual case; this
/// helper exists for future per-batch optimization without changing
/// the call sites.
pub(crate) fn apply_in_env(
    env: &mut JNIEnv,
    node: &GlobalRef,
    props: &AccessibilityProps,
    inferred_role: Option<Role>,
) {
    let view = node.as_obj();

    // Resolve role: explicit override wins.
    let resolved_role = props.role.or(inferred_role);

    // Compose the contentDescription string from label + traits-state
    // tail. TalkBack reads contentDescription verbatim, so the
    // composed string is the user-observable announcement.
    let label = compose_label(props, resolved_role);
    set_content_description(env, &view, label.as_deref());

    // Tooltip / hint — API 26+. The setter throws NoSuchMethodError on
    // older devices; we swallow it (see module doc on API-level
    // gating).
    set_tooltip_text(env, &view, props.hint.as_deref());

    // Identifier — Android has no first-class `accessibilityIdentifier`
    // on plain `View`. We surface it via `setAccessibilityPaneTitle`
    // (API 28+) so UIAutomator / on-device test harnesses that walk
    // for "pane title" can find the element. On older API levels the
    // call is a no-op (NoSuchMethodError swallowed).
    set_accessibility_pane_title(env, &view, props.identifier.as_deref());

    // Important-for-accessibility: hidden > labelled/role-forced > auto.
    let important = if props.hidden {
        IMPORTANT_NO_HIDE_DESCENDANTS
    } else if props.label.is_some()
        || resolved_role.map(role_forces_focus).unwrap_or(false)
    {
        IMPORTANT_YES
    } else {
        IMPORTANT_AUTO
    };
    let _ = env.call_method(
        &view,
        "setImportantForAccessibility",
        "(I)V",
        &[JValue::Int(important)],
    );
    clear_exception(env);

    // Live region — written at the View level (API 19+).
    let live_value = match props.live_region {
        Some(LiveRegionPriority::Polite) => LIVE_REGION_POLITE,
        Some(LiveRegionPriority::Assertive) => LIVE_REGION_ASSERTIVE,
        None => {
            // UPDATES_FREQUENTLY trait acts as an implicit `polite`
            // live region when no explicit `live_region` is set —
            // matches the web backend's behavior.
            if props.traits.contains(AccessibilityTraits::UPDATES_FREQUENTLY) {
                LIVE_REGION_POLITE
            } else {
                LIVE_REGION_NONE
            }
        }
    };
    let _ = env.call_method(
        &view,
        "setAccessibilityLiveRegion",
        "(I)V",
        &[JValue::Int(live_value)],
    );
    clear_exception(env);

    // Heading — `View.setAccessibilityHeading(boolean)` (API 28+).
    // Cross-backend behavior is: TalkBack announces "heading" before
    // the label, parity with iOS `UIAccessibilityTraitHeader` and web
    // `role="heading"`. NoSuchMethodError on older devices is
    // swallowed and the element falls back to a plain text node, same
    // graceful-degradation policy as tooltip.
    let is_heading = matches!(resolved_role, Some(Role::Header));
    let _ = env.call_method(
        &view,
        "setAccessibilityHeading",
        "(Z)V",
        &[JValue::Bool(if is_heading { 1 } else { 0 })],
    );
    clear_exception(env);

    // SELECTED — `View.setSelected(boolean)`. Note: this also affects
    // the View's visual selected state (drawable state set), which is
    // the intended behavior — Tab / ListItem styling drives off the
    // same flag, so a11y and visual selection stay in sync.
    let _ = env.call_method(
        &view,
        "setSelected",
        "(Z)V",
        &[JValue::Bool(
            if props.traits.contains(AccessibilityTraits::SELECTED) {
                1
            } else {
                0
            },
        )],
    );
    clear_exception(env);

    // DISABLED — `View.setEnabled(boolean)`. Affects visual state too
    // (greyed-out drawable). Only flip when the trait is actually set
    // so we don't enable a View the author explicitly disabled
    // elsewhere; pass `true` (enabled) when DISABLED is NOT set, but
    // only for views where the framework owns enabled-state. The
    // `set_disabled` backend trait method covers the explicit
    // author-driven disable path; here we only PROPAGATE the trait
    // flag, never override `false → true` (which would re-enable a
    // view some other layer disabled). Compromise: write `false`
    // unconditionally when DISABLED is on; leave `true` alone otherwise.
    if props.traits.contains(AccessibilityTraits::DISABLED) {
        let _ = env.call_method(
            &view,
            "setEnabled",
            "(Z)V",
            &[JValue::Bool(0)],
        );
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

// ---------------------------------------------------------------------------
// Label composition.
// ---------------------------------------------------------------------------

/// Build the TalkBack-visible contentDescription: author label first,
/// then a comma-separated tail of state flags that Android has no
/// first-class setter for (CHECKED, EXPANDED, COLLAPSED, REQUIRED,
/// READONLY, INVALID, MIXED, BUSY). `None` means "no override" —
/// TalkBack falls back to the View's natural announcement.
///
/// Per-flag string keys are English literals for v1. Localization
/// belongs to the framework's i18n layer (not wired through to a11y
/// yet); when it lands, this function should pull the strings from
/// the host's resource table instead.
fn compose_label(props: &AccessibilityProps, _role: Option<Role>) -> Option<String> {
    let base = props.label.clone();
    let tail = state_tail(props.traits);

    match (base, tail) {
        (None, None) => None,
        (Some(b), None) => Some(b),
        (None, Some(t)) => Some(t),
        (Some(b), Some(t)) => Some(format!("{b}, {t}")),
    }
}

/// Tail string assembled from state flags that don't have a
/// first-class Android View setter. Kept terse — TalkBack reads the
/// whole `contentDescription`, so verbose phrasing here would clog
/// the announcement.
fn state_tail(traits: AccessibilityTraits) -> Option<String> {
    let mut parts: Vec<&'static str> = Vec::new();
    // MIXED supersedes CHECKED (tri-state checkbox).
    if traits.contains(AccessibilityTraits::MIXED) {
        parts.push("mixed");
    } else if traits.contains(AccessibilityTraits::CHECKED) {
        parts.push("checked");
    }
    if traits.contains(AccessibilityTraits::EXPANDED) {
        parts.push("expanded");
    } else if traits.contains(AccessibilityTraits::COLLAPSED) {
        parts.push("collapsed");
    }
    if traits.contains(AccessibilityTraits::BUSY) {
        parts.push("busy");
    }
    if traits.contains(AccessibilityTraits::REQUIRED) {
        parts.push("required");
    }
    if traits.contains(AccessibilityTraits::READONLY) {
        parts.push("read only");
    }
    if traits.contains(AccessibilityTraits::INVALID) {
        parts.push("invalid");
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

/// Does this role imply the element should be marked
/// `importantForAccessibility = YES` even without an explicit label?
/// Matches the iOS `role_forces_accessibility_element` policy — roles
/// that should *be* the focused element get forced into the AX tree.
fn role_forces_focus(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::Link
            | Role::NavigationLink
            | Role::Image
            | Role::Header
            | Role::SearchField
            | Role::Slider
            | Role::Switch
            | Role::Checkbox
            | Role::RadioButton
            | Role::Tab
            | Role::ProgressBar
            | Role::Spinner
            | Role::MenuItem
    )
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_tail_empty_traits_is_none() {
        let traits = AccessibilityTraits::default();
        assert!(state_tail(traits).is_none());
    }

    #[test]
    fn state_tail_checked_and_expanded() {
        let traits = AccessibilityTraits::CHECKED | AccessibilityTraits::EXPANDED;
        assert_eq!(state_tail(traits).as_deref(), Some("checked, expanded"));
    }

    #[test]
    fn state_tail_mixed_supersedes_checked() {
        let traits = AccessibilityTraits::MIXED | AccessibilityTraits::CHECKED;
        assert_eq!(state_tail(traits).as_deref(), Some("mixed"));
    }

    #[test]
    fn compose_label_combines_base_and_tail() {
        let props = AccessibilityProps {
            label: Some("Submit".into()),
            traits: AccessibilityTraits::BUSY,
            ..Default::default()
        };
        assert_eq!(
            compose_label(&props, None).as_deref(),
            Some("Submit, busy")
        );
    }

    #[test]
    fn compose_label_no_base_only_tail() {
        let props = AccessibilityProps {
            traits: AccessibilityTraits::REQUIRED,
            ..Default::default()
        };
        assert_eq!(compose_label(&props, None).as_deref(), Some("required"));
    }

    #[test]
    fn compose_label_default_props_is_none() {
        let props = AccessibilityProps::default();
        assert!(compose_label(&props, None).is_none());
    }

    #[test]
    fn role_forces_focus_button() {
        assert!(role_forces_focus(Role::Button));
        assert!(!role_forces_focus(Role::Text));
        assert!(!role_forces_focus(Role::Group));
    }
}

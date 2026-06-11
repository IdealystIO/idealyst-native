//! iOS-mobile backend accessibility translation —
//! [`AccessibilityProps`] → UIKit `UIAccessibility*` setters on every
//! `UIView` we create.
//!
//! Strategy mirrors the web backend's `a11y.rs`: a single [`apply`]
//! function takes a node + resolved props and writes (or clears) every
//! relevant UIAccessibility property. All `create_*` paths call it
//! after constructing the view; the dynamic
//! [`update_accessibility`](runtime_core::Backend::update_accessibility)
//! path reuses it identically. Clearing on `None` is intentional —
//! reactive a11y prop changes must not leak stale labels onto a view.
//!
//! UIKit walks each `UIView`'s accessibility properties directly, so
//! we don't maintain a parallel semantics tree and
//! [`dump_accessibility_tree`](runtime_core::Backend::dump_accessibility_tree)
//! stays `None` for this backend.
//!
//! ### Live regions
//!
//! UIKit has no per-view "aria-live"-equivalent setter — live updates
//! are imperative via `UIAccessibilityPostNotification(.announcement,
//! …)`. The `props.live_region` field is therefore observed at the
//! framework layer (the walker re-announces when a live-region label
//! changes) and routed through [`announce`] here; [`apply`] itself
//! leaves it untouched.
//!
//! ### Trait mapping subtleties
//!
//! - `CHECKED` has no first-class `UIAccessibilityTrait`. We expose
//!   the state via `accessibilityValue` ("1" / "0") so VoiceOver
//!   announces "selected" / "not selected" without the framework
//!   having to plant a custom action. `MIXED` becomes the value
//!   string "mixed" by the same logic.
//! - `EXPANDED` / `COLLAPSED` similarly have no UIKit trait. We fold
//!   them into the accessibility value string ("expanded" /
//!   "collapsed") so screen-reader announcements stay aligned with
//!   web ARIA semantics.
//! - `BUSY` and `UPDATES_FREQUENTLY` both map to
//!   `UIAccessibilityTraitUpdatesFrequently`. That's what UIKit
//!   exposes — `aria-busy="true"` doesn't have a closer equivalent on
//!   iOS.

use runtime_core::accessibility::{
    AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
use objc2::msg_send;
use objc2_foundation::NSString;
use objc2_ui_kit::UIView;

use crate::imp::IosNode;

/// Apply / refresh every UIAccessibility property on `node` from
/// `props`.
///
/// `inferred_role` is the primitive's default role (see
/// [`runtime_core::accessibility::default_role`]). If
/// `props.role.is_none()` and `inferred_role.is_some()`, the inferred
/// role is used to derive the UIAccessibilityTraits bag; if both are
/// `None`, no role-derived bits are added (the caller's traits flags
/// still apply).
///
/// Idempotent: every property is either written or explicitly cleared.
/// Calling twice with the same `props` produces the same UIKit state;
/// calling with different `props` always converges.
pub(crate) fn apply(node: &IosNode, props: &AccessibilityProps, inferred_role: Option<Role>) {
    let view: &UIView = node.as_view();

    // Resolve role: explicit override wins.
    let resolved_role = props.role.or(inferred_role);

    // Label / hint / identifier.
    set_string_or_clear(view, sel_set_accessibility_label(), props.label.as_deref());
    set_string_or_clear(view, sel_set_accessibility_hint(), props.hint.as_deref());
    set_identifier_or_clear(view, props.identifier.as_deref());

    // Hidden: `accessibilityElementsHidden` hides this view *and all
    // descendants* from the AX tree. The plain
    // `isAccessibilityElement = false` flag is the per-element opt-out
    // — we don't toggle it for `hidden`; instead we use the container-
    // wide hider so transparent View groups stay walked when not
    // explicitly hidden.
    let _: () = unsafe { msg_send![view, setAccessibilityElementsHidden: props.hidden] };

    // `isAccessibilityElement` is forced ON for elements that have an
    // author-supplied label OR a UIKit-implicit trait variant (button,
    // link, image, header, search, slider, spinner, …). For unlabelled
    // generic containers (View with no label), leave it as UIKit's
    // default — UIView defaults to `false` and lets UIKit walk into
    // children; UILabel/UIButton/UISwitch default to `true`.
    let role_forces_element = resolved_role
        .map(role_forces_accessibility_element)
        .unwrap_or(false);
    if props.label.is_some() || role_forces_element {
        let _: () = unsafe { msg_send![view, setIsAccessibilityElement: true] };
    }

    // Accessibility traits — OR together role-derived bits and per-
    // element state flags.
    let traits_bits = traits_to_uikit_bits(resolved_role, props.traits);
    let _: () = unsafe { msg_send![view, setAccessibilityTraits: traits_bits] };

    // Value. Used for CHECKED/MIXED/EXPANDED/COLLAPSED — none of which
    // have a first-class UIKit trait. Set to the canonical announce
    // string; clear when the flag isn't set.
    let value = derive_accessibility_value(props.traits);
    set_string_or_clear(view, sel_set_accessibility_value(), value);
}

/// `setAccessibilityViewIsModal:` — used by the portal/dialog code
/// path. Exposed as a helper so portal callers don't have to repeat
/// the unsafe-msg_send dance. Kept here so all UIAccessibility wiring
/// lives in one place.
#[allow(dead_code)]
pub(crate) fn set_modal(view: &UIView, modal: bool) {
    let _: () = unsafe { msg_send![view, setAccessibilityViewIsModal: modal] };
}

// ---------------------------------------------------------------------------
// Role → UIAccessibilityTraits + per-element-flag mapping.
// ---------------------------------------------------------------------------

/// Map a [`Role`] to the UIAccessibilityTraits bits it contributes.
/// Matches the table in `docs/accessibility-design.md` §1.
///
/// `Role` is `#[non_exhaustive]`; unmapped variants contribute zero
/// (no trait) rather than panicking — safer than guessing when UIKit
/// has no obvious equivalent for a future role.
fn role_to_traits_bits(role: Role) -> u64 {
    // UIAccessibilityTrait* bit values are stable across iOS releases
    // (documented in `<UIKit/UIAccessibilityConstants.h>`). Listed
    // inline here as bit positions so the mapping table is greppable;
    // we could pull `objc2_ui_kit::UIAccessibilityTraitButton` etc.,
    // but those are extern statics that require a runtime load.
    const BUTTON: u64 = 1 << 0;
    const LINK: u64 = 1 << 1;
    const IMAGE: u64 = 1 << 2;
    const SEARCH_FIELD: u64 = 1 << 7;
    const ADJUSTABLE: u64 = 1 << 4; // slider
    const UPDATES_FREQUENTLY: u64 = 1 << 9; // progress / spinner
    const HEADER: u64 = 1 << 28;
    const TAB_BAR: u64 = 1 << 36;
    const TOGGLE_BUTTON: u64 = 1 << 37;
    const STATIC_TEXT: u64 = 1 << 8;

    match role {
        Role::Button => BUTTON,
        Role::Link | Role::NavigationLink => LINK,
        Role::Image => IMAGE,
        Role::Header => HEADER,
        Role::SearchField => SEARCH_FIELD,
        Role::Slider => ADJUSTABLE,
        Role::Switch | Role::Checkbox | Role::RadioButton => TOGGLE_BUTTON,
        Role::ProgressBar | Role::Spinner => UPDATES_FREQUENTLY,
        Role::Tab => BUTTON, // SELECTED bit (added separately by traits) lifts to "selected tab"
        Role::TabList => TAB_BAR,
        // Static-text roles announce as text without a "button" hint.
        Role::Text => STATIC_TEXT,
        // Roles with no first-class UIKit trait — UIKit walks the view
        // and announces label/value without role-specific framing.
        Role::TextField
        | Role::TextArea
        | Role::ComboBox
        | Role::List
        | Role::ListItem
        | Role::Group
        | Role::Separator
        | Role::RadioGroup
        | Role::TabPanel
        | Role::MenuItem
        | Role::Menu
        | Role::MenuBar
        | Role::Toolbar
        | Role::Alert
        | Role::Status
        | Role::Dialog
        | Role::AlertDialog
        | Role::Drawer
        | Role::Popover
        | Role::Tooltip
        | Role::Region => 0,
        // `Role` is non_exhaustive — future variants get UIAccessibilityTraitNone.
        _ => 0,
    }
}

/// Does this role imply the element should be marked
/// `isAccessibilityElement = true`? UIKit defaults
/// `UIView.isAccessibilityElement` to `false` so VoiceOver walks
/// into children. For roles that should *be* the focused element
/// (Button, Link, Image, …) we force it on even when the underlying
/// view class (UIView, not UIButton) would otherwise default to off.
fn role_forces_accessibility_element(role: Role) -> bool {
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

/// Combine role-derived + per-element trait bits.
fn traits_to_uikit_bits(role: Option<Role>, traits: AccessibilityTraits) -> u64 {
    const NOT_ENABLED: u64 = 1 << 6;
    const SELECTED: u64 = 1 << 3;
    const UPDATES_FREQUENTLY: u64 = 1 << 9;

    let mut bits = role.map(role_to_traits_bits).unwrap_or(0);

    if traits.contains(AccessibilityTraits::DISABLED) {
        bits |= NOT_ENABLED;
    }
    if traits.contains(AccessibilityTraits::SELECTED) {
        bits |= SELECTED;
    }
    if traits.contains(AccessibilityTraits::BUSY)
        || traits.contains(AccessibilityTraits::UPDATES_FREQUENTLY)
    {
        bits |= UPDATES_FREQUENTLY;
    }
    // CHECKED / MIXED / EXPANDED / COLLAPSED / REQUIRED / READONLY /
    // INVALID have no first-class UIAccessibilityTrait. They route
    // through `accessibilityValue` (see `derive_accessibility_value`).
    bits
}

/// Translate a small set of state flags into the
/// `accessibilityValue` string. VoiceOver reads this aloud after the
/// label ("Notifications, on" / "Tree node, expanded"), which is the
/// closest user-observable parity with web's `aria-checked` /
/// `aria-expanded` state announcements.
fn derive_accessibility_value(traits: AccessibilityTraits) -> Option<&'static str> {
    if traits.contains(AccessibilityTraits::MIXED) {
        Some("mixed")
    } else if traits.contains(AccessibilityTraits::CHECKED) {
        Some("1")
    } else if traits.contains(AccessibilityTraits::EXPANDED) {
        Some("expanded")
    } else if traits.contains(AccessibilityTraits::COLLAPSED) {
        Some("collapsed")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// announce_for_accessibility — UIAccessibility.post(notification:…).
// ---------------------------------------------------------------------------

/// Post a one-shot accessibility announcement via UIKit's global
/// notification channel.
///
/// VoiceOver reads `msg` aloud immediately if no other speech is in
/// flight; otherwise it queues behind the current utterance.
///
/// **iOS 17+**: attach
/// `UIAccessibilitySpeechAttributeAnnouncementPriority` (an
/// `NSAttributedStringKey` whose values are the
/// `UIAccessibilityPriorityHigh` / `UIAccessibilityPriorityDefault` /
/// `UIAccessibilityPriorityLow` strings) to an `NSAttributedString`
/// argument, mapping:
///   - `Polite`    → `UIAccessibilityPriorityDefault`
///   - `Assertive` → `UIAccessibilityPriorityHigh`
/// This matches AppKit's `NSAccessibilityPriorityKey` (Polite→Medium,
/// Assertive→High) on macOS — both backends converge on the same
/// observable VoiceOver behavior per Rule 7 of CLAUDE.md.
///
/// **iOS < 17**: fall back to posting a plain `NSString`. iOS ignores
/// any priority attribute we'd attach, so we keep the legacy code
/// path bit-for-bit identical and `Polite` / `Assertive` collapse to
/// the same announcement (which is what older releases already did).
///
/// ### Runtime version check
///
/// We use `NSProcessInfo.isOperatingSystemAtLeastVersion:` instead of
/// a compile-time `#[cfg(...)]`. The deployment target's `iphoneos`
/// SDK version is **not** the OS version of the device the build runs
/// on (Mac Catalyst, App Store distribution to older devices, etc.),
/// so a static cfg would either lock us out of the iOS-17 API or
/// promise it to OS releases that don't have it. The runtime check is
/// also what Apple recommends for `@available` parity in Swift.
pub(crate) fn announce(msg: &str, priority: LiveRegionPriority) {
    use objc2_foundation::{NSProcessInfo, NSRange};
    use objc2_ui_kit::{UIAccessibilityAnnouncementNotification, UIAccessibilityPostNotification};

    let ns_msg = NSString::from_str(msg);

    // iOS 17.0 introduced UIAccessibilitySpeechAttributeAnnouncementPriority.
    // `isOperatingSystemAtLeastVersion:` is on NSProcessInfo since
    // iOS 8 — safe to call unconditionally.
    let version = objc2_foundation::NSOperatingSystemVersion {
        majorVersion: 17,
        minorVersion: 0,
        patchVersion: 0,
    };
    // SAFETY: NSProcessInfo singleton is documented thread-safe; the
    // version-compare selector has been stable since iOS 8.
    let is_ios_17_plus = unsafe {
        NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(version)
    };

    if is_ios_17_plus {
        // iOS 17+ path: build an NSMutableAttributedString with the
        // priority attribute over the full range. UIKit looks for the
        // attribute on the announcement argument and routes the
        // utterance through VoiceOver's priority queue.
        use objc2_foundation::NSMutableAttributedString;
        use objc2_ui_kit::{
            UIAccessibilityPriorityDefault, UIAccessibilityPriorityHigh,
            UIAccessibilitySpeechAttributeAnnouncementPriority,
        };

        // alloc + init on a concrete subclass; result is a
        // freshly-retained NSMutableAttributedString we own.
        // `ClassType::alloc` is the objc2 trait method that returns
        // a typed `Allocated<Self>` for the subsequent init call.
        // `initWithString:` is safe-bridge'd by objc2, so no unsafe
        // block is needed at this call site.
        use objc2::ClassType;
        let mut attr = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &ns_msg,
        );

        // The priority value is itself an NSString constant exported
        // by UIKit (`UIAccessibilityPriority` is an
        // `NS_TYPED_ENUM(NSString *)` typedef — alias for NSString).
        // The extern static deref yields `&'static NSString`; NSString
        // IS-A NSObject IS-A AnyObject, so we just take a borrow of
        // the underlying object for the `addAttribute:value:range:` call.
        //
        // We don't reach this branch on pre-iOS-17 where the symbols
        // might be absent — UIKit added these statics in iOS 17.
        // SAFETY: dynamic-linker-resolved NSString singletons; on iOS
        // 17+ both are guaranteed non-null and live for the process
        // lifetime. The version gate above keeps us out on releases
        // where the symbols haven't been defined.
        let priority_str: &NSString = unsafe {
            match priority {
                LiveRegionPriority::Polite => UIAccessibilityPriorityDefault,
                LiveRegionPriority::Assertive => UIAccessibilityPriorityHigh,
            }
        };
        let priority_value: &objc2::runtime::AnyObject = &*priority_str;

        // Apply across the full string. `NSMutableAttributedString`
        // measures length in UTF-16 code units (matching `NSString.length`).
        let len = attr.length();
        let range = NSRange { location: 0, length: len };

        // SAFETY: `key` is the UIKit-exported NSString constant (valid
        // for process lifetime). `priority_value` is the same.
        // `range` covers exactly `[0, len)` of the mutable string we
        // just built; no other thread holds a reference.
        unsafe {
            attr.addAttribute_value_range(
                UIAccessibilitySpeechAttributeAnnouncementPriority,
                priority_value,
                range,
            );
        }

        // SAFETY: extern C call into UIKit. The notification static
        // is loaded by the dynamic linker; `&*attr` is a borrowed
        // NSObject pointer (NSAttributedString IS-A NSObject) valid
        // for the call's lifetime. UIKit copies the argument.
        unsafe {
            UIAccessibilityPostNotification(
                UIAccessibilityAnnouncementNotification,
                Some(&*attr as &objc2::runtime::AnyObject),
            );
        }
    } else {
        // iOS < 17 path: plain NSString argument. The notification
        // copies the argument string immediately; we don't need to
        // retain it across the call. Priority is ignored by UIKit on
        // these releases, matching the historical behavior.
        let _ = priority;
        // SAFETY: extern C call into UIKit. `UIAccessibilityAnnouncementNotification`
        // is a static loaded once by the dynamic linker; `&*ns_msg`
        // is a borrowed NSObject pointer valid for the call's lifetime.
        unsafe {
            UIAccessibilityPostNotification(
                UIAccessibilityAnnouncementNotification,
                Some(&*ns_msg as &objc2::runtime::AnyObject),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------

/// Write `value` via the given setter selector, or clear it (pass
/// nil) when `value` is `None`. Both `setAccessibilityLabel:` and
/// `setAccessibilityHint:` accept `nil` to revert to the platform
/// default; this is what makes reactive prop updates that clear a
/// previously-set field actually drop the value.
fn set_string_or_clear(view: &UIView, sel: objc2::runtime::Sel, value: Option<&str>) {
    // `performSelector:withObject:` is declared as returning `id`
    // ('@') on NSObject regardless of the underlying target's
    // return type, so the Rust receiver type has to be a pointer
    // — even though the actual selectors we send (the
    // `setAccessibility*:` family) are void-returning. Pre-fix
    // this used `let _: () = ...`, which made objc2's debug-mode
    // signature verifier panic ("expected return to have type
    // code '@', but found 'v'") the first time runtime-server-mode walk
    // hit an a11y label — taking down the entire 74-command
    // initial-snapshot apply on the first view it processed, so
    // the iOS shell ended up with just the root view registered
    // and every subsequent SetAnimated command referenced a
    // never-created node. The returned pointer is meaningless for
    // these setters; we ignore it.
    type Id = *mut objc2_foundation::NSObject;
    match value {
        Some(v) => {
            let ns = NSString::from_str(v);
            let _: Id = unsafe { msg_send![view, performSelector: sel, withObject: &*ns] };
        }
        None => {
            let nil: *const objc2_foundation::NSObject = std::ptr::null();
            let _: Id =
                unsafe { msg_send![view, performSelector: sel, withObject: nil] };
        }
    }
}

/// `setAccessibilityIdentifier:` — uses the same nil-on-None
/// semantics as the label/hint setters above. Pulled into its own
/// function because `accessibilityIdentifier` lives on the
/// `UIAccessibilityIdentification` informal protocol, but it's
/// implemented by every `UIView` so `performSelector:` finds it.
fn set_identifier_or_clear(view: &UIView, value: Option<&str>) {
    set_string_or_clear(view, sel_set_accessibility_identifier(), value);
}

fn sel_set_accessibility_label() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityLabel:)
}
fn sel_set_accessibility_hint() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityHint:)
}
fn sel_set_accessibility_value() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityValue:)
}
fn sel_set_accessibility_identifier() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityIdentifier:)
}

#[cfg(test)]
mod tests {
    //! Pure trait/state-mapping coverage. Mirrors the macOS/web/Android
    //! a11y role tests — guards a `Role`/`AccessibilityTraits` variant
    //! silently losing its UIKit mapping. These helpers touch no UIKit,
    //! but the crate only links against iOS, so this runs under an iOS
    //! test runner (sim/device), not host `cargo test`.
    use super::*;

    #[test]
    fn role_maps_to_expected_trait_bits() {
        // Bit positions are the documented UIAccessibilityTrait* values.
        assert_eq!(role_to_traits_bits(Role::Button), 1 << 0);
        assert_eq!(role_to_traits_bits(Role::Link), 1 << 1);
        assert_eq!(role_to_traits_bits(Role::Image), 1 << 2);
        assert_eq!(role_to_traits_bits(Role::Slider), 1 << 4); // adjustable
        assert_eq!(role_to_traits_bits(Role::Header), 1 << 28);
        assert_eq!(role_to_traits_bits(Role::Switch), 1 << 37); // toggle button
        // Roles with no first-class UIKit trait map to 0.
        assert_eq!(role_to_traits_bits(Role::Group), 0);
    }

    #[test]
    fn forces_accessibility_element_for_focusable_roles() {
        assert!(role_forces_accessibility_element(Role::Button));
        assert!(role_forces_accessibility_element(Role::Slider));
        assert!(!role_forces_accessibility_element(Role::Group));
        assert!(!role_forces_accessibility_element(Role::Text));
    }

    #[test]
    fn accessibility_value_reflects_state_traits() {
        assert_eq!(
            derive_accessibility_value(AccessibilityTraits::CHECKED),
            Some("1")
        );
        assert_eq!(
            derive_accessibility_value(AccessibilityTraits::EXPANDED),
            Some("expanded")
        );
        assert_eq!(derive_accessibility_value(AccessibilityTraits::empty()), None);
        // MIXED wins over CHECKED (the precedence in the helper).
        assert_eq!(
            derive_accessibility_value(
                AccessibilityTraits::MIXED | AccessibilityTraits::CHECKED
            ),
            Some("mixed")
        );
    }
}

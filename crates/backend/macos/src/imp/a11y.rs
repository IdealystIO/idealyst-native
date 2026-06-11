//! macOS backend accessibility translation —
//! [`AccessibilityProps`] → AppKit `NSAccessibility` per-attribute setters
//! on every `NSView` we create.
//!
//! Strategy mirrors the iOS backend's `a11y.rs`: a single [`apply`]
//! function takes a node + resolved props and writes (or clears) every
//! relevant NSAccessibility property. All `create_*` paths call it
//! after constructing the view; the dynamic
//! [`update_accessibility`](runtime_core::Backend::update_accessibility)
//! path reuses it identically. Clearing on `None` is intentional —
//! reactive a11y prop changes must not leak stale labels onto a view.
//!
//! AppKit walks each `NSView`'s NSAccessibility properties directly
//! (`accessibilityLabel`, `accessibilityRole`, …), so we don't maintain
//! a parallel semantics tree and
//! [`dump_accessibility_tree`](runtime_core::Backend::dump_accessibility_tree)
//! stays `None` for this backend.
//!
//! ### AppKit vs UIKit subtleties (see [[project_macos_appkit_uikit_diffs]])
//!
//! - **Role + subrole, not traits.** UIKit packs a bitfield of "traits"
//!   onto every view; AppKit instead exposes a single string `role`
//!   plus an optional `subrole`. Most of our role mappings are direct
//!   role assignments. The handful of state-like flags UIKit traits
//!   encode (`SELECTED`, `DISABLED`, `BUSY`) route through dedicated
//!   per-attribute setters on AppKit (`setAccessibilitySelected:`,
//!   `setAccessibilityEnabled:`, etc.) rather than into the role.
//! - **Subrole hosts the `Switch` / `Header` / `Tab` / `SearchField` /
//!   `Dialog` flavors** on top of the closest base role (CheckBox,
//!   StaticText, RadioButton, TextField, Window). NSAccessibility
//!   announces "switch button", "heading", "tab", etc. when the
//!   subrole is set — without one, VoiceOver only announces the base
//!   role.
//! - **`hidden` is `setAccessibilityElement:NO`,** not a separate
//!   "hidden" attribute. AppKit doesn't expose an analog of UIKit's
//!   `accessibilityElementsHidden`; instead an element either *is* an
//!   AX element or it isn't. Removing the element from the AX tree
//!   has the same observable effect for descendants because AX walks
//!   through transparent containers.
//! - **No first-class `BUSY` attribute.** AppKit's busy state lives on
//!   `NSProgressIndicator` instances (their `indeterminate` property);
//!   there's no per-view "I am busy" marker on a generic NSView. We
//!   document the trait as unsupported on this backend rather than
//!   plant a custom attribute that VoiceOver wouldn't recognize.
//!
//! ### Live regions
//!
//! AppKit has no per-view "aria-live"-equivalent setter — live updates
//! are imperative via `NSAccessibilityPostNotificationWithUserInfo(
//! …, .announcementRequested, [.announcement: msg, .priority: priority])`.
//! The `props.live_region` field is therefore observed at the
//! framework layer and routed through [`announce`] here; [`apply`]
//! itself leaves it untouched.
//!
//! ### Trait mapping subtleties (mirrors iOS)
//!
//! - `CHECKED` has no first-class NSAccessibility attribute. We expose
//!   the state via `accessibilityValue` ("1" / "0") so VoiceOver
//!   announces "checked" / "unchecked" without the framework having to
//!   plant a custom action. `MIXED` becomes the value string "mixed";
//!   `EXPANDED` / `COLLAPSED` ride through "expanded" / "collapsed"
//!   so screen-reader announcements stay aligned with web ARIA
//!   semantics.

use runtime_core::accessibility::{
    AccessibilityProps, AccessibilityTraits, LiveRegionPriority, Role,
};
use objc2::msg_send;
use objc2_app_kit::NSView;
use objc2_foundation::NSString;

use crate::imp::MacosNode;

/// Apply / refresh every NSAccessibility property on `node` from
/// `props`.
///
/// `inferred_role` is the primitive's default role (see
/// [`runtime_core::accessibility::default_role`]). If
/// `props.role.is_none()` and `inferred_role.is_some()`, the inferred
/// role is used to pick the NSAccessibility role string; if both are
/// `None`, no role override is written (the view keeps whatever role
/// its AppKit superclass already advertises — `NSTextField` →
/// `.staticText`, generic `NSView` → none / inferred by AX walker).
///
/// Idempotent: every property is either written or explicitly cleared.
/// Calling twice with the same `props` produces the same AppKit state;
/// calling with different `props` always converges.
pub(crate) fn apply(node: &MacosNode, props: &AccessibilityProps, inferred_role: Option<Role>) {
    let view: &NSView = node.as_view();

    // Resolve role: explicit override wins over inferred.
    let resolved_role = props.role.or(inferred_role);

    // Label / hint / identifier. AppKit's accessibilityLabel = UIKit's;
    // accessibilityHelp = UIKit's accessibilityHint. The platform AX
    // tools (VoiceOver, AX Inspector) speak both attributes.
    set_string_or_clear(view, sel_set_accessibility_label(), props.label.as_deref());
    set_string_or_clear(view, sel_set_accessibility_help(), props.hint.as_deref());
    set_string_or_clear(
        view,
        sel_set_accessibility_identifier(),
        props.identifier.as_deref(),
    );

    // Role + subrole. AppKit's `accessibilityRole` takes an NSString;
    // we pass the canonical NSAccessibility*Role constants when we
    // have a mapping. `setAccessibilitySubrole:nil` clears any prior
    // subrole — important because the same NSView may be reused across
    // reactive role changes (Tab → Button).
    if let Some(role) = resolved_role {
        if let Some(role_str) = role_to_ns_accessibility_role(role) {
            let _: () = unsafe { msg_send![view, setAccessibilityRole: role_str] };
        }
        let subrole = role_to_ns_accessibility_subrole(role);
        match subrole {
            Some(s) => {
                let _: () = unsafe { msg_send![view, setAccessibilitySubrole: s] };
            }
            None => {
                let nil: *const NSString = std::ptr::null();
                let _: () = unsafe { msg_send![view, setAccessibilitySubrole: nil] };
            }
        }
    }

    // Hidden — opt the element out of the AX tree entirely. AppKit
    // doesn't have a UIKit-equivalent "hide me AND my descendants"
    // attribute; the next-best is `setAccessibilityElement:NO`, which
    // drops this view from the AX walker. Descendants stay walkable
    // unless they're also hidden — this matches the documented design
    // (hidden = "decorative, no AX identity") and aligns with web's
    // `aria-hidden="true"` semantics where descendants inherit hide.
    let element_active = !props.hidden;
    let _: () = unsafe { msg_send![view, setAccessibilityElement: element_active] };

    // State flags. AppKit splits per-attribute (unlike UIKit's traits
    // bitfield).
    let _: () = unsafe {
        msg_send![
            view,
            setAccessibilityEnabled: !props.traits.contains(AccessibilityTraits::DISABLED)
        ]
    };
    let _: () = unsafe {
        msg_send![
            view,
            setAccessibilitySelected: props.traits.contains(AccessibilityTraits::SELECTED)
        ]
    };
    let _: () = unsafe {
        msg_send![
            view,
            setAccessibilityRequired: props.traits.contains(AccessibilityTraits::REQUIRED)
        ]
    };

    // Value — used for CHECKED/MIXED/EXPANDED/COLLAPSED. AppKit has
    // `setAccessibilityExpanded:` natively for the expanded state, so
    // route the expand/collapse bit through that and reserve the value
    // string for checkbox tri-state announcement (where AppKit has no
    // first-class attribute).
    let expanded = props.traits.contains(AccessibilityTraits::EXPANDED);
    let collapsed = props.traits.contains(AccessibilityTraits::COLLAPSED);
    if expanded || collapsed {
        let _: () = unsafe { msg_send![view, setAccessibilityExpanded: expanded] };
    }

    let value = derive_accessibility_value(props.traits);
    set_string_or_clear(view, sel_set_accessibility_value(), value);
}

// ---------------------------------------------------------------------------
// Role / subrole mapping (see docs/accessibility-design.md §1 macOS column).
// ---------------------------------------------------------------------------

/// Map a [`Role`] to the canonical `NSAccessibility*Role` string
/// constant. Returns the constant `NSString` from `objc2_app_kit`
/// (loaded once by the dynamic linker, lives for the process).
///
/// `Role` is `#[non_exhaustive]`; unmapped variants return `None`
/// rather than panicking — VoiceOver will fall back to the underlying
/// view class's role.
fn role_to_ns_accessibility_role(role: Role) -> Option<&'static NSString> {
    use objc2_app_kit::{
        NSAccessibilityButtonRole, NSAccessibilityCheckBoxRole, NSAccessibilityDrawerRole,
        NSAccessibilityGroupRole, NSAccessibilityImageRole, NSAccessibilityLinkRole,
        NSAccessibilityListRole, NSAccessibilityMenuBarRole, NSAccessibilityMenuItemRole,
        NSAccessibilityMenuRole, NSAccessibilityPopUpButtonRole, NSAccessibilityPopoverRole,
        NSAccessibilityProgressIndicatorRole, NSAccessibilityRadioButtonRole,
        NSAccessibilityRadioGroupRole, NSAccessibilityRowRole, NSAccessibilitySplitterRole,
        NSAccessibilityStaticTextRole, NSAccessibilityTabGroupRole, NSAccessibilityTextFieldRole,
        NSAccessibilityToolbarRole, NSAccessibilityWindowRole,
    };
    // SAFETY: each `NSAccessibility*Role` is a stable AppKit string
    // constant loaded once by the dynamic linker; reading them is
    // safe for the process lifetime. We wrap the whole match arm in
    // unsafe so the per-arm reads don't need individual unsafe
    // blocks. Each constant is a `&'static NSAccessibilityRole` (type
    // alias for `NSString`); we deref-coerce to `&'static NSString`
    // via the return type.
    Some(unsafe {
        match role {
            Role::Button => NSAccessibilityButtonRole,
            Role::Link | Role::NavigationLink => NSAccessibilityLinkRole,
            Role::Image => NSAccessibilityImageRole,
            Role::Text => NSAccessibilityStaticTextRole,
            // AppKit has no first-class heading role; the heading is a
            // subrole on `staticText` (see
            // `role_to_ns_accessibility_subrole`).
            Role::Header => NSAccessibilityStaticTextRole,
            Role::List => NSAccessibilityListRole,
            Role::ListItem => NSAccessibilityRowRole,
            Role::Group | Role::TabPanel | Role::Region => NSAccessibilityGroupRole,
            Role::Separator => NSAccessibilitySplitterRole,
            Role::TextField | Role::TextArea | Role::SearchField => NSAccessibilityTextFieldRole,
            // `Switch` is a checkbox with the `switch` subrole on
            // AppKit — see `role_to_ns_accessibility_subrole`.
            Role::Switch | Role::Checkbox => NSAccessibilityCheckBoxRole,
            Role::RadioButton | Role::Tab => NSAccessibilityRadioButtonRole,
            Role::RadioGroup => NSAccessibilityRadioGroupRole,
            Role::ComboBox => NSAccessibilityPopUpButtonRole,
            Role::TabList => NSAccessibilityTabGroupRole,
            Role::MenuItem => NSAccessibilityMenuItemRole,
            Role::Menu => NSAccessibilityMenuRole,
            Role::MenuBar => NSAccessibilityMenuBarRole,
            Role::Toolbar => NSAccessibilityToolbarRole,
            // ProgressIndicator covers both determinate and
            // indeterminate (Spinner) — AppKit's BusyIndicator role is
            // deprecated.
            Role::ProgressBar | Role::Spinner => NSAccessibilityProgressIndicatorRole,
            // Alert / Status announce via `announce` (no per-view role);
            // the visible view itself is `staticText` so VoiceOver
            // still reads it on focus.
            Role::Alert | Role::Status => NSAccessibilityStaticTextRole,
            // Dialog flavors all sit on `window` + the appropriate
            // subrole.
            Role::Dialog | Role::AlertDialog => NSAccessibilityWindowRole,
            Role::Drawer => NSAccessibilityDrawerRole,
            Role::Popover | Role::Tooltip => NSAccessibilityPopoverRole,
            // `Role` is `#[non_exhaustive]`; a future variant with no
            // mapped NS role falls back to no role (caller writes nil).
            _ => return None,
        }
    })
}

/// Map a [`Role`] to its NSAccessibility *subrole* string, when one is
/// needed to disambiguate the flavor on top of the base role. Returns
/// `None` for roles that don't need a subrole — caller writes `nil` to
/// clear any prior subrole on the same NSView.
fn role_to_ns_accessibility_subrole(role: Role) -> Option<&'static NSString> {
    use objc2_app_kit::{
        NSAccessibilitySearchFieldSubrole, NSAccessibilitySwitchSubrole,
        NSAccessibilitySystemDialogSubrole, NSAccessibilityTabButtonSubrole,
    };
    // SAFETY: AppKit subrole constants — same reasoning as
    // `role_to_ns_accessibility_role` above.
    Some(unsafe {
        match role {
            Role::Switch => NSAccessibilitySwitchSubrole,
            Role::SearchField => NSAccessibilitySearchFieldSubrole,
            Role::Tab => NSAccessibilityTabButtonSubrole,
            // AlertDialog → system dialog subrole; plain Dialog reads
            // as a regular window without a subrole. AppKit has no
            // public `AXHeading` subrole constant (it's a runtime-only
            // string in older AppKit headers), so `Header` rides as a
            // plain `staticText` role without subrole — VoiceOver
            // still announces the label, just without the "heading"
            // framing.
            Role::AlertDialog => NSAccessibilitySystemDialogSubrole,
            // `Role` is `#[non_exhaustive]`; a future variant with no
            // mapped subrole falls back to no subrole (caller writes nil).
            _ => return None,
        }
    })
}

/// Translate a small set of state flags into the
/// `accessibilityValue` string. VoiceOver reads this aloud after the
/// label (e.g. "Subscribe, checked"), matching the iOS routing in
/// [`crate::imp::a11y`]-equivalent on iOS and aligning with web's
/// `aria-checked` / `aria-expanded` state announcements.
///
/// Note: `EXPANDED`/`COLLAPSED` ride through here as a fallback only —
/// the primary path is `setAccessibilityExpanded:`. Keeping a value
/// string in addition means assistive tech that doesn't surface the
/// boolean attribute (older AT, scripts reading the value attribute)
/// still gets the state announcement.
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
// announce_for_accessibility — NSAccessibilityPostNotificationWithUserInfo.
// ---------------------------------------------------------------------------

/// Post a one-shot accessibility announcement via AppKit's global
/// notification channel.
///
/// VoiceOver reads `msg` aloud immediately when priority is `.high`,
/// or queues behind the current utterance for `.medium`. We use the
/// shared `NSApp` instance as the announcement element — Apple's
/// docs recommend the affected window for window-scoped announcements,
/// but global "form submitted" / "loading complete" announcements
/// don't have a stable focus target and the application object is the
/// established fallback.
///
/// userInfo dictionary shape (per AppKit headers):
///   - `NSAccessibilityAnnouncementKey` → `NSString` of the message
///   - `NSAccessibilityPriorityKey`     → `NSNumber` wrapping
///     `NSAccessibilityPriorityLevel`
pub(crate) fn announce(msg: &str, priority: LiveRegionPriority) {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSAccessibilityAnnouncementKey, NSAccessibilityAnnouncementRequestedNotification,
        NSAccessibilityNotificationUserInfoKey, NSAccessibilityPostNotificationWithUserInfo,
        NSAccessibilityPriorityKey, NSAccessibilityPriorityLevel,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSObject};

    // Build NSString for the message and NSNumber for the priority
    // level. AppKit's posting API copies both into the notification
    // userInfo dictionary at call time; we don't need to retain past
    // the call.
    let ns_msg = NSString::from_str(msg);
    let level = match priority {
        LiveRegionPriority::Polite => NSAccessibilityPriorityLevel::NSAccessibilityPriorityMedium,
        LiveRegionPriority::Assertive => NSAccessibilityPriorityLevel::NSAccessibilityPriorityHigh,
    };
    // NSAccessibilityPriorityLevel is a transparent newtype over
    // NSInteger — wrap the raw integer value in an NSNumber so it can
    // ride inside the userInfo dictionary.
    let ns_level: Retained<NSNumber> = NSNumber::new_isize(level.0);

    // Build the userInfo dictionary with `setObject:forKey:` so we
    // sidestep `NSDictionary::from_vec`'s strict `CounterpartOrSelf`
    // constraint (the value type is `AnyObject` in the AX API, but the
    // values we have are `NSString` / `NSNumber`). NSMutableDictionary
    // accepts any NSObject for value, which is what we want.
    //
    // SAFETY: each `msg_send!` here is a documented Foundation API:
    //   - `+[NSMutableDictionary dictionary]` returns an empty,
    //     autoreleased NSMutableDictionary.
    //   - `-setObject:forKey:` retains both arguments; the dictionary
    //     holds the strong count after the call, so the local
    //     Retained<NSString>/<NSNumber> can drop freely afterwards.
    //   - the two AX-key globals are stable AppKit string constants.
    let user_info: Retained<NSObject> = unsafe {
        let cls = objc2::class!(NSMutableDictionary);
        let dict: *mut NSObject = msg_send![cls, dictionary];
        let _: () = msg_send![
            dict,
            setObject: &*ns_msg as &NSObject,
            forKey: NSAccessibilityAnnouncementKey as &NSAccessibilityNotificationUserInfoKey,
        ];
        let _: () = msg_send![
            dict,
            setObject: &*ns_level as &NSObject,
            forKey: NSAccessibilityPriorityKey as &NSAccessibilityNotificationUserInfoKey,
        ];
        Retained::retain(dict).expect("retain user_info dictionary")
    };

    // The notification target. Apple's docs accept any AX element;
    // for window-scoped announcements `NSApp.keyWindow` is preferred,
    // but the framework doesn't track per-window state at this layer
    // — fall back to the shared NSApplication, which is the
    // documented generic-announcement target.
    let app_cls = objc2::class!(NSApplication);
    let app: *mut AnyObject = unsafe { msg_send![app_cls, sharedApplication] };
    if app.is_null() {
        // No NSApplication yet (e.g. announce called before host
        // bootstrap). Silently drop — there's no AX subsystem to post
        // to until NSApp exists.
        return;
    }
    // SAFETY: `app` is a non-null Objective-C object pointer returned
    // by `+[NSApplication sharedApplication]`, valid for the process
    // lifetime. `NSAccessibilityAnnouncementRequestedNotification` is
    // a static loaded by the dynamic linker. The user_info dictionary
    // is cast to the AX-typed parameter; both keys present are
    // documented to be of type `NSAccessibilityNotificationUserInfoKey`
    // (an NSString alias), so the cast is safe at the ObjC layer.
    unsafe {
        let dict_ptr: *const NSDictionary<NSAccessibilityNotificationUserInfoKey, AnyObject> =
            Retained::as_ptr(&user_info) as *const _;
        NSAccessibilityPostNotificationWithUserInfo(
            &*app,
            NSAccessibilityAnnouncementRequestedNotification,
            Some(&*dict_ptr),
        );
    }
}

// ---------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------

/// Write `value` via the given setter selector, or clear it (pass
/// nil) when `value` is `None`. AppKit's accessibility string setters
/// all accept nil to revert to the platform default; this is what
/// makes reactive prop updates that clear a previously-set field
/// actually drop the value.
fn set_string_or_clear(view: &NSView, sel: objc2::runtime::Sel, value: Option<&str>) {
    // `performSelector:withObject:` is declared as returning `id`
    // ('@') on NSObject regardless of the underlying target's
    // return type, so the Rust receiver type has to be a pointer
    // — even though the actual selectors we send (the
    // `setAccessibility*:` family) are void-returning. Pre-fix
    // this used `let _: () = ...`, which made objc2's debug-mode
    // signature verifier panic ("expected return to have type
    // code '@', but found 'v'") the first time the runtime-server-mode walk
    // hit an a11y label. The returned pointer is meaningless for
    // these setters; we ignore it.
    type Id = *mut objc2_foundation::NSObject;
    match value {
        Some(v) => {
            let ns = NSString::from_str(v);
            let _: Id = unsafe { msg_send![view, performSelector: sel, withObject: &*ns] };
        }
        None => {
            let nil: *const objc2_foundation::NSObject = std::ptr::null();
            let _: Id = unsafe { msg_send![view, performSelector: sel, withObject: nil] };
        }
    }
}

fn sel_set_accessibility_label() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityLabel:)
}
fn sel_set_accessibility_help() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityHelp:)
}
fn sel_set_accessibility_value() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityValue:)
}
fn sel_set_accessibility_identifier() -> objc2::runtime::Sel {
    objc2::sel!(setAccessibilityIdentifier:)
}

#[cfg(test)]
mod tests {
    //! Pure role/state-mapping coverage for the AppKit a11y translation.
    //! Mirrors the Android a11y role-mapping tests and wgpu's
    //! `a11y_tests` module — guards against a `Role`/`AccessibilityTraits`
    //! variant silently losing its NSAccessibility mapping. These assert
    //! the pure mapping helpers only (no NSView / running NSApp needed).
    use super::*;
    use objc2_app_kit::{
        NSAccessibilityButtonRole, NSAccessibilityCheckBoxRole, NSAccessibilityImageRole,
        NSAccessibilityLinkRole, NSAccessibilityStaticTextRole, NSAccessibilitySwitchSubrole,
        NSAccessibilityTabButtonSubrole,
    };

    /// `&'static NSString` constants are stable process-lifetime pointers,
    /// so identity comparison is the right equality for "did this role map
    /// to that exact AppKit constant".
    fn same(a: Option<&'static NSString>, b: &'static NSString) -> bool {
        a.map_or(false, |a| std::ptr::eq(a, b))
    }

    #[test]
    fn role_maps_to_expected_ns_role() {
        assert!(same(
            role_to_ns_accessibility_role(Role::Button),
            unsafe { NSAccessibilityButtonRole }
        ));
        assert!(same(
            role_to_ns_accessibility_role(Role::Link),
            unsafe { NSAccessibilityLinkRole }
        ));
        assert!(same(
            role_to_ns_accessibility_role(Role::Image),
            unsafe { NSAccessibilityImageRole }
        ));
        assert!(same(
            role_to_ns_accessibility_role(Role::Text),
            unsafe { NSAccessibilityStaticTextRole }
        ));
        // Switch and Header both ride a base role + a subrole (below).
        assert!(same(
            role_to_ns_accessibility_role(Role::Switch),
            unsafe { NSAccessibilityCheckBoxRole }
        ));
        assert!(same(
            role_to_ns_accessibility_role(Role::Header),
            unsafe { NSAccessibilityStaticTextRole }
        ));
    }

    #[test]
    fn subrole_only_set_for_flavored_roles() {
        // Switch / Tab need a subrole on top of their base role.
        assert!(same(
            role_to_ns_accessibility_subrole(Role::Switch),
            unsafe { NSAccessibilitySwitchSubrole }
        ));
        assert!(same(
            role_to_ns_accessibility_subrole(Role::Tab),
            unsafe { NSAccessibilityTabButtonSubrole }
        ));
        // A plain role needs no subrole — caller writes nil.
        assert!(role_to_ns_accessibility_subrole(Role::Button).is_none());
        assert!(role_to_ns_accessibility_subrole(Role::Text).is_none());
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
        assert_eq!(
            derive_accessibility_value(AccessibilityTraits::COLLAPSED),
            Some("collapsed")
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

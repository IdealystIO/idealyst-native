use runtime_core::accessibility::{AccessibilityProps, AccessibilityTraits, Role};

fn close_button_props() -> AccessibilityProps {
    AccessibilityProps {
        label: Some("Close dialog".to_string()),
        role: Some(Role::Button),
        traits: AccessibilityTraits::DISABLED,
        ..Default::default()
    }
}

// Traits are orthogonal state flags, composed with `|`.
fn expanded_tab() -> AccessibilityTraits {
    AccessibilityTraits::SELECTED | AccessibilityTraits::EXPANDED
}

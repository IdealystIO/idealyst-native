//! Portable integration tests — exercise the public `haptics` API exactly as
//! an external app would (the crate's own unit tests reach internals; these
//! only see the public surface).
//!
//! The contract under test is platform-independent: every function is
//! best-effort, infallible, and must be safe to call on whatever host runs
//! `cargo test` — never panicking, whether the active backend is the real
//! macOS `NSHapticFeedbackManager` path or the `noop` shim on Linux/Windows
//! CI.

use haptics::{impact, is_supported, notify, selection, ImpactStyle, NotificationFeedback};

#[test]
fn every_public_call_is_safe_to_make() {
    // All five impact weights.
    for style in [
        ImpactStyle::Light,
        ImpactStyle::Medium,
        ImpactStyle::Heavy,
        ImpactStyle::Soft,
        ImpactStyle::Rigid,
    ] {
        impact(style);
    }
    // All three notification outcomes.
    for fb in [
        NotificationFeedback::Success,
        NotificationFeedback::Warning,
        NotificationFeedback::Error,
    ] {
        notify(fb);
    }
    selection();

    // The support predicate is pure and must answer without side effects.
    let _ = is_supported();
}

#[test]
fn enums_are_copy_value_types() {
    // An author passes these by value into the fns repeatedly; they're `Copy`.
    let s = ImpactStyle::Rigid;
    impact(s);
    impact(s); // still usable — not moved
    assert_eq!(s, ImpactStyle::Rigid);

    let fb = NotificationFeedback::Warning;
    notify(fb);
    notify(fb);
    assert_eq!(fb, NotificationFeedback::Warning);
}

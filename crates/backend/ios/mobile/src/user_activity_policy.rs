//! Pure decisions for the iOS `user_activity()` source (`imp/activity.rs`),
//! kept un-gated so their coverage runs from any host.

/// Whether a window gets the activity recognizer, by its ObjC class name.
///
/// The app's own windows — a plain `UIWindow` (the generated
/// `AppDelegate`'s), a Swift / ObjC subclass an app declares, and the
/// framework's `IdealystPassthroughWindow` — are observed. UIKit's private
/// windows (`UITextEffectsWindow`, `UIRemoteKeyboardWindow`, …) are not:
/// they host the soft keyboard, whose key taps are delivered to the
/// keyboard's own process and never reach a recognizer anyway (typing is
/// observed through text-change notifications instead), and adding
/// recognizers to UIKit-owned windows is outside what UIKit supports.
pub(crate) fn observes_window(class_name: &str) -> bool {
    let private = class_name.starts_with("_UI") || class_name.starts_with("UI");
    !private || class_name == "UIWindow"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_windows_are_observed() {
        assert!(observes_window("UIWindow"));
        assert!(observes_window("IdealystPassthroughWindow"));
        assert!(observes_window("MyApp.KioskWindow"));
    }

    #[test]
    fn uikit_private_windows_are_skipped() {
        assert!(!observes_window("UITextEffectsWindow"));
        assert!(!observes_window("UIRemoteKeyboardWindow"));
        assert!(!observes_window("_UIInteractiveHighlightEffectWindow"));
    }
}

//! Pressable activation: which moment of a pointer press fires a
//! `pressable`'s action.

/// When a pointer press fires a `pressable`'s action.
///
/// Every backend implements both modes natively, so the observable
/// behaviour is identical everywhere:
///
/// - **[`Release`](Self::Release)** (the default) is the platform click:
///   the action fires when the primary pointer is released over the
///   control, and a press dragged off before release does nothing.
/// - **[`Down`](Self::Down)** fires the action the moment the primary
///   pointer goes down on the control. The rest of that press (moves,
///   the release) belongs to the control and fires nothing more, so
///   one press is one action. This is how desktop menu buttons behave:
///   the menu is open before the button is released.
///
/// In both modes only the primary button activates; a secondary
/// (right / Ctrl) press does not. Keyboard activation (Space / Return
/// on a focused control), assistive-technology activation and a robot
/// `click` are single discrete events with no "down" to separate from
/// an "up", so they fire the action once in either mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PressActivation {
    /// Fire on release, like a platform click. The default.
    #[default]
    Release,
    /// Fire as the primary pointer goes down.
    Down,
}

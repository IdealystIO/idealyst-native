//! The width ladder (rule `ladder`): the one place the kit turns a
//! breakpoint into a decision.

use runtime_core::{current_breakpoint, untrack, Breakpoint};

/// The three forms a screen has. Content has two layouts, phone and
/// desktop; a tablet draws the desktop layout and differs only in where
/// the app's sidebar sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormFactor {
    /// `Xs`, under 640. The phone layout: sheets, stacked blocks, one
    /// region at a time.
    Phone,
    /// `Sm` and `Md`, 640 to 1023. The desktop layout with the sidebar
    /// as a drawer.
    Tablet,
    /// `Lg` and up, 1024 and wider. The desktop layout with the sidebar
    /// pinned.
    Desktop,
}

impl FormFactor {
    /// The form a breakpoint draws.
    pub fn of(breakpoint: Breakpoint) -> Self {
        if breakpoint.is_at_least(Breakpoint::Lg) {
            FormFactor::Desktop
        } else if breakpoint.is_at_least(Breakpoint::Sm) {
            FormFactor::Tablet
        } else {
            FormFactor::Phone
        }
    }
}

/// The form the window draws now. TRACKED: an effect, a `ui!` branch or
/// a style closure that calls it re-runs when the window crosses 640 or
/// 1024.
pub fn form_factor() -> FormFactor {
    FormFactor::of(current_breakpoint().get())
}

/// The phone layout is drawing. TRACKED, like [`form_factor`].
pub fn is_phone() -> bool {
    form_factor() == FormFactor::Phone
}

/// The window is a phone's at this moment, read untracked. For a value
/// a screen picks ONCE as it opens and the reader changes from there (a
/// canvas that opens fitted to the width, a zoom level), never for
/// choosing what to draw: that is the layout's (rule `layout`).
///
/// ```ignore
/// let zoom = signal(if opens_on_phone() { None } else { Some(1.0) });
/// ```
pub fn opens_on_phone() -> bool {
    untrack(is_phone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_steps_at_sm_and_lg_only() {
        assert_eq!(FormFactor::of(Breakpoint::Xs), FormFactor::Phone);
        assert_eq!(FormFactor::of(Breakpoint::Sm), FormFactor::Tablet);
        assert_eq!(FormFactor::of(Breakpoint::Md), FormFactor::Tablet);
        assert_eq!(FormFactor::of(Breakpoint::Lg), FormFactor::Desktop);
        assert_eq!(FormFactor::of(Breakpoint::Xl), FormFactor::Desktop);
    }
}

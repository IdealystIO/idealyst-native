//! Whether the platform should be keeping the display on: the portable half,
//! with no platform call in it.

/// Counts holds and tracks whether the app is on screen.
pub(crate) struct Policy {
    holds: usize,
    visible: bool,
    /// What the platform was last told.
    told: bool,
    /// Something changed that a refused request may succeed after.
    retry: bool,
}

impl Policy {
    pub(crate) fn new(visible: bool) -> Self {
        Self { holds: 0, visible, told: false, retry: false }
    }

    pub(crate) fn hold(&mut self) {
        self.holds += 1;
    }

    pub(crate) fn release(&mut self) {
        debug_assert!(self.holds > 0, "every guard releases once");
        self.holds = self.holds.saturating_sub(1);
    }

    /// The app's situation was reported. Also reached by changes other than
    /// visibility: a browser may refuse a wake lock before the first user
    /// gesture, and that gesture is one of them.
    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        self.retry = true;
    }

    /// What to tell the platform now, if anything. Backends' `set(true)` is
    /// idempotent, so asking again while engaged costs nothing.
    pub(crate) fn take_call(&mut self) -> Option<bool> {
        let engaged = self.holds > 0 && self.visible;
        let retry = std::mem::take(&mut self.retry) && engaged;
        let changed = engaged != self.told;
        self.told = engaged;
        (changed || retry).then_some(engaged)
    }
}

#[cfg(test)]
impl Policy {
    pub(crate) fn holds(&self) -> usize {
        self.holds
    }

    pub(crate) fn told(&self) -> bool {
        self.told
    }
}

#[cfg(test)]
mod tests {
    use super::Policy;

    #[test]
    fn the_last_hold_releases() {
        let mut p = Policy::new(true);
        assert_eq!(p.take_call(), None, "nothing held, nothing said");
        p.hold();
        assert_eq!(p.take_call(), Some(true));
        p.hold();
        assert_eq!(p.take_call(), None, "already on");
        p.release();
        assert_eq!(p.take_call(), None, "one hold left");
        p.release();
        assert_eq!(p.take_call(), Some(false));
    }

    #[test]
    fn no_effect_off_screen_and_again_on_return() {
        let mut p = Policy::new(true);
        p.hold();
        assert_eq!(p.take_call(), Some(true));
        p.set_visible(false);
        assert_eq!(p.take_call(), Some(false));
        p.hold();
        assert_eq!(p.take_call(), None, "held in the background: not yet");
        p.set_visible(true);
        assert_eq!(p.take_call(), Some(true));
    }

    #[test]
    fn released_in_the_background_stays_off_on_return() {
        let mut p = Policy::new(true);
        p.hold();
        p.take_call();
        p.set_visible(false);
        p.take_call();
        p.release();
        assert_eq!(p.take_call(), None);
        p.set_visible(true);
        assert_eq!(p.take_call(), None);
    }

    #[test]
    fn a_report_while_engaged_asks_again() {
        let mut p = Policy::new(true);
        p.hold();
        p.take_call();
        p.set_visible(true);
        assert_eq!(p.take_call(), Some(true), "a refused request gets another try");
        assert_eq!(p.take_call(), None, "once per report");
    }
}

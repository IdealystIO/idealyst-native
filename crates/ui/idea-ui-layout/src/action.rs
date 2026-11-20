//! [`Action`] — one verb, in the one shape every layout takes it.

use std::rc::Rc;

use idea_ui::ToneRef;
use runtime_core::{untrack, IconData, Reactive};

/// A verb a person can press: its label, what it does, and how it looks
/// where there is room for less than a labelled button.
///
/// The same value serves a [`PageHeader`](crate::PageHeader)'s actions,
/// an [`ActionBar`](crate::ActionBar) and an
/// [`ActionMenu`](crate::ActionMenu), so a verb that moves between them
/// as the window narrows keeps its label, glyph and handler.
///
/// ```ignore
/// Action::new("Add heading", on_add).icon(PLUS).primary()
/// Action::new("Delete", on_delete).tone(tone::Danger).disabled(rx!(locked.get()))
/// ```
#[derive(Clone)]
pub struct Action {
    pub(crate) label: String,
    pub(crate) icon: Option<IconData>,
    pub(crate) on_press: Rc<dyn Fn()>,
    pub(crate) primary: bool,
    pub(crate) tone: Option<ToneRef>,
    pub(crate) busy: Option<Reactive<bool>>,
    pub(crate) disabled: Option<Reactive<bool>>,
    pub(crate) test_id: Option<&'static str>,
}

impl Action {
    /// A verb and its handler.
    pub fn new(label: impl Into<String>, on_press: Rc<dyn Fn()>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            on_press,
            primary: false,
            tone: None,
            busy: None,
            disabled: None,
            test_id: None,
        }
    }

    /// The verb's glyph: drawn before the label, and ALONE where a
    /// header collapses its primary on a phone.
    pub fn icon(mut self, icon: IconData) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The screen's one filled verb (rule `header-actions`).
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    /// The verb's tone: `tone::Danger` for a verb that destroys. A button
    /// takes it, and an action menu's row draws its label and icon in the
    /// tone's ink, anchored or as a phone's sheet.
    pub fn tone(mut self, tone: impl Into<ToneRef>) -> Self {
        self.tone = Some(tone.into());
        self
    }

    /// In flight: the button spins and refuses a second press.
    pub fn busy(mut self, busy: impl Into<Reactive<bool>>) -> Self {
        self.busy = Some(busy.into());
        self
    }

    /// Not available yet (the form is incomplete).
    pub fn disabled(mut self, disabled: impl Into<Reactive<bool>>) -> Self {
        self.disabled = Some(disabled.into());
        self
    }

    /// The robot/E2E id of the control the verb is drawn as.
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.test_id = Some(id);
        self
    }

    /// The verb's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn busy_now(&self) -> Reactive<bool> {
        self.busy.clone().unwrap_or(Reactive::Static(false))
    }

    pub(crate) fn disabled_now(&self) -> Reactive<bool> {
        self.disabled.clone().unwrap_or(Reactive::Static(false))
    }

    /// The handler as a menu or sheet row runs it: a row has no disabled
    /// or spinning state of its own, so a press while the verb is
    /// unavailable does nothing.
    pub(crate) fn guarded_press(&self) -> Rc<dyn Fn()> {
        let run = self.on_press.clone();
        let busy = self.busy.clone();
        let disabled = self.disabled.clone();
        Rc::new(move || {
            let blocked = untrack(|| {
                busy.as_ref().is_some_and(|b| b.get()) || disabled.as_ref().is_some_and(|d| d.get())
            });
            if !blocked {
                run();
            }
        })
    }
}

impl Default for Action {
    /// An unlabelled verb that does nothing: only the placeholder a
    /// props struct's `Default` needs.
    fn default() -> Self {
        Self::new(String::new(), Rc::new(|| {}))
    }
}

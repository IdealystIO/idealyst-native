//! Compile-checked usage **recipes** for the storage SDK.
//!
//! `recipe!` self-gates on the `catalog` feature: production builds expand to
//! nothing, and `runtime-core` isn't even a dependency then.

use runtime_core::recipe;

recipe!(
    persisted_signal,
    /// A text note that persists across app restarts — via
    /// `storage::persisted_signal`, the blessed storage↔signal rail (needs
    /// the storage crate's `reactive` feature). It hydrates from storage on
    /// creation, persists on every change automatically, and is
    /// race-correct by contract: a value the user enters BEFORE the async
    /// load resolves wins over the stored one (hand-rolled load/persist
    /// wiring almost always clobbers it — don't hand-roll this).
    pub fn persisted_note() -> ::runtime_core::Element {
        use ::runtime_core::ui;

        let note = crate::persisted_signal("recipes-demo", "note", String::new());

        ui! {
            view {
                text_input(
                    value = note,
                    on_change = move |t| note.set(t),
                    placeholder = "Saved as you type",
                )
                text { "Current: {note}" }
            }
        }
    }
);

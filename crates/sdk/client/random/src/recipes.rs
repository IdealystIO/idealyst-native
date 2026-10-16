//! Compile-checked usage **recipes** for the `random` SDK.
//!
//! Each `recipe!(target, fn ...)` is a real, type-checked example, compiled
//! against this crate's live API whenever the `catalog` feature is built.
//! Recipes are self-contained — every `use` lives inside the fn — so the
//! captured source reads as a complete, copy-pasteable example.

use runtime_core::recipe;

recipe!(
    range,
    /// Roll a die on each press. `random::range(1..=6)` draws from the
    /// per-thread generator, seeded from OS entropy on first use; call
    /// `random::seed(n)` first (e.g. in a test) to make every roll
    /// reproducible on every platform.
    pub fn random_dice_roll() -> ::runtime_core::Element {
        use ::runtime_core::{signal, text, ui};

        let roll = signal(0u8);
        let on_click = move || roll.set(crate::range(1..=6));

        ui! {
            view {
                button(label = "Roll".to_string(), on_click = on_click)
                text(move || format!("You rolled {}", roll.get()))
            }
        }
    }
);

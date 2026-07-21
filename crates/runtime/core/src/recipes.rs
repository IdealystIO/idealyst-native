//! Compile-checked usage **recipes** for the core primitives — the canonical
//! "how do I actually wire this" examples the MCP catalog serves.
//!
//! Why these exist: the arena's first feedback report (todo-app run-2,
//! 2026-07-20) showed an agent reconstructing exactly these three patterns —
//! a submit form, a keyed reactive list, and the surrounding state wiring —
//! by reading five scattered framework source files, because `list_recipes`
//! for a bare app returned nothing. Each `recipe!` here compiles against the
//! live API (a prop change that isn't reflected here is a compile error),
//! so the served example can't rot.
//!
//! `recipe!` self-gates on the `catalog` feature: production builds expand
//! to nothing. Recipes are self-contained — imports live inside each fn — so
//! the captured source reads as a complete, copy-pasteable example.

use crate::recipe;

recipe!(
    text_input,
    /// A single-line input with Enter-to-submit and a button sharing the same
    /// handler. The `value` signal is the input's source of truth; `on_change`
    /// writes it back; `on_key_down` turns Enter into submit (returning
    /// `PreventDefault` so the platform's own Enter behaviour is suppressed).
    pub fn input_with_submit() -> ::runtime_core::Element {
        use ::runtime_core::primitives::key::KeyOutcome;
        use ::runtime_core::{signal, ui};

        let draft = signal(String::new());
        let submitted = signal(String::new());

        let submit = move || {
            let text = draft.get();
            if !text.trim().is_empty() {
                submitted.set(text);
                draft.set(String::new());
            }
        };

        let submit_on_enter = submit.clone();
        ui! {
            view {
                text_input(
                    value = draft,
                    // `value` is the signal → widget leg only; the user's
                    // keystrokes arrive via on_change — without this, the
                    // signal never updates and get() is always empty.
                    on_change = move |t| draft.set(t),
                    placeholder = "Type and press Enter",
                ).on_key_down(move |e| {
                    // on_key_down is a BUILDER METHOD (chained), not a ui!
                    // prop — an inline `on_key_down = ...` prop is silently
                    // dropped by the macro.
                    if e.key == "Enter" {
                        submit_on_enter();
                        KeyOutcome::PreventDefault
                    } else {
                        KeyOutcome::Default
                    }
                })
                button(label = "Submit", on_click = submit)
                text { "Last submitted: {submitted}" }
            }
        }
    }
);

recipe!(
    ui,
    /// A reactive list with add and remove. The keyed `for` iterates the
    /// `Signal<Vec<T>>` ITSELF (not `.get()`): mutations via `.update(..)`
    /// re-render, and `key =` drives reconciliation so rows move/reuse
    /// instead of rebuilding. Stable ids (not indexes) make removal correct.
    pub fn keyed_list_add_remove() -> ::runtime_core::Element {
        use ::runtime_core::{signal, ui};

        #[derive(Clone)]
        struct Item {
            id: u64,
            label: String,
        }

        let items = signal(Vec::<Item>::new());
        let next_id = signal(0u64);

        let add = move || {
            let id = next_id.get();
            next_id.set(id + 1);
            items.update(|v| v.push(Item { id, label: format!("Item {id}") }));
        };

        ui! {
            view {
                button(label = "Add item", on_click = add)
                for item in items, key = item.id {
                    view {
                        text { "{item.label}" }
                        button(
                            label = "Remove",
                            on_click = move || items.update(|v| v.retain(|i| i.id != item.id)),
                        )
                    }
                }
            }
        }
    }
);

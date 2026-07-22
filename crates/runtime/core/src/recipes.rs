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
                        // Field paths aren't f-string slots (only a bare
                        // `{ident}` is) — read the field in a closure.
                        text { move || item.label.clone() }
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

recipe!(
    presence,
    /// A `signal(false)`-driven toast that animates in and out with
    /// `presence` — the primitive that owns mount/unmount *timing* so an
    /// enter animation can play on appearance and an exit animation can
    /// finish before the subtree is torn down (a plain `if`/`when` would
    /// tear it down instantly, with no window for the exit to play).
    ///
    /// `presence(child)` returns a builder; chain `.present(...)` (the
    /// open/close predicate), `.enter(...)` and `.exit(...)`. On enter,
    /// `enter.state` is applied *before* the first paint then interpolated
    /// back to rest over `enter.duration_ms`; on exit, `exit.state` is
    /// interpolated *toward* and the subtree is held mounted for
    /// `exit.duration_ms` before it drops. `PresenceState` carries only the
    /// four universally-interpolatable properties (opacity + 2D translate +
    /// uniform scale) — here a fade-and-slide, mirrored both ways. Finish
    /// with `.into_element()` and splat the result into the tree.
    pub fn animated_toast() -> ::runtime_core::Element {
        use ::runtime_core::{
            presence, signal, ui, Easing, IntoElement, PresenceAnim, PresenceState,
        };

        let open = signal(false);

        // The presence-controlled subtree. `child` is a closure so presence
        // can (re)build it per mount; it captures nothing here.
        let toast = presence(|| {
            ui! {
                view {
                    text { "Saved" }
                }
            }
        })
        .present(move || open.get())
        // Enter: start faded + nudged down, settle to rest over 200ms.
        .enter(PresenceAnim::new(
            PresenceState::rest().opacity(0.0).translate_y(8.0),
            200,
            Easing::EaseOut,
        ))
        // Exit: fade + slide back out over 150ms, THEN unmount (the mirror
        // of enter — sharing the `PresenceState` shape reads symmetrically).
        .exit(PresenceAnim::new(
            PresenceState::rest().opacity(0.0).translate_y(8.0),
            150,
            Easing::EaseIn,
        ))
        .into_element();

        ui! {
            view {
                button(label = "Toggle toast", on_click = move || open.update(|o| *o = !*o))
                // Splat the built presence element as a sibling child (bare
                // identifier — no braces — is the pre-built-Element splat).
                toast
            }
        }
    }
);

recipe!(
    overlay,
    /// A keyed list whose rows delete through a CONFIRM dialog rendered as a
    /// viewport `overlay`. A `signal(Option<u64>)` holds the id awaiting
    /// confirmation; while it's `Some`, an `if let` INSIDE `ui!` (the
    /// standard conditional — never a `Vec::push` before the macro) mounts a
    /// centered confirm panel over a scrim.
    ///
    /// `overlay(placement = Center, backdrop = Dismiss, on_dismiss = …)`
    /// centers the panel and renders a dismiss-on-tap backdrop: a tap on the
    /// scrim (or Escape / back) fires `on_dismiss`, which clears the pending
    /// id — the cancel path. The panel's own Confirm button both removes the
    /// row and clears the id; Cancel just clears it. `Signal` is `Copy`, so
    /// each closure captures its own copy without a `.clone()`.
    pub fn confirm_dialog_overlay() -> ::runtime_core::Element {
        use ::runtime_core::primitives::overlay::BackdropMode;
        use ::runtime_core::{signal, ui, ViewportPlacement};

        #[derive(Clone)]
        struct Item {
            id: u64,
            label: String,
        }

        let items = signal(vec![
            Item { id: 0, label: "Groceries".to_string() },
            Item { id: 1, label: "Ideas".to_string() },
        ]);
        // The row awaiting delete confirmation — `None` = no dialog open.
        let pending = signal(None::<u64>);

        ui! {
            view {
                for item in items, key = item.id {
                    view {
                        // Field paths aren't f-string slots (only a bare
                        // `{ident}` is) — read the field in a closure.
                        text { move || item.label.clone() }
                        button(label = "Delete", on_click = move || pending.set(Some(item.id)))
                    }
                }
                // The confirm dialog is in the tree only while a delete is
                // pending. Reading `pending.get()` here makes this branch
                // reactive — it mounts/unmounts as the signal flips.
                if let Some(id) = pending.get() {
                    overlay(
                        placement = ViewportPlacement::Center,
                        backdrop = BackdropMode::Dismiss,
                        // Backdrop tap / Escape / back = cancel.
                        on_dismiss = move || pending.set(None),
                    ) {
                        view {
                            text { "Delete this item?" }
                            button(label = "Cancel", on_click = move || pending.set(None))
                            button(
                                label = "Delete",
                                on_click = move || {
                                    items.update(|v| v.retain(|i| i.id != id));
                                    pending.set(None);
                                },
                            )
                        }
                    }
                }
            }
        }
    }
);

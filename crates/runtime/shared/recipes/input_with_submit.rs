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

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

    #[derive(Clone, PartialEq)]
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
                                // Read-modify-set — the write is what
                                // re-renders the keyed list.
                                let mut v = items.get();
                                v.retain(|i| i.id != id);
                                items.set(v);
                                pending.set(None);
                            },
                        )
                    }
                }
            }
        }
    }
}

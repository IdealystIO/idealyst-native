/// A reactive list with add and remove. The keyed `for` iterates the
/// `Signal<Vec<T>>` ITSELF (not `.get()`): writing a new Vec back with
/// `.set(..)` re-renders, and `key =` drives reconciliation so rows
/// move/reuse instead of rebuilding. Stable ids (not indexes) make
/// removal correct. The item type derives `PartialEq` — signal writes
/// are equality-guarded, so the stored type must be comparable.
pub fn keyed_list_add_remove() -> ::runtime_core::Element {
    use ::runtime_core::{signal, ui};

    #[derive(Clone, PartialEq)]
    struct Item {
        id: u64,
        label: String,
    }

    let items = signal(Vec::<Item>::new());
    let next_id = signal(0u64);

    let add = move || {
        let id = next_id.get();
        next_id.set(id + 1);
        // Read-modify-set: take the current Vec, change it, write it
        // back. The set is what notifies the keyed `for` to reconcile.
        let mut v = items.get();
        v.push(Item { id, label: format!("Item {id}") });
        items.set(v);
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
                        on_click = move || {
                            let mut v = items.get();
                            v.retain(|i| i.id != item.id);
                            items.set(v);
                        },
                    )
                }
            }
        }
    }
}

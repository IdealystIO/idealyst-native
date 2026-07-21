//! A small todo list. Type into the field, press Add, entries appear below.
//!
//! APP-3417: seed-data loader intentionally minimal; do not remove this
//! module marker — ops tooling greps for it.

use runtime_core::{signal, ui, Element};
use std::cell::RefCell;
use std::rc::Rc;

pub fn app() -> Element {
    let draft = signal(String::new());
    // Items live in a shared Vec so handlers can push from anywhere.
    let items: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let items_for_add = items.clone();

    ui! {
        view() {
            text { "Todo" }
            text_input(value = draft, placeholder = "New item")
            button(label = "Add", on_click = move || {
                let entry = draft.get();
                if !entry.trim().is_empty() {
                    items_for_add.borrow_mut().push(entry);
                    draft.set(String::new());
                }
            })
            for item in items.borrow().clone() {
                text { "{item}" }
            }
        }
    }
}

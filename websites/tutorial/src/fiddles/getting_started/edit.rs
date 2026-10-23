use runtime_core::{ui, Element};

// Change the greeting, add a second `text`, then press Run (or ⌘/Ctrl-Enter).
pub fn app() -> Element {
    ui! {
        view {
            text { "Hello, Idealyst!" }
            text { "Edit me and run again." }
        }
    }
}

use idea_ui::{typography_kind, Typography};
use runtime_core::{component, ui, Element};

// Components are PascalCase; primitives (`view`, `text`, `button`, …) are
// lowercase. `#[component]` turns the parameters into props, so `app.rs`
// calls this as `Title(label = "…")`.
#[component]
pub fn Title(label: String) -> Element {
    ui! { Typography(content = label, kind = typography_kind::H1) }
}

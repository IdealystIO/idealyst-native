// Sibling module of `lib.rs`. The shared `use crate::__rt::*;`
// prelude is injected by the fiddle server, so framework types
// and `ui!` / `stylesheet!` are already in scope.

// Components are PascalCase, primitives (`view`, `text`, `button`, …) are
// snake_case. `#[component]` reads the fn's parameters as the props and
// generates the props struct + the `Title` tag alias, so the `ui!` call site
// in `lib.rs` writes `Title(label = "…")`.
#[component]
pub fn Title(label: String) -> Element {
    // idea-ui's `Typography` is a styled-text component; it takes its
    // string via the `content` prop, not as a `{ ... }` body.
    ui! { Typography(content = label, kind = idea_ui::typography_kind::H1.into()) }
}

//! A handful of `#[component]` definitions that exercise the catalog:
//!
//! - `IconLabel` / `PrimaryButton` / `Spacer` — leaf components, no
//!   `ui!` body, no composes.
//! - `Card` — composes two leaves; demonstrates a multi-child host.
//! - `AppShell` — composes `Card` and `PrimaryButton`; deeper graph.
//! - `Spacer` — *no doc comment*; the catalog records `docs: ""`.
//! - `PrimaryButton` — multi-paragraph doc-comment; verifies that
//!   newlines + blank-line breaks round-trip into the catalog.
//! - `forms::Submit` / `forms::FormRoot` — a submodule. `FormRoot`
//!   composes `Submit` (resolves into the `forms` module).
//!
//! Component fns are named PascalCase to match their `ui!` / `jsx!`
//! tag, so a call site's ident is exactly the catalog entry name.
//!
//! The demo sticks to distinct names: a `forms::Card` colliding with the
//! root `Card` would be ambiguous at compile time. The proximity rule is
//! covered in `mcp-catalog/tests/registers_component.rs` instead.
//!
//! These functions are never called directly — the point is the
//! `inventory::submit!` each `#[component]` emits as a side-effect.

#![allow(dead_code)]

use runtime_core::Element;
use runtime_core::{component, idealyst_tool, ui, IdealystSchema};

/// A small icon-with-label widget. Leaf component — no `ui!` body,
/// so it has no composes edges.
#[component]
pub fn IconLabel() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

/// A primary action button.
///
/// Used in dialogs, forms, and the app shell's header. This
/// multi-paragraph doc-comment is here to demonstrate that the
/// catalog preserves newlines and blank-line paragraph breaks.
#[component]
pub fn PrimaryButton() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

// Note (`//`, not `///`): `spacer` has no doc comment by design — the
// catalog should record `docs: ""`. This text doesn't become docs.
#[component]
pub fn Spacer() -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

/// A card with an icon-label header and a primary action.
/// Composes two leaf components — visible in `composes`.
#[component]
pub fn Card() -> Element {
    ui! {
        view() {
            IconLabel()
            PrimaryButton()
        }
    }
}

/// The app's top-level layout. Composes `Card` and an extra
/// `PrimaryButton`. Reverse adjacency: `Card` and `PrimaryButton`
/// each list `app_shell` among their users.
#[component]
pub fn AppShell() -> Element {
    ui! {
        view() {
            Card()
            PrimaryButton()
            Spacer()
        }
    }
}

/// Props for [`LabeledBadge`]. With `#[derive(IdealystSchema)]`
/// every field shows up in the catalog as a `PropFieldSpec` with
/// docs + optional `#[schema(constraint = "...")]` hints.
#[derive(Debug, Default, IdealystSchema)]
pub struct LabeledBadgeProps {
    /// Visible label text.
    pub label: String,
    /// Numeric badge value, capped at 99 in render.
    pub count: u32,
    /// Background color.
    #[schema(constraint = "valid CSS color")]
    pub color: String,
}

/// A badge with a text label and a count. Demonstrates a single-
/// struct signature in the catalog: the resolved view shows
/// `params: props: &LabeledBadgeProps`.
#[component]
pub fn LabeledBadge(_props: &LabeledBadgeProps) -> Element {
    ::runtime_core::view(::std::vec::Vec::new())
}

/// Returns a hex color darkened by `amount` (linear-light space).
/// Standalone helper exposed through MCP via `#[idealyst_tool]` —
/// shows up under `list_tools` and `describe_tool`.
#[idealyst_tool]
pub fn darken(_hex: &str, _amount: f32) -> String {
    // Demo body — real impl would convert hex → linear → scale → hex.
    String::new()
}

/// Submodule whose `FormRoot` host references a `Submit` button.
/// The resolver should resolve `Submit` to `forms::submit` since
/// only one entry has that short-name.
pub mod forms {
    use runtime_core::Element;
    use runtime_core::{component, ui};

    /// A submit button. Unique short-name; resolves directly.
    #[component]
    pub fn Submit() -> Element {
        ::runtime_core::view(::std::vec::Vec::new())
    }

    /// Form-page host. Composes `Submit`.
    #[component]
    pub fn FormRoot() -> Element {
        ui! {
            view() {
                Submit()
            }
        }
    }
}

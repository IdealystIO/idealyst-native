//! A handful of `#[component]` definitions that exercise the catalog:
//!
//! - `icon_label` / `primary_button` / `spacer` — leaf components, no
//!   `ui!` body, no composes.
//! - `card` — composes two leaves; demonstrates a multi-child host.
//! - `app_shell` — composes `Card` and `PrimaryButton`; deeper graph.
//! - `spacer` — *no doc comment*; the catalog records `docs: ""`.
//! - `primary_button` — multi-paragraph doc-comment; verifies that
//!   newlines + blank-line breaks round-trip into the catalog.
//! - `forms::submit` / `forms::form_root` — a submodule. `form_root`
//!   composes `Submit` (resolves into the `forms` module).
//!
//! Fn names are `snake_case` (idiomatic Rust); `ui!` / `jsx!` call
//! sites use `PascalCase`. The macros convert between them: e.g.
//! `PrimaryButton()` lowers to `primary_button!()`. Authors may also
//! write the call site directly in `snake_case` — the conversion is
//! idempotent on already-snake input.
//!
//! Note: an earlier draft had `forms::card` collide with the root
//! `card` to demo proximity resolution at the macro level — but
//! Rust's `macro_rules!` visibility into child modules makes
//! `Card()` inside `forms::*` ambiguous at compile time. The
//! proximity rule is covered exhaustively in
//! `framework-mcp/tests/registers_component.rs` instead. The demo
//! sticks to distinct names.
//!
//! These functions are never called directly — the point is the
//! `inventory::submit!` each `#[component]` emits as a side-effect.

#![allow(dead_code)]

use framework_core::Primitive;
use framework_core::{component, idealyst_tool, ui, IdealystSchema};

/// A small icon-with-label widget. Leaf component — no `ui!` body,
/// so it has no composes edges.
#[component]
pub fn icon_label() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

/// A primary action button.
///
/// Used in dialogs, forms, and the app shell's header. This
/// multi-paragraph doc-comment is here to demonstrate that the
/// catalog preserves newlines and blank-line paragraph breaks.
#[component]
pub fn primary_button() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

// Note (`//`, not `///`): `spacer` has no doc comment by design — the
// catalog should record `docs: ""`. This text doesn't become docs.
#[component]
pub fn spacer() -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

/// A card with an icon-label header and a primary action.
/// Composes two leaf components — visible in `composes`.
#[component]
pub fn card() -> Primitive {
    ui! {
        View() {
            IconLabel()
            PrimaryButton()
        }
    }
}

/// The app's top-level layout. Composes `Card` and an extra
/// `PrimaryButton`. Reverse adjacency: `Card` and `PrimaryButton`
/// each list `app_shell` among their users.
#[component]
pub fn app_shell() -> Primitive {
    ui! {
        View() {
            Card()
            PrimaryButton()
            Spacer()
        }
    }
}

/// Props for [`labeled_badge`]. With `#[derive(IdealystSchema)]`
/// every field shows up in the catalog as a `PropFieldSpec` with
/// docs + optional `#[schema(constraint = "...")]` hints.
#[derive(Debug, IdealystSchema)]
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
pub fn labeled_badge(_props: &LabeledBadgeProps) -> Primitive {
    ::framework_core::view(::std::vec::Vec::new())
}

/// Returns a hex color darkened by `amount` (linear-light space).
/// Standalone helper exposed through MCP via `#[idealyst_tool]` —
/// shows up under `list_tools` and `describe_tool`.
#[idealyst_tool]
pub fn darken(_hex: &str, _amount: f32) -> String {
    // Demo body — real impl would convert hex → linear → scale → hex.
    String::new()
}

/// Submodule whose `form_root` host references a `Submit` button.
/// The resolver should resolve `Submit` to `forms::submit` since
/// only one entry has that short-name.
pub mod forms {
    use framework_core::Primitive;
    use framework_core::{component, ui};

    /// A submit button. Unique short-name; resolves directly.
    #[component]
    pub fn submit() -> Primitive {
        ::framework_core::view(::std::vec::Vec::new())
    }

    /// Form-page host. Composes `Submit`.
    #[component]
    pub fn form_root() -> Primitive {
        ui! {
            View() {
                Submit()
            }
        }
    }
}

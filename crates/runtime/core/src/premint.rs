//! Premint style-dump registry — the collection side of build-time CSS
//! emission (see the styling guide's build-time CSS section and
//! `StyleSource::Preminted`).
//!
//! Under `cfg(idealyst_premint_dump)` (set by the CLI's ephemeral dump
//! build, paired with this crate's `style-dump` feature), every
//! `stylesheet!` expansion registers a [`PremintSheet`] into
//! [`PREMINT_SHEETS`] via a linkme distributed slice. Link-time
//! collection — NOT first-use registration — because the dump must see
//! every sheet in the binary, including ones on screens the dump run
//! never renders.
//!
//! The CSS assembly itself lives in the `premint-dump` crate (it needs
//! the `css` crate, which depends on this one — the slice lives here to
//! break that cycle: generated app code references only
//! `runtime_core::premint`).

use crate::style::{StyleApplication, StyleRules, StyleSheet};
use crate::StateBits;
use std::rc::Rc;

/// Re-export for `stylesheet!`-generated registration code
/// (`#[linkme(crate = ::runtime_core::premint::linkme)]`).
pub use linkme;

/// One variant axis of a registered sheet: its name, every value's
/// snake-case string (declaration order), and the segment an *unset*
/// axis contributes to the preminted class name — the `#[default]`
/// arm's name, or `"_"` when the axis declares none. Mirrors exactly
/// what the `stylesheet!` runtime branch assembles, which is the
/// no-manifest contract between the `.css` and the shipped binary.
pub struct PremintAxis {
    pub name: &'static str,
    pub values: &'static [&'static str],
    pub default_value: &'static str,
}

/// One `stylesheet!` registered for the dump: its class base
/// (`iy-<12 hex of the sheet-source hash>`), the generated
/// `<name>_style()` sheet constructor, and its variant axes.
pub struct PremintSheet {
    pub base_class: &'static str,
    pub sheet: fn() -> Rc<StyleSheet>,
    pub axes: &'static [PremintAxis],
}

#[linkme::distributed_slice]
pub static PREMINT_SHEETS: [PremintSheet] = [..];

/// Resolve a `StyleApplication` into the exact `(base, state overlays,
/// breakpoint overlays, container overlays)` tuple the web backend
/// hands `apply_styled_variants` — including the theme-default text
/// font fill on the base. The dump crate feeds this straight into
/// `css::class_rule_group`, so a preminted rule body is assembled by
/// the same code path a live-minted one would be.
#[allow(clippy::type_complexity)]
pub fn resolve_layers(
    app: &StyleApplication,
) -> (
    Rc<StyleRules>,
    Vec<(StateBits, Rc<StyleRules>)>,
    Vec<(crate::Breakpoint, Rc<StyleRules>)>,
    Vec<(f32, Rc<StyleRules>)>,
) {
    let base = crate::walker::style::with_default_text_font(crate::style::resolve(app));
    (
        base,
        crate::walker::style::resolve_state_overlays(app),
        crate::walker::style::resolve_breakpoint_overlays(app),
        crate::walker::style::resolve_container_overlays(app),
    )
}

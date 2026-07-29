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
//! `runtime_core::premint`). The dump reads each sheet's structure
//! through the `premint_*` accessors on [`StyleSheet`] (base rules,
//! per-arm DELTA rules, overlay-axis lists) and emits one rule per
//! layer; the runtime stamps one class per selected axis, and the CSS
//! source-order cascade reproduces `StyleSheet::resolve`'s later-wins
//! merge. No per-combo enumeration — CSS size is the SUM of arms, not
//! their cartesian product.

use crate::style::StyleSheet;
use std::rc::Rc;

/// Re-export for `stylesheet!`-generated registration code
/// (`#[linkme(crate = ::runtime_core::premint::linkme)]`).
pub use linkme;

/// One `stylesheet!` registered for the dump: its class base
/// (`iy-<12 hex of the sheet-source hash>`) and the generated
/// `<name>_style()` sheet constructor. Everything else the dump needs
/// (axes, arm deltas, overlay lists) is read from the sheet itself via
/// the `premint_*` accessors, so the entry can't drift from the sheet.
pub struct PremintSheet {
    pub base_class: &'static str,
    pub sheet: fn() -> Rc<StyleSheet>,
}

#[linkme::distributed_slice]
pub static PREMINT_SHEETS: [PremintSheet] = [..];

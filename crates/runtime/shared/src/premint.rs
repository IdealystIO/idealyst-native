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

// ---------------------------------------------------------------------------
// Runtime-assembled sheets
// ---------------------------------------------------------------------------
//
// The link-time slice above only reaches sheets with a `stylesheet!`
// expansion site. idea-theme's component sheets have none: they are
// assembled at runtime from builtin kinds/tones PLUS whatever the app
// registers before `install_idea_theme`, so their variant space simply
// does not exist until the app runs. That is why nine idea-ui
// components — button, badge, chip, tag, alert, toast, switch,
// icon_button, typography — never preminted and kept the live style
// engine linked in every bundle that used one.
//
// They can still be collected, because the dump binary BUILDS THE APP
// (`world.enter(|| app())`) before asking for CSS: by then the theme is
// installed and the sheets are fully assembled. `StyleSheet::premint_as`
// registers here as it hands out the `Rc`, so the dump sees every sheet
// the app installs, including ones no screen it renders would touch.
//
// A `Vec`, not a set: `premint_as` is called once per sheet per install
// and installs are not idempotent-by-identity (an app may reinstall a
// theme). `dump_all_css` dedups on the base class, which is derived from
// the identity, so a reinstall of identical content collapses to one
// rule set.

thread_local! {
    static ASSEMBLED: std::cell::RefCell<Vec<Rc<StyleSheet>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Record a runtime-assembled sheet for the dump. Called by
/// [`StyleSheet::premint_as`](crate::StyleSheet::premint_as); never by
/// app code.
pub fn register_assembled_sheet(sheet: &Rc<StyleSheet>) {
    ASSEMBLED.with(|s| s.borrow_mut().push(Rc::clone(sheet)));
}

/// Every runtime-assembled sheet registered so far, in registration
/// order. The dump calls this after building the app tree.
pub fn assembled_sheets() -> Vec<Rc<StyleSheet>> {
    ASSEMBLED.with(|s| s.borrow().clone())
}

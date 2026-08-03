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
// Framework-owned sheets
// ---------------------------------------------------------------------------

// The `if`-empty-branch anchor sheet itself lives in `crate::style`
// (`empty_absolute_sheet`) — it must exist in EVERY build, while this
// module only compiles under the dump's `style-dump` feature. Only the
// link-time registration lives here.
#[cfg(idealyst_premint_dump)]
#[linkme::distributed_slice(PREMINT_SHEETS)]
static EMPTY_ABSOLUTE_SHEET: PremintSheet = PremintSheet {
    base_class: crate::style::EMPTY_ABSOLUTE_CLASS,
    sheet: crate::style::empty_absolute_sheet,
};

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

// ---------------------------------------------------------------------------
// Auto-preminted STATIC sheets
// ---------------------------------------------------------------------------
//
// `StyleSheet::r#static` derives its premint class from the rules'
// content key, so the dump and the shipped bundle agree on the name with
// no hand-written identity at all. The registry captures (class, rules)
// at construction — the sheet itself is not yet `Rc`-wrapped there, so
// unlike `ASSEMBLED` this stores the raw parts. A later
// `premint_as`/`premint_with_class`/layer-adding call RETRACTS the entry
// (see `StyleSheet::retract_auto_premint`), so a sheet that graduates to
// an explicit identity doesn't also leave a dead auto-class rule in the
// asset. Deduped by class at dump time: content-equal sheets share one
// class by construction, and one rule serves them all.

thread_local! {
    static STATIC_RULES: std::cell::RefCell<Vec<(Rc<str>, crate::StyleRules)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Record an auto-preminted static sheet's parts for the dump. Called by
/// [`StyleSheet::r#static`](crate::StyleSheet::r#static); never by app code.
pub fn register_static_rules(class: Rc<str>, rules: crate::StyleRules) {
    STATIC_RULES.with(|s| s.borrow_mut().push((class, rules)));
}

/// Retract a previously-registered auto entry (the sheet gained an
/// explicit identity or a layer the auto class can't name).
pub fn unregister_static_rules(class: &str) {
    STATIC_RULES.with(|s| s.borrow_mut().retain(|(c, _)| &**c != class));
}

/// Every auto-preminted static sheet registered so far. The dump calls
/// this after building the app tree, like [`assembled_sheets`].
pub fn static_rules() -> Vec<(Rc<str>, crate::StyleRules)> {
    STATIC_RULES.with(|s| s.borrow().clone())
}

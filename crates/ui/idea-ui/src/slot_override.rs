//! Per-slot style overrides for idea-ui components.
//!
//! Every idea-ui component resolves its appearance from the installed theme
//! sheets. That's the right default, but authors regularly need a one-off
//! escape hatch — a dark-green label on a white button, a modal body that sits
//! flush with an edge-to-edge illustration header. Rebuilding the theme for a
//! single call site is overkill, and colouring via the web CSS cascade breaks
//! on native (text/icon colour doesn't inherit there — see `Button`).
//!
//! The convention: a component exposes an `Option<Rc<StyleSheet>>` prop per
//! **slot** — `style` for its root, plus one per sub-element it renders
//! (`label_style`, `icon_style`, `content_style`, …). Each override is layered
//! on top of that slot's resolved theme style via [`StyleApplication::with_overrides`],
//! which resolves LAST, so every field the author sets wins without disturbing
//! the rest of the theme style. Overrides are static (a `StyleSheet`, not a
//! reactive value); they resolve against the default [`VariantSet`].
//!
//! Authoring an override is a plain static sheet:
//!
//! ```ignore
//! Button(
//!     label = "Get started",
//!     tone = tone::Neutral,
//!     variant = variant::Filled,               // white surface
//!     label_style = Rc::new(StyleSheet::r#static(StyleRules {
//!         color: Some(Tokenized::token("intent-success-fg", Color("#0b6b3a".into()))),
//!         ..Default::default()
//!     })),                                       // dark-green label, native-safe
//! )
//! ```

use std::rc::Rc;

use runtime_core::{StyleApplication, StyleRules, StyleSheet, VariantSet};

/// Resolve an optional slot-override sheet to its [`StyleRules`]. `None` (the
/// common case) resolves to an empty rule set that merges to a no-op.
///
/// Overrides are resolved against the default [`VariantSet`] — they're ad-hoc
/// tweaks, not variant-driven theme sheets, so there's no active variant to
/// select. A themed override sheet still resolves (it just picks its own
/// defaults).
pub fn override_rules(sheet: &Option<Rc<StyleSheet>>) -> StyleRules {
    match sheet {
        Some(s) => s.resolve(&VariantSet::default()),
        None => StyleRules::default(),
    }
}

/// Layer an optional slot-override sheet on top of a resolved slot
/// [`StyleApplication`]. A `None` override returns `base` untouched; a `Some`
/// merges the override as the top resolution layer (see
/// [`StyleApplication::with_overrides`]).
pub fn apply_override(base: StyleApplication, sheet: &Option<Rc<StyleSheet>>) -> StyleApplication {
    match sheet {
        Some(s) => base.with_overrides(s.resolve(&VariantSet::default())),
        None => base,
    }
}

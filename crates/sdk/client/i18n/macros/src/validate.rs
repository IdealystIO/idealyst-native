//! Compile-time validation — the strong-typing guarantees.
//!
//! Each problem becomes a `syn::Error` spanned at the offending item, the
//! same friendly-diagnostic technique `runtime-macros`' `doc_check` uses.
//! All problems are collected (not just the first) so one compile surfaces
//! every gap.

use crate::parse::{placeholders, I18nInput};
use std::collections::HashSet;

/// The index of the `(default)` locale, returned for the emitter once the
/// input validates.
pub struct Model {
    pub default_idx: usize,
}

/// Returns `Ok(Model)` if everything checks out, else every error found.
pub fn check(input: &I18nInput) -> Result<Model, Vec<syn::Error>> {
    let mut errors = Vec::new();

    // --- locale header -----------------------------------------------------
    if input.locales.is_empty() {
        errors.push(syn::Error::new(
            proc_macro2::Span::call_site(),
            "`locales` must declare at least one locale",
        ));
        return Err(errors);
    }

    let mut seen_variants = HashSet::new();
    let mut seen_codes = HashSet::new();
    let mut default_indices = Vec::new();
    for (i, loc) in input.locales.iter().enumerate() {
        if !seen_variants.insert(loc.variant.to_string()) {
            errors.push(syn::Error::new_spanned(
                &loc.variant,
                format!("duplicate locale variant `{}`", loc.variant),
            ));
        }
        if !seen_codes.insert(loc.code.value()) {
            errors.push(syn::Error::new_spanned(
                &loc.code,
                format!("duplicate locale code `{}`", loc.code.value()),
            ));
        }
        if loc.is_default {
            default_indices.push(i);
            if loc.is_lazy {
                errors.push(syn::Error::new_spanned(
                    &loc.variant,
                    "the `default` locale cannot also be `lazy` — its strings are the \
                     compile-time fallback and must be bundled",
                ));
            }
        }
    }
    match default_indices.len() {
        1 => {}
        0 => errors.push(syn::Error::new(
            proc_macro2::Span::call_site(),
            "exactly one locale must be marked `(default)` — it is the reference \
             locale and the fallback when a translation is unavailable",
        )),
        _ => {
            for &i in &default_indices {
                errors.push(syn::Error::new_spanned(
                    &input.locales[i].variant,
                    "only one locale may be marked `(default)`",
                ));
            }
        }
    }

    // Bundled = every non-lazy locale; it must translate every message.
    let bundled: Vec<&crate::parse::LocaleDecl> =
        input.locales.iter().filter(|l| !l.is_lazy).collect();
    let lazy_variants: HashSet<String> =
        input.locales.iter().filter(|l| l.is_lazy).map(|l| l.variant.to_string()).collect();
    let known_variants: HashSet<String> =
        input.locales.iter().map(|l| l.variant.to_string()).collect();

    let default_idx = default_indices.first().copied().unwrap_or(0);
    let default_variant = input.locales[default_idx].variant.to_string();
    let default_code = input.locales[default_idx].code.value();

    // --- messages ----------------------------------------------------------
    let mut seen_messages = HashSet::new();
    for msg in &input.messages {
        if !seen_messages.insert(msg.name.to_string()) {
            errors.push(syn::Error::new_spanned(
                &msg.name,
                format!("duplicate message `{}`", msg.name),
            ));
        }

        // Args must be unique.
        let mut seen_args = HashSet::new();
        for arg in &msg.args {
            if !seen_args.insert(arg.to_string()) {
                errors.push(syn::Error::new_spanned(
                    arg,
                    format!("duplicate argument `{arg}` in message `{}`", msg.name),
                ));
            }
        }
        let arg_set: HashSet<String> = msg.args.iter().map(|a| a.to_string()).collect();

        // Per-entry checks: known variant, not-lazy, no dupes, valid placeholders.
        let mut entry_variants = HashSet::new();
        for entry in &msg.entries {
            let v = entry.variant.to_string();
            if !known_variants.contains(&v) {
                errors.push(syn::Error::new_spanned(
                    &entry.variant,
                    format!("`{v}` is not a declared locale"),
                ));
                continue;
            }
            if !entry_variants.insert(v.clone()) {
                errors.push(syn::Error::new_spanned(
                    &entry.variant,
                    format!("duplicate translation for `{v}` in message `{}`", msg.name),
                ));
            }
            if lazy_variants.contains(&v) {
                errors.push(syn::Error::new_spanned(
                    &entry.variant,
                    format!(
                        "`{v}` is a `lazy` (opt-in) locale; its strings come from a fetched \
                         pack, so remove this inline translation"
                    ),
                ));
            }

            // Every placeholder must name a declared argument.
            for ph in placeholders(&entry.template.value()) {
                if !arg_set.contains(&ph) {
                    errors.push(syn::Error::new_spanned(
                        &entry.template,
                        format!(
                            "placeholder `{{{ph}}}` has no matching argument in `{}({})`",
                            msg.name,
                            msg.args.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ")
                        ),
                    ));
                }
            }
        }

        // Every bundled locale must translate this message.
        for loc in &bundled {
            let v = loc.variant.to_string();
            if !entry_variants.contains(&v) {
                errors.push(syn::Error::new_spanned(
                    &msg.name,
                    format!(
                        "message `{}` is missing a translation for bundled locale `{v}` — \
                         add `{v}: \"…\"`, or mark `{v}` as `(lazy)` to load it at runtime",
                        msg.name
                    ),
                ));
            }
        }

        // Every declared argument must be used in the default locale's
        // translation, so a typo'd or dead argument is caught.
        if let Some(default_entry) = msg.entries.iter().find(|e| e.variant == default_variant) {
            let used: HashSet<String> =
                placeholders(&default_entry.template.value()).into_iter().collect();
            for arg in &msg.args {
                if !used.contains(&arg.to_string()) {
                    errors.push(syn::Error::new_spanned(
                        arg,
                        format!(
                            "argument `{arg}` is never used in the default (`{default_code}`) \
                             translation of message `{}`",
                            msg.name
                        ),
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(Model { default_idx })
    } else {
        Err(errors)
    }
}

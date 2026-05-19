//! Vue Composition API taxonomy.
//!
//! Vue's reactive primitives live in `<script setup>` blocks. The
//! lowering pass walks the setup body and reduces each known call
//! to a porter IR primitive; anything unknown becomes a hole.
//!
//! Template directives (`v-if`, `v-for`, `@click`, `:attr`,
//! `{{ expr }}`) are *not* primitives — they live in `<template>`
//! and lower into [`port_core::ir::JsxNode`] directly. See the
//! parser module for that mapping.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveClass {
    Mechanical(Mechanical),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanical {
    /// `ref(x)` → `signal!(x)`. The lowering pass rewrites `.value`
    /// reads to `.get()` and `.value = v` assigns to `.set(v)`.
    Ref,
    /// `computed(fn)` → derived signal. Same MVP shape as Solid's
    /// `createMemo`.
    Computed,
    /// `watchEffect(fn)` → `Effect::new(move || …)`. idealyst
    /// signals auto-track, so the implicit dep set matches.
    WatchEffect,
    /// `watch(source, cb)` → `Effect::new(move || { let _ = source.get(); cb })`.
    /// Vue's `watch` is "track these specific sources, run the
    /// callback when they change" — modeled as an effect that
    /// reads its sources up front so idealyst's auto-tracking
    /// picks them up.
    Watch,
    /// `onMounted(fn)` → `Effect::new` with `// onMounted`
    /// comment. Same lossy mapping as Solid's `onMount`.
    OnMounted,
    /// `onUnmounted(fn)` → cleanup closure. Hole when context is
    /// unclear.
    OnUnmounted,
}

// Note: `defineProps` / `withDefaults` / `defineEmits` aren't in
// the registry because they're handled directly by the script
// walker (which needs the type argument off the call expression,
// not the call shape alone). Anything else falls to `Unknown` and
// ports as a plain function call.

pub struct PrimitiveRegistry {
    entries: HashMap<&'static str, PrimitiveClass>,
}

impl PrimitiveRegistry {
    pub fn classify(&self, name: &str) -> PrimitiveClass {
        self.entries.get(name).copied().unwrap_or(PrimitiveClass::Unknown)
    }
}

pub fn builtin() -> PrimitiveRegistry {
    let mut e: HashMap<&'static str, PrimitiveClass> = HashMap::new();
    e.insert("ref", PrimitiveClass::Mechanical(Mechanical::Ref));
    e.insert("computed", PrimitiveClass::Mechanical(Mechanical::Computed));
    e.insert("watchEffect", PrimitiveClass::Mechanical(Mechanical::WatchEffect));
    e.insert("watch", PrimitiveClass::Mechanical(Mechanical::Watch));
    e.insert("onMounted", PrimitiveClass::Mechanical(Mechanical::OnMounted));
    e.insert("onUnmounted", PrimitiveClass::Mechanical(Mechanical::OnUnmounted));
    PrimitiveRegistry { entries: e }
}

/// Template-directive lowering table. Documented as data so the
/// porter's *template coverage* is one explicit reference.
///
/// Entries are `(source-directive, idealyst-attribute-or-control)`.
/// The lowering pass consults this when walking `<template>`.
pub const TEMPLATE_DIRECTIVES: &[(&str, &str)] = &[
    // Conditionals: `v-if="x"` → `if x.get() { … }` block in the
    // surrounding jsx! invocation. Lowered structurally by the
    // parser, not as an attribute.
    ("v-if", "if-block"),
    ("v-else-if", "else-if-block"),
    ("v-else", "else-block"),
    // Iteration: `v-for="item in items"` → `for item in items.get().iter() { … }`.
    ("v-for", "for-block"),
    // Event handler shorthand: `@click="onClick"` → `on_click={…}`.
    ("@click", "on_click"),
    ("@input", "on_input"),
    ("@change", "on_change"),
    // Bound attribute shorthand: `:value="x"` → `value={x.get()}`.
    (":bind", "expr-attr"),
    // Two-way binding: `v-model="x"` → bidirectional shim. Hole
    // for now since idealyst has no single equivalent primitive.
    ("v-model", "two-way-binding"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_is_mechanical() {
        assert!(matches!(
            builtin().classify("ref"),
            PrimitiveClass::Mechanical(Mechanical::Ref),
        ));
    }

    #[test]
    fn unknown_routes_to_unknown() {
        assert_eq!(builtin().classify("someCustomComposable"), PrimitiveClass::Unknown);
    }
}

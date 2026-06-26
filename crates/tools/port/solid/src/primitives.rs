//! Solid's reactive primitive taxonomy.
//!
//! Solid is unusually close to idealyst's runtime model — both
//! treat the component function as a setup function that runs
//! once and registers signals/effects/memos that drive future
//! updates. The translation table is short and almost entirely
//! mechanical:

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveClass {
    Mechanical(Mechanical),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanical {
    /// `const [v, setV] = createSignal(init)` → `let v = signal!(init);`
    /// Solid signals are *called* to read (`v()`) — the lowering pass
    /// rewrites reads to `v.get()` and `setV(x)` to `v.set(x)`.
    CreateSignal,
    /// `createEffect(fn)` → `effect!({ … })`.
    CreateEffect,
    /// `createMemo(fn)` → derived signal. The MVP emits an inline
    /// closure; future passes can upgrade to a memo signal.
    CreateMemo,
    /// `createResource(fn)` → an async-flavored signal pair. Lowered
    /// as a hole with the original call attached — there's no
    /// idealyst MVP equivalent yet.
    CreateResource,
    /// `onMount(fn)` → `effect!({ … })` with a `// onMount` marker
    /// comment. Solid's onMount is effectively "effect that runs
    /// once after first paint" — idealyst signals don't separate
    /// mount from update phases, so the lowering is one-way lossy.
    OnMount,
    /// `onCleanup(fn)` → a destructor closure registered on the
    /// surrounding scope. Lowered as a hole with the original
    /// attached when the surrounding context isn't obvious.
    OnCleanup,
    /// `useContext(Ctx)` → `inject::<Ctx>()`. Same shape as
    /// React's; the first argument's identifier is used as the
    /// Rust type name.
    UseContext,
}

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
    e.insert("createSignal", PrimitiveClass::Mechanical(Mechanical::CreateSignal));
    e.insert("createEffect", PrimitiveClass::Mechanical(Mechanical::CreateEffect));
    e.insert("createMemo", PrimitiveClass::Mechanical(Mechanical::CreateMemo));
    e.insert("createResource", PrimitiveClass::Mechanical(Mechanical::CreateResource));
    e.insert("onMount", PrimitiveClass::Mechanical(Mechanical::OnMount));
    e.insert("onCleanup", PrimitiveClass::Mechanical(Mechanical::OnCleanup));
    e.insert("useContext", PrimitiveClass::Mechanical(Mechanical::UseContext));
    PrimitiveRegistry { entries: e }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_signal_is_mechanical() {
        assert!(matches!(
            builtin().classify("createSignal"),
            PrimitiveClass::Mechanical(Mechanical::CreateSignal),
        ));
    }
}

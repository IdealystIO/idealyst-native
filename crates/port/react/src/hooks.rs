//! Hook detection taxonomy.
//!
//! React "hooks" aren't a framework concept in idealyst — they're
//! just function calls. The porter only needs to special-case
//! hooks whose call shape maps to an idealyst *primitive*:
//!
//! - [`HookClass::Mechanical`] — the lowering pass emits a
//!   framework-core equivalent (`useState` → `signal!`,
//!   `useEffect` → `Effect::new`).
//! - [`HookClass::Unknown`] — every other call (custom hooks,
//!   third-party hooks, `useContext`, `useReducer`, ...). The
//!   porter ports these as plain function calls. If a Rust
//!   function with the same name exists (user-written or
//!   ported from the hook body), the generated code links
//!   against it; otherwise the build fails with a clear name
//!   resolution error.
//!
//! Notably absent: there is no "Shimmed" category. The porter
//! does not invent a React-flavored runtime layer on top of
//! framework-core. If/when idealyst grows its own answer for
//! cross-tree data flow, refs, or reducer-style state, the
//! relevant call shapes can graduate from `Unknown` to
//! `Mechanical` with an explicit mapping.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookClass {
    Mechanical(Mechanical),
    Unknown,
}

/// Mechanical lowerings — the porter emits a direct
/// framework-core equivalent with no runtime shim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanical {
    /// `useState(init)` → `let [name] = signal!(init);`
    /// The lowering pass also records the matching setter ident
    /// so JSX/handler walkers can rewrite `setX(v)` → `x.set(v)`.
    UseState,
    /// `useEffect(fn, deps)` → `Effect::new(move || { … });`
    /// Deps are emitted as an informational comment — idealyst
    /// signals auto-track, so the dep list is redundant in the
    /// lowered code but useful for review.
    UseEffect,
    /// `useLayoutEffect(fn, deps)` → same as `useEffect`. The
    /// distinction (sync after DOM mutation) has no idealyst
    /// equivalent in the current MVP; we lower identically and
    /// note it in the emitted comment.
    UseLayoutEffect,
    /// `useMemo(fn, deps)` → a derived computation. The MVP
    /// emits an inline closure invocation; a future pass can
    /// upgrade to a memo-signal once the framework exposes one.
    UseMemo,
    /// `useCallback(fn, deps)` → just the inlined closure. deps
    /// are dropped (signals capture by value already).
    UseCallback,
    /// `useRef(init)` → `Ref::new()` when used for an imperative
    /// handle, or `signal!(init)` when used as a mutable cell.
    /// The lowering pass picks based on how the ref is used; if
    /// neither pattern matches it falls through to a hole.
    UseRef,
    /// `useContext(Ctx)` → `inject::<Ctx>()`. idealyst's context
    /// API is type-keyed, so the first argument's identifier is
    /// adopted verbatim as the Rust type name. The user defines
    /// the matching Rust type (typically by porting the original
    /// `createContext<T>(default)` into a `pub struct Ctx { … }`).
    UseContext,
}

/// The registry. Only call shapes that map to a framework-core
/// primitive are listed; everything else (custom hooks,
/// `useContext`, `useReducer`, ...) falls through to
/// [`HookClass::Unknown`] and is ported as a plain function call.
///
/// Adding a new mechanical lowering: a) variant on [`Mechanical`],
/// b) entry here, c) match arm in `port-react/src/parser.rs`
/// `classify_call`.
pub struct HookRegistry {
    entries: HashMap<&'static str, HookClass>,
}

impl HookRegistry {
    pub fn classify(&self, hook_name: &str) -> HookClass {
        self.entries.get(hook_name).copied().unwrap_or(HookClass::Unknown)
    }
}

pub fn builtin() -> HookRegistry {
    let mut entries: HashMap<&'static str, HookClass> = HashMap::new();
    entries.insert("useState", HookClass::Mechanical(Mechanical::UseState));
    entries.insert("useEffect", HookClass::Mechanical(Mechanical::UseEffect));
    entries.insert("useLayoutEffect", HookClass::Mechanical(Mechanical::UseLayoutEffect));
    entries.insert("useMemo", HookClass::Mechanical(Mechanical::UseMemo));
    entries.insert("useCallback", HookClass::Mechanical(Mechanical::UseCallback));
    entries.insert("useRef", HookClass::Mechanical(Mechanical::UseRef));
    entries.insert("useContext", HookClass::Mechanical(Mechanical::UseContext));
    HookRegistry { entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_use_state_as_mechanical() {
        assert!(matches!(
            builtin().classify("useState"),
            HookClass::Mechanical(Mechanical::UseState)
        ));
    }

    #[test]
    fn unknown_hooks_route_to_unknown() {
        assert_eq!(builtin().classify("useFancyCustomThing"), HookClass::Unknown);
    }
}

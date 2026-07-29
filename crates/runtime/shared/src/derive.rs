//! `Derived<T>` and `Action` — the canonical structured reactive
//! expressions.
//!
//! See [REACTIVITY_DSL_DESIGN.md](../../../../REACTIVITY_DSL_DESIGN.md)
//! for the design rationale. Quick summary: closures are opaque,
//! generator backends (like Roku) can't ship closures to the
//! device, so reactive expressions need both their runtime form
//! (a callable) and a structured description (method name +
//! signal inputs) carried in one type. `Derived<T>` is that type
//! for value-producing expressions; `Action` is the equivalent for
//! fire-and-forget event handlers.
//!
//! Authors don't construct these directly. The `ui!` macro
//! recognizes call shapes inside reactive positions and emits
//! `Derived<T>` / `Action` for the author. The types live here so
//! the macro + backends share a single canonical shape.
//!
//! ## Migration status
//!
//! Phase 0: these types exist but nothing in the framework
//! consumes them yet. The legacy `TextSource::Bound`,
//! `WhenBinding`, `ActionBinding`, and `*Decl` primitives still
//! carry the structure. Subsequent phases migrate primitives one
//! at a time to consume `Derived<T>` / `Action`, then delete the
//! legacy surface.

use std::rc::Rc;

/// A pure reactive transformation. Carries both the callable form
/// (for runtime backends that can re-evaluate closures whenever a
/// signal changes) and a serializable description (for generator
/// backends that emit a wire stream and can't ship closures).
///
/// All four fields are populated at construction; backends consume
/// whichever subset they need. The macro that constructs a
/// `Derived<T>` guarantees `compute` evaluates exactly what
/// `method(input_signals...)` would compute on the device, so the
/// two views are guaranteed consistent — both come from the same
/// author-side call expression.
///
/// `T` is the value type the transformation produces. Common
/// shapes:
/// - `Derived<String>` for reactive text content.
/// - `Derived<bool>` for the discriminant of a conditional.
/// - `Derived<usize>` for a list's count.
/// - `Derived<serde_json::Value>` for a switch discriminant when
///   the discriminant type varies (the pattern arms compare via
///   JSON equality).
///
/// Backends-side: runtime backends use `compute`; generator
/// backends serialize `method` + `inputs` + `initial`.
pub struct Derived<T> {
    /// Stable name of the pure transformation that produces `T`
    /// from the current values of `inputs`. Generator backends
    /// emit this symbol into their wire stream so the device-side
    /// runtime can dispatch to the transpiled implementation by
    /// name. Runtime backends can use it for tooling labels.
    pub method: &'static str,

    /// Arena ids of every signal `method` reads, in the order they
    /// appear in `method`'s parameter list. Generator backends use
    /// this to wire up device-side subscribers; runtime backends
    /// use it as the dependency set of the Effect they install.
    pub inputs: Vec<u64>,

    /// JSON snapshot of `inputs`' current values, captured at
    /// `Derived<T>` construction. Generator backends use this to
    /// declare each signal's initial value on the device so the
    /// first dispatch has something to read. Runtime backends
    /// ignore it — they read live values via `compute`.
    pub initial: Vec<crate::__serde_json::Value>,

    /// Runtime evaluator. Closes over the same signals that
    /// `inputs` names and calls the same logic that `method`
    /// transpiles to. Runtime backends call this on signal change.
    /// Generator backends never invoke it.
    pub compute: Rc<dyn Fn() -> T>,
}

impl<T> std::fmt::Debug for Derived<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Derived")
            .field("method", &self.method)
            .field("inputs", &self.inputs)
            .field("initial", &self.initial)
            .field("compute", &"<closure>")
            .finish()
    }
}

impl<T> Clone for Derived<T> {
    fn clone(&self) -> Self {
        Self {
            method: self.method,
            inputs: self.inputs.clone(),
            initial: self.initial.clone(),
            compute: self.compute.clone(),
        }
    }
}

/// A reactive event handler. Same dual-shape as `Derived<T>` but
/// for one-shot fires (button presses, gesture handlers, etc.)
/// rather than value-producing expressions.
///
/// `output` captures the read-modify-write idiom: an event's
/// handler computes a value from the input signals and writes the
/// result back to one of them. The classic counter button —
/// "increment count" — has `inputs = [count.id()]`, `output =
/// Some(count.id())`, and `method = "increment"`. Generator
/// backends use this to ship a self-contained "press handler"
/// that doesn't need to round-trip back to the host.
pub struct Action {
    /// Stable name of the transformation the event fires. Same
    /// resolution as `Derived::method`.
    pub method: &'static str,

    /// Signal ids the method reads (in parameter order).
    pub inputs: Vec<u64>,

    /// JSON snapshots of the input signals' current values,
    /// parallel to `inputs`.
    pub initial: Vec<crate::__serde_json::Value>,

    /// Optional signal id the method's return value writes back
    /// to. `None` for fire-and-forget actions; `Some` for the
    /// read-modify-write pattern.
    pub output: Option<u64>,

    /// Runtime evaluator: reads inputs, calls the transformation,
    /// writes the result to `output` (if set). Runtime backends
    /// hook this up to the native event; generator backends
    /// serialize `method` + `inputs` + `output` instead.
    pub fire: Rc<dyn Fn()>,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Action")
            .field("method", &self.method)
            .field("inputs", &self.inputs)
            .field("initial", &self.initial)
            .field("output", &self.output)
            .field("fire", &"<closure>")
            .finish()
    }
}

impl Clone for Action {
    fn clone(&self) -> Self {
        Self {
            method: self.method,
            inputs: self.inputs.clone(),
            initial: self.initial.clone(),
            output: self.output,
            fire: self.fire.clone(),
        }
    }
}

/// Coercion trait for accepting either a fully-built `Derived<T>`
/// (typically produced by a macro that recognized a `method(sigs)`
/// call shape) or a bare `Fn() -> T` closure at the same call site.
///
/// The closure path produces a `Derived<T>` with empty metadata
/// (`method: ""`, no inputs) — runtime backends use `compute`;
/// generator backends report a build-time error when they
/// encounter one because there's no method name to dispatch.
pub trait IntoDerived<T> {
    fn into_derived(self) -> Derived<T>;
}

impl<T> IntoDerived<T> for Derived<T> {
    fn into_derived(self) -> Derived<T> {
        self
    }
}

impl<F, T> IntoDerived<T> for F
where
    F: Fn() -> T + 'static,
{
    fn into_derived(self) -> Derived<T> {
        Derived {
            method: "",
            inputs: Vec::new(),
            initial: Vec::new(),
            compute: Rc::new(self),
        }
    }
}

impl<T> Derived<T> {
    /// True if this Derived carries no structured metadata — i.e.
    /// it was constructed from a bare closure rather than a
    /// `#[method]`-backed structured expression. Generator backends
    /// use this to detect "I can't ship this; the closure has no
    /// name."
    pub fn is_opaque(&self) -> bool {
        self.method.is_empty()
    }
}

/// Coercion trait for accepting either a fully-built `Action`
/// (typically produced by a macro that recognized a structured
/// call shape) or a bare `Fn()` closure at the same call site.
///
/// The closure path produces an `Action` with empty metadata
/// (`method: ""`, no inputs / output) — runtime backends use
/// `fire`; generator backends should report a build-time error
/// when they encounter one because there's no method name to
/// dispatch on the device.
pub trait IntoAction {
    fn into_action(self) -> Action;
}

impl IntoAction for Action {
    fn into_action(self) -> Action {
        self
    }
}

impl<F> IntoAction for F
where
    F: Fn() + 'static,
{
    fn into_action(self) -> Action {
        Action {
            method: "",
            inputs: Vec::new(),
            initial: Vec::new(),
            output: None,
            fire: Rc::new(self),
        }
    }
}

/// True if this Action carries no structured metadata — i.e. it
/// was constructed from a bare `Fn()` closure rather than a
/// `#[method]`-backed structured handler. Generator backends use
/// this to detect "I can't ship this; the closure has no name."
impl Action {
    pub fn is_opaque(&self) -> bool {
        self.method.is_empty()
    }
}

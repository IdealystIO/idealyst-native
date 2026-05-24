//! `DocControls` — reflective tooling for the self-referencing
//! documentation app.
//!
//! Behind the `docs` cargo feature. When on, every component's
//! `*Props` type derives [`DocControls`], giving the docs app a
//! uniform shape for building live preview + control-panel pairs:
//!
//! - `init_state()` builds a `*State` struct with one
//!   `Signal<T>` per controllable field.
//! - `from_state(&state)` reads the signals and produces a fresh
//!   `Props` value to invoke the component with.
//! - `render_controls(&state)` renders the control panel — itself
//!   composed of idea-ui components (Switch, Field, Pressable
//!   rows). That's the self-referencing part: documenting the
//!   library uses the library.
//!
//! ## Why this module lives in idea-ui rather than the docs crate
//!
//! Rust's orphan rule means a `DocControls for PressableProps`
//! impl has to live in either idea-ui (where `PressableProps`
//! lives) or `idea-ui-docs-core` (where the trait lives) — and
//! a `*-docs-core` crate can't depend on idea-ui *and* be
//! depended on by idea-ui (cycle). So the trait, helpers, and
//! the per-Props derives all live here, gated by the `docs`
//! feature to keep the dead-code cost out of production apps that
//! don't render the docs.
//!
//! ## Field-type → control mapping (handled by the derive)
//!
//! | Field type                  | Control rendered            |
//! |-----------------------------|-----------------------------|
//! | `String`                    | Field (text input)          |
//! | `bool`                      | Switch                      |
//! | `Option<String>`            | Switch + Field (when on)    |
//! | `T: VariantEnum`            | Pressable row (one per variant) |
//! | `Rc<dyn Intent>`            | Built-in intent picker      |
//! | callbacks (`Rc<dyn Fn…>`)   | (none — uses Default)       |
//! | other types                 | (none — falls back to Default) |
//!
//! Per-field attribute: `#[doc_control(skip)]` removes a field
//! from the panel; `#[doc_control(label = "…")]` overrides the
//! humanized label.

use std::rc::Rc;

use runtime_core::{ui, ChildList, Primitive, Signal, VariantEnum};

// `ui!`-lowered names need the local macros + variant types in
// scope.
#[allow(unused_imports)]
use crate::{
    card, caption, field, heading, select, stack, switch, HeadingKind,
    SelectOption as IdeaSelectOption, StackGap,
};

pub use idea_ui_docs_derive::DocControls;

// =============================================================================
// The trait
// =============================================================================

/// Every documented component's `*Props` type implements this.
/// The derive does it mechanically; you can hand-impl when the
/// auto-derive doesn't fit.
pub trait DocControls: Sized {
    /// Holds one signal per controllable field. The docs page
    /// constructs this once, then reads + writes it across renders.
    type State: Copy + 'static;

    /// Build a state value with default field values.
    fn init_state() -> Self::State;

    /// Read the state's signals and produce a fresh props value.
    ///
    /// **Not reactive on its own.** Calling this from a plain
    /// (non-reactive) scope produces a snapshot — once the props
    /// are passed to a component constructor, the resulting tree
    /// won't re-render when the signals change. Use
    /// [`reactive_preview`] to wrap the build site in a `switch`
    /// that does rebuild on signal changes.
    fn from_state(state: &Self::State) -> Self;

    /// Render a control panel that mutates `state`. The panel is
    /// composed entirely of idea-ui components — that's the
    /// self-referencing part.
    fn render_controls(state: &Self::State) -> Primitive;

    /// Wrap a preview-builder closure in a `switch` whose key
    /// reads every controllable signal. When any signal flips,
    /// the framework rebuilds the preview from a fresh
    /// `from_state(...)`.
    ///
    /// This is the seam that makes the docs app live — user
    /// components like `Pressable` aren't reactive at the
    /// individual prop level (only built-in primitives are), so
    /// the docs page wraps the whole preview tree in a switch
    /// that rebuilds whenever a control changes.
    ///
    /// Usage:
    /// ```ignore
    /// let state = PressableProps::init_state();
    /// let preview = PressableProps::reactive_preview(&state, |props| ui! {
    ///     Pressable(
    ///         label = props.label,
    ///         on_click = props.on_click,
    ///         intent = props.intent,
    ///         size = props.size
    ///     )
    /// });
    /// ```
    fn reactive_preview<F: Fn(Self) -> Primitive + 'static>(
        state: &Self::State,
        build: F,
    ) -> Primitive;
}

// =============================================================================
// Control helpers — used by the derive's expansion
// =============================================================================

/// A row of [label, control] used by every auto-generated control
/// panel.
pub fn control_row(label: &str, control: Primitive) -> Primitive {
    let label_text = label.to_string();
    let label_node = ui! { Caption(content = label_text) };
    let children = vec![label_node, control];
    ui! {
        Stack(gap = StackGap::Xs) { children }
    }
}

/// Wraps a list of control rows into a documented "controls panel"
/// surface — Card with a "Controls" heading and the rows stacked
/// below.
pub fn controls_panel(rows: Vec<Primitive>) -> Primitive {
    let mut children: Vec<Primitive> = Vec::with_capacity(rows.len() + 1);
    children.push(ui! { Heading(content = "Controls".to_string(), kind = HeadingKind::H3) });
    for r in rows {
        ChildList::append_to(r, &mut children);
    }
    ui! { Card { children } }
}

pub fn string_control(value: Signal<String>) -> Primitive {
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |s| value.set(s));
    ui! {
        Field(
            value = value,
            on_change = on_change,
            placeholder = Some("…".to_string())
        )
    }
}

/// `Option<String>` is rendered as a switch toggling presence + an
/// always-visible field that only matters when the switch is on.
pub fn optional_string_control(
    enabled: Signal<bool>,
    value: Signal<String>,
) -> Primitive {
    let on_toggle: Rc<dyn Fn(bool)> = Rc::new(move |b| enabled.set(b));
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |s| value.set(s));
    ui! {
        Stack(gap = StackGap::Xs) {
            Switch(
                label = Some("Set".to_string()),
                value = enabled,
                on_change = on_toggle
            )
            Field(
                value = value,
                on_change = on_change,
                placeholder = Some("…".to_string())
            )
        }
    }
}

/// Resolve an `Option<String>` from its (enabled, value) signal pair.
pub fn optional_string_value(enabled: Signal<bool>, value: Signal<String>) -> Option<String> {
    if enabled.get() {
        Some(value.get())
    } else {
        None
    }
}

pub fn bool_control(value: Signal<bool>) -> Primitive {
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |b| value.set(b));
    ui! { Switch(value = value, on_change = on_change) }
}

/// Variant-enum picker rendered as a `Select`. The user-facing
/// signal holds typed `E` values; the Select internally binds to a
/// shadow `Signal<String>` whose string ids we keep in sync via a
/// pair of effects.
pub fn variant_enum_control<E>(value: Signal<E>) -> Primitive
where
    E: VariantEnum + PartialEq + 'static,
{
    use runtime_core::Effect;

    let variants = E::all_variants();
    let options: Vec<IdeaSelectOption> = variants
        .iter()
        .map(|v| {
            let s = v.as_variant_str();
            IdeaSelectOption::new(s, s)
        })
        .collect();

    // Shadow signal — Select's controlled binding. We mirror the
    // typed `value` into `shadow` via an Effect, and the reverse
    // direction goes through the Select's `on_change` callback.
    let initial = value.get().as_variant_str().to_string();
    let shadow: Signal<String> = Signal::new(initial);

    // Forward typed → string. The framework's `run_effect`
    // short-circuits re-entrant invocations for the same effect id,
    // so the `shadow.set` below doesn't recursively re-fire this
    // effect and corrupt its subscription set.
    //
    // `mem::forget` matches `memo_with` / `resource` / animation
    // bindings: when called inside an active render scope (the common
    // case) the scope adopted the effect (`owns: false`) and the
    // forget is a no-op. When called outside any scope (tests,
    // ad-hoc construction) the forget pins the effect to thread
    // lifetime so the typed→shadow sync survives past return — pre-fix
    // the `let _e_typed_to_shadow` drop fired at end-of-statement and
    // silently cancelled the sync.
    let e_typed_to_shadow = Effect::new(move || {
        let s = value.get().as_variant_str().to_string();
        if shadow.get() != s {
            shadow.set(s);
        }
    });
    std::mem::forget(e_typed_to_shadow);

    let on_change: Rc<dyn Fn(String)> = {
        let variants = variants;
        Rc::new(move |picked: String| {
            for &v in variants {
                if v.as_variant_str() == picked {
                    if value.get() != v {
                        value.set(v);
                    }
                    break;
                }
            }
        })
    };

    ui! {
        Select(
            value = shadow,
            on_change = on_change,
            options = options,
            placeholder = Some("Pick".to_string())
        )
    }
}

// Legacy IntentKind / intent_control removed: doc-controls now use
// `IntentTag` directly through the generic `variant_enum_control`
// path, since `IntentTag` implements `VariantEnum`.

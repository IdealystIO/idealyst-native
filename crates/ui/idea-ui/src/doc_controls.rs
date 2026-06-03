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

use runtime_core::{ui, ChildList, Element, Signal, VariantEnum};

// `ui!`-lowered names need the local macros + variant types in
// scope.
#[allow(unused_imports)]
use crate::{
    Card, Field, Select, SelectOption as IdeaSelectOption, Stack, StackGap, Switch, Typography,
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
    fn render_controls(state: &Self::State) -> Element;

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
    fn reactive_preview<F: Fn(Self) -> Element + 'static>(
        state: &Self::State,
        build: F,
    ) -> Element;
}

// =============================================================================
// Control helpers — used by the derive's expansion
// =============================================================================

/// A row of [label, control] used by every auto-generated control
/// panel.
pub fn control_row(label: &str, control: Element) -> Element {
    let label_text = label.to_string();
    let label_node = ui! { Typography(content = label_text, kind = crate::typography_kind::Caption) };
    let children = vec![label_node, control];
    ui! {
        Stack(gap = StackGap::Xs) { children }
    }
}

/// Wraps a list of control rows into a documented "controls panel"
/// surface — Card with a "Controls" heading and the rows stacked
/// below.
pub fn controls_panel(rows: Vec<Element>) -> Element {
    let mut children: Vec<Element> = Vec::with_capacity(rows.len() + 1);
    children.push(ui! { Typography(content = "Controls".to_string(), kind = crate::typography_kind::H3) });
    for r in rows {
        ChildList::append_to(r, &mut children);
    }
    ui! { Card { children } }
}

pub fn string_control(value: Signal<String>) -> Element {
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
) -> Element {
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

pub fn bool_control(value: Signal<bool>) -> Element {
    let on_change: Rc<dyn Fn(bool)> = Rc::new(move |b| value.set(b));
    ui! { Switch(value = value, on_change = on_change) }
}

/// Variant-enum picker rendered as a `Select`. The user-facing
/// signal holds typed `E` values; the Select internally binds to a
/// shadow `Signal<String>` whose string ids we keep in sync via a
/// pair of effects.
pub fn variant_enum_control<E>(value: Signal<E>) -> Element
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
    // `persist()` is adopt-or-pin: inside an active render scope (the
    // common case) the scope already owns the effect and this is a no-op;
    // outside any scope (tests, ad-hoc construction) it pins the effect to
    // thread lifetime so the typed→shadow sync survives past return — pre-
    // fix the `let _e_typed_to_shadow` drop fired at end-of-statement and
    // silently cancelled the sync.
    Effect::new(move || {
        let s = value.get().as_variant_str().to_string();
        if shadow.get() != s {
            shadow.set(s);
        }
    })
    .persist();

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

/// Generic picker over a typed `*Ref` handle (`ToneRef`,
/// `VariantRef`, `ButtonSizeRef`, `ShapeRef`, `TypographyKindRef`).
/// Renders a Select populated from the type's `builtins_list()` —
/// users see one row per built-in modifier and clicking changes the
/// active selection.
///
/// App code with custom modifier types implements
/// [`idea_theme::extensible::RefBuiltins`] on the relevant `*Ref`
/// wrapper to plug into this same picker.
pub fn ref_picker_control<T: idea_theme::extensible::RefBuiltins>(
    value: Signal<T>,
) -> Element {
    use runtime_core::Effect;

    let builtins = T::builtins_list();
    let options: Vec<IdeaSelectOption> = builtins
        .iter()
        .map(|(name, _)| IdeaSelectOption::new(*name, *name))
        .collect();

    // Same shadow-signal pattern as `variant_enum_control` — the
    // Select binds to a `Signal<String>` while the typed `*Ref` value
    // is mirrored via an Effect.
    let initial = value.get().current_key().to_string();
    let shadow: Signal<String> = Signal::new(initial);

    // adopt-or-pin; see `variant_enum_control` above.
    Effect::new(move || {
        let s = value.get().current_key().to_string();
        if shadow.get() != s {
            shadow.set(s);
        }
    })
    .persist();

    let on_change: Rc<dyn Fn(String)> = Rc::new(move |picked: String| {
        // Re-enumerate per call so we can return owned `T` from the
        // matched arm without lifetime headaches over `&builtins`.
        for (name, t_ref) in T::builtins_list() {
            if name == picked {
                value.set(t_ref);
                break;
            }
        }
    });

    ui! {
        Select(
            value = shadow,
            on_change = on_change,
            options = options,
            placeholder = Some("Pick".to_string())
        )
    }
}

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

use framework_core::{ui, ChildList, Primitive, Signal, VariantEnum};

// `ui!`-lowered names need the local macros + variant types in
// scope.
#[allow(unused_imports)]
use crate::{
    card, caption, field, heading, hstack, select, switch, vstack, HeadingKind, Intent,
    IntoRcIntent, SelectOption as IdeaSelectOption, StackGap,
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
        VStack(gap = StackGap::Xs) { children }
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
        VStack(gap = StackGap::Xs) {
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
    use framework_core::Effect;

    let variants = E::all_variants();
    let options: Vec<IdeaSelectOption> = variants
        .iter()
        .map(|v| {
            let s = v.as_variant_str();
            IdeaSelectOption::new(s, s)
        })
        .collect();

    // Shadow signal — Select's controlled binding. We keep it in
    // sync with `value` via two effects:
    //   1. `value` → `shadow` whenever the typed signal flips.
    //   2. `shadow` → `value` when the user picks an option (via
    //      `on_change`); we resolve the string back to a variant
    //      and write it.
    //
    // The two-effect dance avoids a feedback loop because
    // `Signal::set` no-ops when the new value equals the current.
    let initial = value.get().as_variant_str().to_string();
    let shadow: Signal<String> = Signal::new(initial);

    // Forward typed → string.
    let _e_typed_to_shadow = Effect::new(move || {
        let s = value.get().as_variant_str().to_string();
        if shadow.get() != s {
            shadow.set(s);
        }
    });

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

/// Intent picker rendered as a `Select`. Same bridging shape as
/// [`variant_enum_control`] — the user signal holds [`IntentKind`];
/// a shadow `Signal<String>` drives the Select; an Effect keeps
/// the two synced.
pub fn intent_control(value: Signal<IntentKind>) -> Primitive {
    use framework_core::Effect;

    let kinds = IntentKind::all();
    let options: Vec<IdeaSelectOption> = kinds
        .iter()
        .map(|k| IdeaSelectOption::new(k.id(), k.name()))
        .collect();

    let initial = value.get().id().to_string();
    let shadow: Signal<String> = Signal::new(initial);
    let _e = Effect::new(move || {
        let s = value.get().id().to_string();
        if shadow.get() != s {
            shadow.set(s);
        }
    });

    let on_change: Rc<dyn Fn(String)> = Rc::new(move |picked: String| {
        if let Some(kind) = IntentKind::from_id(&picked) {
            if value.get() != kind {
                value.set(kind);
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

/// Names every built-in idea-ui intent. The docs use this as a
/// reflective vocabulary — `Rc<dyn Intent>` can't be enumerated
/// from outside, so we keep a parallel enumerable type for the
/// control panel.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IntentKind {
    Primary,
    Secondary,
    Neutral,
    Ghost,
    Success,
    Warning,
    Danger,
}

impl Default for IntentKind {
    fn default() -> Self {
        IntentKind::Primary
    }
}

impl IntentKind {
    pub fn all() -> &'static [IntentKind] {
        &[
            IntentKind::Primary,
            IntentKind::Secondary,
            IntentKind::Neutral,
            IntentKind::Ghost,
            IntentKind::Success,
            IntentKind::Warning,
            IntentKind::Danger,
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            IntentKind::Primary => "Primary",
            IntentKind::Secondary => "Secondary",
            IntentKind::Neutral => "Neutral",
            IntentKind::Ghost => "Ghost",
            IntentKind::Success => "Success",
            IntentKind::Warning => "Warning",
            IntentKind::Danger => "Danger",
        }
    }

    /// Stable string identifier. Used when `IntentKind` rides
    /// through a `Signal<String>` (e.g. as the binding for a
    /// `Select` whose ids are strings).
    pub fn id(&self) -> &'static str {
        match self {
            IntentKind::Primary => "primary",
            IntentKind::Secondary => "secondary",
            IntentKind::Neutral => "neutral",
            IntentKind::Ghost => "ghost",
            IntentKind::Success => "success",
            IntentKind::Warning => "warning",
            IntentKind::Danger => "danger",
        }
    }

    /// Inverse of [`id`]. Returns `None` for unknown ids.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "primary" => Some(IntentKind::Primary),
            "secondary" => Some(IntentKind::Secondary),
            "neutral" => Some(IntentKind::Neutral),
            "ghost" => Some(IntentKind::Ghost),
            "success" => Some(IntentKind::Success),
            "warning" => Some(IntentKind::Warning),
            "danger" => Some(IntentKind::Danger),
            _ => None,
        }
    }

    pub fn into_rc(self) -> Rc<dyn Intent> {
        match self {
            IntentKind::Primary => crate::Primary.into_rc(),
            IntentKind::Secondary => crate::Secondary.into_rc(),
            IntentKind::Neutral => crate::Neutral.into_rc(),
            IntentKind::Ghost => crate::Ghost.into_rc(),
            IntentKind::Success => crate::Success.into_rc(),
            IntentKind::Warning => crate::Warning.into_rc(),
            IntentKind::Danger => crate::Danger.into_rc(),
        }
    }
}

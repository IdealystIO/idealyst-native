//! `Select` — a controlled dropdown.
//!
//! Visually: a trigger button showing the current option's label.
//! Clicking opens a popover-anchored menu listing every option;
//! clicking an option updates the bound signal + closes the menu.
//!
//! Same shape as `Tabs`: string-keyed for type-system simplicity.
//! Hosts that want a typed enum on top wrap that themselves —
//! generic component types don't play well with idea-ui's
//! invocation-macro pattern.
//!
//! ```ignore
//! let value = signal!("pear".to_string());
//! let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| value.set(v));
//! ui! {
//!     Select(
//!         value = value,
//!         on_change = on_change,
//!         options = vec![
//!             SelectOption::new("apple",  "Apple"),
//!             SelectOption::new("pear",   "Pear"),
//!             SelectOption::new("banana", "Banana"),
//!         ]
//!     )
//! }
//! ```
//!
//! Dismiss semantics: `BackdropMode::None` keeps the page behind
//! interactive, but the web backend's click-outside listener fires
//! `on_dismiss` when the user clicks anywhere outside the menu.
//! Escape closes too. No focus trap.

use std::rc::Rc;

use framework_core::primitives::overlay::{
    AnchorTarget, BackdropMode, ElementAlign, ElementAnchor, ElementSide, OverlayAnchor,
};
use framework_core::{
    signal, ui, IntoPrimitive, PressableHandle, Primitive, Ref, Signal, StyleApplication,
    VariantEnum,
};

use crate::stylesheets::{SelectMenu, SelectOption as SelectOptionStyle, SelectTrigger};
use crate::theme::IdeaThemeRef;

pub use crate::stylesheets::SelectTriggerSize as SelectSize;

/// One choice in the menu. `id` is the value written to the bound
/// signal when the user picks this option; `label` is what's shown.
#[derive(Clone)]
pub struct SelectOption {
    pub id: String,
    pub label: String,
}

impl SelectOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into() }
    }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct SelectProps {
    /// Controlled — the host owns this signal. `Select` reads it
    /// to decide which option's label to show on the trigger, and
    /// writes to it via `on_change` on pick.
    pub value: Signal<String>,
    pub on_change: Rc<dyn Fn(String)>,
    /// The menu's choices. Empty list ⇒ menu has no rows; trigger
    /// shows the placeholder (or empty if none was set).
    pub options: Vec<SelectOption>,
    /// Trigger size — sm / md / lg, matching Field.
    pub size: SelectSize,
    /// Shown on the trigger when `value` doesn't match any option's
    /// id. Useful for "Choose..." prompts.
    pub placeholder: Option<String>,
}

impl Default for SelectProps {
    fn default() -> Self {
        Self {
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            options: Vec::new(),
            size: SelectSize::default(),
            placeholder: None,
        }
    }
}

pub fn select(props: SelectProps) -> Primitive {
    let value = props.value;
    let on_change = props.on_change.clone();
    let size = props.size;
    let placeholder = props.placeholder.clone();
    // Shared across the trigger's label closure + the menu's
    // option rows; cheap-clone of `Rc<Vec<SelectOption>>`.
    let options = Rc::new(props.options);

    // Per-instance state.
    let open: Signal<bool> = signal!(false);
    let trigger_ref: Ref<PressableHandle> = Ref::new();

    // ---- Trigger ----
    //
    // Built on the framework's `pressable` primitive (a tappable
    // `<div>`, no `<button>` UA chrome) so the SelectTrigger
    // stylesheet drives the whole visual without fighting any
    // browser defaults. The label is a reactive `Text` child whose
    // closure reads `value` and resolves it through the option
    // list, so the trigger label updates whenever the selection
    // flips programmatically.
    let label_options = options.clone();
    let label_placeholder = placeholder.clone();
    let label_source: framework_core::TextSource = framework_core::IntoTextSource::into_text_source(
        move || {
            label_options
                .iter()
                .find(|o| o.id == value.get())
                .map(|o| o.label.clone())
                .or_else(|| label_placeholder.clone())
                .unwrap_or_default()
        },
    );
    let label_child = framework_core::Primitive::Text {
        source: label_source,
        style: None,
        ref_fill: None,
    };
    let trigger_style = move || {
        let _ = framework_core::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        StyleApplication::new(SelectTrigger::sheet())
            .with("size", size.as_variant_str().to_string())
    };
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let trigger = framework_core::pressable(vec![label_child], move || (on_open)())
        .with_style(trigger_style)
        .bind(trigger_ref)
        .into_primitive();

    // ---- Menu ----
    //
    // Gated by `when(open.get(), …, …)`: only mounts while open.
    // Each open-flip rebuilds the menu from scratch, which means
    // the row-state lives inside the menu's per-open scope and
    // drops cleanly on close. Builder closures live in
    // `menu_build` below to keep the surrounding code readable.
    let menu_options = options.clone();
    let menu_on_change = on_change.clone();
    let menu_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let menu = framework_core::when(
        move || open.get(),
        move || {
            menu_build(
                value,
                menu_options.clone(),
                menu_on_change.clone(),
                menu_close.clone(),
                trigger_ref,
            )
        },
        || ui! { View {} }.into_primitive(),
    );

    ui! {
        View {
            trigger
            menu
        }
    }
}

/// Build the menu primitive for a single open cycle. The `when`
/// branch flip calls this each time `open` goes false → true, so
/// the menu's scope (including per-row event closures) frees on
/// close + re-mounts on reopen.
fn menu_build(
    value: Signal<String>,
    options: Rc<Vec<SelectOption>>,
    on_change: Rc<dyn Fn(String)>,
    menu_close: Rc<dyn Fn()>,
    trigger_ref: Ref<PressableHandle>,
) -> Primitive {
    let mut rows: Vec<Primitive> = Vec::with_capacity(options.len());
    for option in options.iter() {
        let opt_id = option.id.clone();
        let opt_label = option.label.clone();
        let on_change_for_row = on_change.clone();
        let menu_close_for_row = menu_close.clone();
        let on_click: Rc<dyn Fn()> = Rc::new({
            let opt_id = opt_id.clone();
            move || {
                (on_change_for_row)(opt_id.clone());
                (menu_close_for_row)();
            }
        });

        // The row's active-variant style closure reads `value` so
        // the highlight flips reactively when the selection
        // changes — no row rebuild required.
        let opt_id_for_style = opt_id.clone();
        let row_style = move || {
            let _ = framework_core::active_theme()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            let variant = if value.get() == opt_id_for_style { "on" } else { "off" };
            StyleApplication::new(SelectOptionStyle::sheet())
                .with("active", variant.to_string())
        };

        let label_child = framework_core::Primitive::Text {
            source: framework_core::TextSource::Static(opt_label),
            style: None,
            ref_fill: None,
        };
        let row = framework_core::pressable(vec![label_child], move || (on_click)())
            .with_style(row_style)
            .into_primitive();
        rows.push(row);
    }

    let menu_style = SelectMenu();
    framework_core::overlay(vec![ui! { View(style = menu_style) { rows } }])
        .anchor(OverlayAnchor::Element(ElementAnchor {
            target: AnchorTarget::from(trigger_ref),
            side: ElementSide::Below,
            align: ElementAlign::Start,
            offset: 4.0,
        }))
        .backdrop(BackdropMode::None)
        .trap_focus(false)
        .on_dismiss(move || (menu_close)())
        .into_primitive()
}

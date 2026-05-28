//! `Select` — controlled dropdown. Ported to the extensible
//! namespace; same API as the closed-enum
//! [`crate::components::select`]. The component's only stylable axis
//! today is `size` (trigger height) — adding a Tone trait would let
//! authors color the trigger when the value carries semantic meaning
//! (e.g. selecting an alert level). Not wired yet; lands here when
//! there's a concrete use case.
//!
//! ```ignore
//! ui! {
//!     Select(
//!         value = value,
//!         on_change = on_change,
//!         options = vec![
//!             SelectOption::new("apple", "Apple"),
//!             SelectOption::new("pear", "Pear"),
//!         ],
//!     )
//! }
//! ```

use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide};
use runtime_core::{
    signal, ui, IntoElement, PressableHandle, Element, Reactive, Ref, Signal, StyleApplication,
    VariantEnum,
};

use idea_theme::theme::IdeaThemeRef;

use crate::stylesheets::{SelectMenu, SelectOption as SelectOptionStyle, SelectTrigger};

pub use crate::stylesheets::SelectTriggerSize as SelectSize;

#[derive(Clone)]
pub struct SelectOption {
    pub id: String,
    /// Row label. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
}

impl SelectOption {
    pub fn new(id: impl Into<String>, label: impl Into<Reactive<String>>) -> Self {
        Self { id: id.into(), label: label.into() }
    }
}

pub struct SelectProps {
    pub value: Signal<String>,
    pub on_change: Rc<dyn Fn(String)>,
    pub options: Vec<SelectOption>,
    pub size: SelectSize,
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

pub fn select(props: SelectProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let size = props.size;
    let placeholder = props.placeholder.clone();
    let options = Rc::new(props.options);

    let open: Signal<bool> = signal!(false);
    let trigger_ref: Ref<PressableHandle> = Ref::new();

    let label_options = options.clone();
    let label_placeholder = placeholder.clone();
    let label_source: runtime_core::TextSource = runtime_core::IntoTextSource::into_text_source(
        move || {
            label_options
                .iter()
                .find(|o| o.id == value.get())
                .map(|o| o.label.get())
                .or_else(|| label_placeholder.clone())
                .unwrap_or_default()
        },
    );
    let label_child = runtime_core::text(label_source).into_element();
    let trigger_style = move || {
        let _ = idea_theme::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        StyleApplication::new(SelectTrigger::sheet())
            .with("size", size.as_variant_str().to_string())
    };
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let trigger = runtime_core::pressable(vec![label_child], move || (on_open)())
        .with_style(trigger_style)
        .bind(trigger_ref)
        .into_element();

    let menu_options = options.clone();
    let menu_on_change = on_change.clone();
    let menu_close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let menu = runtime_core::when(
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
        || ui! { View {} }.into_element(),
    );

    ui! {
        View {
            trigger
            menu
        }
    }
}

fn menu_build(
    value: Signal<String>,
    options: Rc<Vec<SelectOption>>,
    on_change: Rc<dyn Fn(String)>,
    menu_close: Rc<dyn Fn()>,
    trigger_ref: Ref<PressableHandle>,
) -> Element {
    let mut rows: Vec<Element> = Vec::with_capacity(options.len());
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

        let opt_id_for_style = opt_id.clone();
        let row_style = move || {
            let _ = idea_theme::active_theme()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            let variant = if value.get() == opt_id_for_style { "on" } else { "off" };
            StyleApplication::new(SelectOptionStyle::sheet()).with("active", variant.to_string())
        };

        // `opt_label` is a `Reactive<String>` — route it through
        // `IntoTextSource` (Static→Static, Dynamic→Bound) so a live
        // option label re-paints its row.
        let label_child = runtime_core::text(opt_label).into_element();
        let row = runtime_core::pressable(vec![label_child], move || (on_click)())
            .with_style(row_style)
            .into_element();
        rows.push(row);
    }

    let menu_style = SelectMenu();
    runtime_core::anchored_overlay(
        AnchorTarget::from(trigger_ref),
        vec![ui! { View(style = menu_style) { rows } }],
    )
    .side(ElementSide::Below)
    .align(ElementAlign::Start)
    .offset(4.0)
    .backdrop(BackdropMode::None)
    .trap_focus(false)
    .on_dismiss(move || (menu_close)())
    .into_element()
}

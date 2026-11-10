//! The component playground: the live component on a canvas, a panel with
//! one control per prop, and the `ui!` call the current controls make.
//!
//! The catalog records each component's exported preview symbol; the
//! build-time `component_preview` / `recipe_renderer` lookups (generated
//! by `preview_codegen` into `OUT_DIR/previews.rs`) turn a symbol into a
//! callable. A [`Preview`] lists its controls as plain data
//! (`runtime_core::previews::Control`); this module decides how each kind
//! is drawn, with idea-ui components.
//!
//! State: one `Signal<Vec<ControlValue>>` per playground, one value per
//! control. Each editor keeps its own typed signal for its idea-ui input
//! and reports the new `ControlValue` upward. The component on the canvas
//! and the Usage snippet are keyed rebuilds over the vector, so editing a
//! control never rebuilds the controls; "Reset props" rebuilds the panel
//! (a generation counter) so every editor starts again from its initial
//! value.

use std::cell::RefCell;
use std::rc::Rc;

use idea_ui::{
    size, typography_kind, variant, Button, Checkbox, ControlSize, Field, SegmentOption, SegmentedControl, Select, SelectOption,
    Stack, StackGap, Switch, Typography,
};
use runtime_core::previews::{Control, ControlValue, FieldSpec, IconChoice, Preview};
use runtime_core::{component, signal, switch, ui, Element, WriteSignal};

use crate::shell::CodePanel;
use crate::styles::{
    Canvas, CanvasColumn, CanvasToolbar, FillFrame, PanelCaption, PhoneFrame, PlaygroundBody, PlaygroundCard,
    PropName, PropRow, PropRowHead, PropType, PropsPanel, ToolbarSpacer, UsageBar, UsageHint,
    UsageTitle,
};

include!(concat!(env!("OUT_DIR"), "/previews.rs"));

/// The icons an icon prop's picker offers (`previews::set_icon_choices`):
/// a handful of common Lucide glyphs, named as in the pack. Registered once
/// at boot, before any preview reads its controls.
pub fn register_icon_choices() {
    macro_rules! lucide {
        ($($name:literal => $konst:ident),* $(,)?) => {
            vec![$(IconChoice {
                name: $name,
                code: concat!("icons_lucide::", stringify!($konst)),
                icon: icons_lucide::$konst,
            }),*]
        };
    }
    runtime_core::previews::set_icon_choices(lucide![
        "star" => STAR,
        "heart" => HEART,
        "check" => CHECK,
        "x" => X,
        "plus" => PLUS,
        "search" => SEARCH,
        "settings" => SETTINGS,
        "user" => USER,
        "bell" => BELL,
        "mail" => MAIL,
        "calendar" => CALENDAR,
        "house" => HOUSE,
        "image" => IMAGE,
        "info" => INFO,
        "triangle-alert" => TRIANGLE_ALERT,
        "download" => DOWNLOAD,
        "trash-2" => TRASH_2,
        "arrow-right" => ARROW_RIGHT,
    ]);
}

/// What an editor reports when its value changes.
type OnValue = Rc<dyn Fn(ControlValue)>;

fn ignore() -> OnValue {
    Rc::new(|_| {})
}

/// A choice this short is a segmented control; a longer one is a select.
const SEGMENTED_MAX_OPTIONS: usize = 4;
/// …and its labels must fit the panel's width side by side.
const SEGMENTED_MAX_CHARS: usize = 28;

/// The value a fresh editor of `control` starts at when nothing supplies
/// one.
fn empty_value(control: &Control) -> ControlValue {
    match control {
        Control::Text => ControlValue::Text(String::new()),
        Control::Bool => ControlValue::Bool(false),
        Control::Number { min, .. } => ControlValue::Number(min.unwrap_or(0.0).max(0.0)),
        Control::Choice(keys) => ControlValue::Choice(keys.first().cloned().unwrap_or_default()),
        Control::Optional { .. } => ControlValue::Optional(None),
        Control::Group(parts) => ControlValue::Group(parts.iter().map(|(_, c)| empty_value(c)).collect()),
        Control::Handler => ControlValue::Handler,
    }
}

/// `n` as a number field shows it: no trailing `.0` on whole numbers.
fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

/// The declared type with every path dropped to its last segment
/// (`::runtime_core::Reactive<Option<String>>` → `Reactive<Option<String>>`).
pub fn bare_type(declared: &str) -> String {
    // Token text arrives spaced (`Reactive < String >`): drop whitespace,
    // except one space between two words (`dyn Fn`).
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut compact = String::new();
    let mut pending_space = false;
    for c in declared.chars() {
        if c.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && word(c) && compact.chars().last().is_some_and(word) {
            compact.push(' ');
        }
        pending_space = false;
        compact.push(c);
    }
    let mut out = String::new();
    let mut ident = String::new();
    for c in compact.chars() {
        if c.is_alphanumeric() || c == '_' || c == ':' {
            ident.push(c);
        } else {
            out.push_str(ident.rsplit("::").next().unwrap_or(&ident));
            ident.clear();
            out.push(c);
        }
    }
    out.push_str(ident.rsplit("::").next().unwrap_or(&ident));
    out
}

/// The type a prop row shows: [`bare_type`] with the `Reactive` / `Signal`
/// wrapper — how the value is passed, not what it is — unwrapped
/// (`::runtime_core::Reactive<ToneRef>` → `ToneRef`).
pub fn short_type(declared: &str) -> String {
    let bare = bare_type(declared);
    for wrapper in ["Reactive<", "Signal<", "ReadSignal<"] {
        if let Some(inner) = bare.strip_prefix(wrapper).and_then(|s| s.strip_suffix('>')) {
            return inner.to_string();
        }
    }
    bare
}

/// Whether a field is shown in the panel. A required callback is always
/// attached (the component's default), so it has nothing to edit.
fn has_row(field: &FieldSpec) -> bool {
    field.control != Control::Handler
}

/// The `ui!` call the current values make. Props still at the
/// component's default are left out, as a caller would; the children
/// slot becomes a child block.
pub fn usage_code(component: &str, preview: &Preview, values: &[ControlValue]) -> String {
    let mut props = Vec::new();
    let mut children = None;
    for (index, (field, value)) in preview.fields.iter().zip(values).enumerate() {
        if *value == field.default {
            continue;
        }
        let code = preview.code(index, value).unwrap_or_else(|| "/* … */".to_string());
        if field.children {
            children = Some(code);
        } else {
            props.push(format!("    {} = {},", field.name, code));
        }
    }
    let mut out = if props.is_empty() {
        format!("{component}()")
    } else {
        format!("{component}(\n{}\n)", props.join("\n"))
    };
    if let Some(child) = children {
        out.push_str(&format!(" {{\n    {child}\n}}"));
    }
    out
}

/// The canvas width: the docs column, or a phone's.
#[derive(Clone, Copy, PartialEq)]
enum Width {
    Fill,
    Phone,
}

/// The playground of the component whose catalog entry records `symbol`:
/// the canvas with its toolbar, the props panel, and the Usage snippet.
/// Renders nothing when the symbol has no preview (check with
/// [`component_preview`]).
#[component]
pub fn Playground(#[prop(static)] symbol: String, #[prop(static)] name: String) -> Element {
    let Some(make) = component_preview(&symbol) else {
        return ui! { view() };
    };
    let preview = Rc::new(make());
    let initial = preview.initial_values();
    let values = signal(initial.clone());
    let generation = signal(0u32);
    let width = signal(Width::Fill);
    let has_rows = preview.fields.iter().any(has_row);

    let width_options = vec![SegmentOption::new("fill", "Fill"), SegmentOption::new("phone", "Phone")];
    let on_width: Rc<dyn Fn(String)> =
        Rc::new(move |id: String| width.set(if id == "phone" { Width::Phone } else { Width::Fill }));
    let width_id =
        runtime_core::memo(move || if width.get() == Width::Phone { "phone".to_string() } else { "fill".to_string() });
    let on_reset: Rc<dyn Fn()> = Rc::new(move || {
        values.set(initial.clone());
        generation.update(|g| g + 1);
    });

    let canvas_preview = preview.clone();
    let panel_preview = preview.clone();
    let usage_preview = preview.clone();
    let copy_preview = preview.clone();
    let copy_name = name.clone();
    let copy_text: Rc<dyn Fn() -> String> =
        Rc::new(move || usage_code(&copy_name, &copy_preview, &runtime_core::untrack(|| values.get())));

    ui! {
        view(style = PlaygroundCard()) {
            view(style = PlaygroundBody()) {
                view(style = CanvasColumn()) {
                    view(style = CanvasToolbar()) {
                        SegmentedControl(value = width_id, on_change = on_width, options = width_options, size = ControlSize::Sm)
                        view(style = ToolbarSpacer())
                        Button(
                            label = "Reset props",
                            on_click = on_reset,
                            variant = variant::Ghost,
                            size = size::Sm,
                            leading_icon = Some(icons_lucide::ROTATE_CCW),
                        )
                    }
                    view(style = Canvas()) {
                        // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the previewed component, rebuilt from the edited values and canvas width
                        {switch(
                            move || (width.get(), values.get()),
                            move |(w, vals): &(Width, Vec<ControlValue>)| {
                                let component = canvas_preview.render(vals);
                                match w {
                                    Width::Fill => ui! { view(style = FillFrame()) { component } },
                                    Width::Phone => ui! { view(style = PhoneFrame()) { component } },
                                }
                            },
                        )}
                    }
                }
                if has_rows {
                    view(style = PropsPanel()) {
                        text(style = PanelCaption()) { "Props" }
                        // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the props panel, rebuilt from its initial values on Reset
                        {switch(move || generation.get(), move |_| props_rows(&panel_preview, values.write_only()))}
                    }
                }
            }
            view(style = UsageBar()) {
                text(style = UsageTitle()) { "Usage" }
                text(style = UsageHint()) { "Props at their default are left out" }
                view(style = ToolbarSpacer())
                CopyButton(text = copy_text)
            }
            // idealyst-lint-disable-next-line prefer-ui-control-flow -- keyed rebuild of one shape: the Usage snippet for the current values
            {switch(move || values.get(), move |vals: &Vec<ControlValue>| {
                let src = usage_code(&name, &usage_preview, vals);
                ui! { CodePanel(src = src) }
            })}
        }
    }
}

/// One row per editable field, each reporting into its slot of `values`.
fn props_rows(preview: &Preview, values: WriteSignal<Vec<ControlValue>>) -> Element {
    let rows: Vec<(usize, FieldSpec)> =
        preview.fields.iter().cloned().enumerate().filter(|(_, f)| has_row(f)).collect();
    ui! {
        Stack(gap = StackGap::Md) {
            for (index, field) in rows {
                PropEditorRow(field = Some(field), on_change = report_at(values, index))
            }
        }
    }
}

/// A reporter that writes into slot `index` of the playground's values.
fn report_at(values: WriteSignal<Vec<ControlValue>>, index: usize) -> OnValue {
    Rc::new(move |value| {
        values.update(|current| {
            let mut next = current.clone();
            if let Some(slot) = next.get_mut(index) {
                *slot = value;
            }
            next
        })
    })
}

/// A labelled prop: its name, then on the right its type — or, for an
/// optional prop, the checkbox that sets it ("Some", or "Attach handler"
/// for a callback) — and the editor below.
#[component]
fn PropEditorRow(#[prop(static)] field: Option<FieldSpec>, #[prop(static, default = ignore())] on_change: OnValue) -> Element {
    let Some(field) = field else { return ui! { view() } };
    let name = field.name.to_string();
    match field.control.clone() {
        Control::Optional { inner, example } => {
            let ControlValue::Optional(start) = field.initial.clone() else { return ui! { view() } };
            let handler = *inner == Control::Handler;
            let enabled = signal(start.is_some());
            let last = Rc::new(RefCell::new(
                start.map(|v| *v).or(example.map(|v| *v)).unwrap_or_else(|| empty_value(&inner)),
            ));
            let inner_initial = last.borrow().clone();
            let on_toggle: Rc<dyn Fn(bool)> = {
                let last = last.clone();
                let on_change = on_change.clone();
                Rc::new(move |on: bool| {
                    enabled.set(on);
                    on_change(ControlValue::Optional(on.then(|| Box::new(last.borrow().clone()))));
                })
            };
            let on_inner: OnValue = Rc::new(move |v: ControlValue| {
                *last.borrow_mut() = v.clone();
                // A handler, not an effect: this read subscribes nothing.
                if enabled.get() {
                    on_change(ControlValue::Optional(Some(Box::new(v))));
                }
            });
            let label = if handler { "Attach handler" } else { "Some" }.to_string();
            let inner_control = *inner;
            ui! {
                view(style = PropRow()) {
                    view(style = PropRowHead()) {
                        text(style = PropName()) { name }
                        Checkbox(label = Some(label), value = enabled, on_change = on_toggle, size = ControlSize::Sm)
                    }
                    if !handler {
                        ControlEditor(control = inner_control, initial = inner_initial, on_change = on_inner)
                    }
                }
            }
        }
        control => {
            let ty = if field.children { "children".to_string() } else { short_type(field.type_name) };
            let initial = field.initial;
            ui! {
                view(style = PropRow()) {
                    view(style = PropRowHead()) {
                        text(style = PropName()) { name }
                        text(style = PropType()) { ty }
                    }
                    ControlEditor(control = control, initial = initial, on_change = on_change)
                }
            }
        }
    }
}

/// An editor for any [`Control`], dispatching on its kind. A nested
/// `Optional` (inside a `Group`) is a row of its own, unlabelled.
#[component]
fn ControlEditor(
    #[prop(static, default = Control::Text)] control: Control,
    #[prop(static, default = ControlValue::Text(String::new()))] initial: ControlValue,
    #[prop(static, default = ignore())] on_change: OnValue,
) -> Element {
    ui! {
        match control {
            Control::Text => {
                TextEditor(initial = initial, on_change = on_change)
            }
            Control::Bool => {
                BoolEditor(initial = initial, on_change = on_change)
            }
            Control::Number { min, max, integer, .. } => {
                NumberEditor(initial = initial, min = min, max = max, integer = integer, on_change = on_change)
            }
            Control::Choice(keys) => {
                ChoiceEditor(keys = keys, initial = initial, on_change = on_change)
            }
            Control::Optional { inner, example } => {
                PropEditorRow(
                    field = Some(FieldSpec {
                        name: "",
                        type_name: "",
                        control: Control::Optional { inner, example },
                        initial: initial.clone(),
                        default: initial,
                        children: false,
                    }),
                    on_change = on_change,
                )
            }
            Control::Group(parts) => {
                GroupEditor(parts = parts, initial = initial, on_change = on_change)
            }
            Control::Handler => {
                view()
            }
        }
    }
}

#[component]
fn TextEditor(
    #[prop(static, default = ControlValue::Text(String::new()))] initial: ControlValue,
    #[prop(static, default = ignore())] on_change: OnValue,
) -> Element {
    let ControlValue::Text(start) = initial else { return ui! { view() } };
    let value = signal(start);
    let changed: Rc<dyn Fn(String)> = Rc::new(move |s: String| {
        value.set(s.clone());
        on_change(ControlValue::Text(s));
    });
    ui! { Field(value = value, on_change = changed, on_focus_change = Some(editing_tracker())) }
}

#[component]
fn BoolEditor(
    #[prop(static, default = ControlValue::Bool(false))] initial: ControlValue,
    #[prop(static, default = ignore())] on_change: OnValue,
) -> Element {
    let ControlValue::Bool(start) = initial else { return ui! { view() } };
    let value = signal(start);
    let changed: Rc<dyn Fn(bool)> = Rc::new(move |b: bool| {
        value.set(b);
        on_change(ControlValue::Bool(b));
    });
    ui! { Switch(value = value, on_change = changed) }
}

/// A number typed into a field. Input that doesn't parse is left in the
/// field and not reported; a parsed value is rounded when `integer` and
/// clamped to the bounds before it reaches the preview.
#[component]
fn NumberEditor(
    #[prop(static, default = ControlValue::Number(0.0))] initial: ControlValue,
    #[prop(static)] min: Option<f64>,
    #[prop(static)] max: Option<f64>,
    #[prop(static)] integer: bool,
    #[prop(static, default = ignore())] on_change: OnValue,
) -> Element {
    let ControlValue::Number(start) = initial else { return ui! { view() } };
    let value = signal(number_text(start));
    let changed: Rc<dyn Fn(String)> = Rc::new(move |s: String| {
        value.set(s.clone());
        let Ok(mut n) = s.trim().parse::<f64>() else { return };
        if integer {
            n = n.round();
        }
        if let Some(lo) = min {
            n = n.max(lo);
        }
        if let Some(hi) = max {
            n = n.min(hi);
        }
        on_change(ControlValue::Number(n));
    });
    ui! { Field(value = value, on_change = changed, on_focus_change = Some(editing_tracker())) }
}

/// A short choice as a segmented control, a longer one as a select.
#[component]
fn ChoiceEditor(
    #[prop(static)] keys: Vec<String>,
    #[prop(static, default = ControlValue::Choice(String::new()))] initial: ControlValue,
    #[prop(static, default = ignore())] on_change: OnValue,
) -> Element {
    let ControlValue::Choice(start) = initial else { return ui! { view() } };
    let segmented = keys.len() <= SEGMENTED_MAX_OPTIONS
        && keys.iter().map(|k| k.chars().count()).sum::<usize>() <= SEGMENTED_MAX_CHARS;
    let value = signal(start);
    let changed: Rc<dyn Fn(String)> = Rc::new(move |key: String| {
        value.set(key.clone());
        on_change(ControlValue::Choice(key));
    });
    let segments: Vec<SegmentOption> = keys.iter().map(|k| SegmentOption::new(k.clone(), title_case(k))).collect();
    let options: Vec<SelectOption> = keys.iter().map(|k| SelectOption::new(k.clone(), k.clone())).collect();
    // Each branch lowers to its own `move` closure: one handle apiece.
    let select_changed = changed.clone();
    ui! {
        if segmented {
            SegmentedControl(value = value, on_change = changed, options = segments, size = ControlSize::Sm)
        } else {
            Select(value = value, on_change = select_changed, options = options)
        }
    }
}

/// `"filled"` → `"Filled"`, `"two_tone"` → `"Two tone"`: a key as a
/// segment label.
fn title_case(key: &str) -> String {
    let spaced = key.replace(['_', '-'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// One labelled editor per part; reports the whole group on any change.
#[component]
fn GroupEditor(
    #[prop(static)] parts: Vec<(String, Control)>,
    #[prop(static, default = ControlValue::Group(Vec::new()))] initial: ControlValue,
    #[prop(static, default = ignore())] on_change: OnValue,
) -> Element {
    let ControlValue::Group(mut start) = initial else { return ui! { view() } };
    start.resize_with(parts.len(), || ControlValue::Text(String::new()));
    let current = Rc::new(RefCell::new(start.clone()));
    let rows: Vec<(usize, String, Control, ControlValue)> = parts
        .into_iter()
        .zip(start)
        .enumerate()
        .map(|(i, ((label, control), value))| (i, label, control, value))
        .collect();
    ui! {
        Stack(gap = StackGap::Sm) {
            for (index, label, control, value) in rows {
                Stack(gap = StackGap::Xs) {
                    Typography(content = label, kind = typography_kind::Caption)
                    ControlEditor(
                        control = control,
                        initial = value,
                        on_change = report_part(current.clone(), index, on_change.clone()),
                    )
                }
            }
        }
    }
}

fn report_part(current: Rc<RefCell<Vec<ControlValue>>>, index: usize, on_change: OnValue) -> OnValue {
    Rc::new(move |value| {
        let group = {
            let mut parts = current.borrow_mut();
            parts[index] = value;
            parts.clone()
        };
        on_change(ControlValue::Group(group));
    })
}

// =============================================================================
// Copy, and the "typing in a field" flag
// =============================================================================

thread_local! {
    /// How many of the docs' text fields hold focus. App-level key
    /// listeners hear keys typed into a field too (`docs/keyboard-input.md`),
    /// so the `/` search shortcut checks this before taking the key.
    static EDITING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Whether a docs text field has focus.
pub fn editing() -> bool {
    EDITING.with(|e| e.get() > 0)
}

/// A field's `on_focus_change` that keeps [`editing`] current.
pub fn editing_tracker() -> Rc<dyn Fn(bool)> {
    Rc::new(|focused| EDITING.with(|e| e.set(if focused { e.get() + 1 } else { e.get().saturating_sub(1) })))
}

/// How long "Copied" shows before the button reads "Copy" again.
const COPIED_MS: i32 = 1500;

/// A small "Copy" button that puts `text()` on the clipboard.
#[component]
pub fn CopyButton(
    #[prop(static, default = Rc::new(String::new) as Rc<dyn Fn() -> String>)] text: Rc<dyn Fn() -> String>,
) -> Element {
    let copied = signal(false);
    let on_click: Rc<dyn Fn()> = Rc::new(move || {
        let value = text();
        runtime_core::spawn_then(async move { clipboard::set_text(&value).await }, move |result| {
            if result.is_ok() {
                copied.set(true);
                runtime_core::after_ms_scoped(COPIED_MS, move || copied.set(false));
            }
        });
    });
    let label = runtime_core::memo(move || if copied.get() { "Copied".to_string() } else { "Copy".to_string() });
    ui! {
        Button(
            label = label,
            on_click = on_click,
            variant = variant::Ghost,
            size = size::Sm,
            leading_icon = Some(icons_lucide::COPY),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CatalogModel, Entry, Kind};

    fn component(model: &CatalogModel, name: &str) -> Entry {
        model
            .of_kind(Kind::Component)
            .into_iter()
            .find(|e| e.name == name && e.module_path.starts_with("idea_ui::"))
            .unwrap_or_else(|| panic!("idea-ui's {name} is in the catalog"))
            .clone()
    }

    /// The whole chain on a real component: the catalog (extracted by
    /// build.rs) records Button's symbol, the generated `extern` block
    /// links it from idea-ui, and the preview it returns carries a control
    /// per editable prop and renders.
    #[test]
    fn idea_ui_button_preview_links_through_its_catalog_symbol() {
        let model = CatalogModel::build();
        let button = component(&model, "Button");
        assert!(button.preview.starts_with("__idealyst_preview__idea_ui__Button__"), "{}", button.preview);
        let make = component_preview(&button.preview).expect("Button's preview is linked");
        runtime_core::__with_fresh_world(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let preview = make();
            let names: Vec<_> = preview.fields.iter().map(|f| f.name).collect();
            for expected in ["label", "tone", "variant", "size", "shape", "disabled", "loading", "block"] {
                assert!(names.contains(&expected), "{expected} missing from {names:?}");
            }
            for absent in ["bind_to", "styles"] {
                assert!(!names.contains(&absent), "{absent} has no control: {names:?}");
            }
            let on_click = preview.fields.iter().find(|f| f.name == "on_click").unwrap();
            assert_eq!(on_click.control, Control::Handler, "a required callback: no row, no snippet entry");
            let label = preview.fields.iter().find(|f| f.name == "label").unwrap();
            assert_eq!(short_type(label.type_name), "String", "{}", label.type_name);
            let tone = preview.fields.iter().find(|f| f.name == "tone").unwrap();
            assert!(
                matches!(&tone.control, Control::Choice(keys) if keys.iter().any(|k| k == "danger")),
                "ToneRef's #[prop_control] impl: {:?}",
                tone.control
            );
            assert_eq!(tone.initial, ControlValue::Choice("primary".into()));
            let _ = preview.render(&preview.initial_values());
        });
    }

    /// A portal escapes the preview box and an open one covers the docs
    /// page, so overlay components keep their symbol in the catalog but get
    /// no live preview.
    #[test]
    fn overlay_components_have_no_live_preview() {
        let model = CatalogModel::build();
        let modal = component(&model, "Modal");
        assert!(!modal.preview.is_empty(), "the catalog still records the symbol");
        assert!(component_preview(&modal.preview).is_none());
    }

    /// idea-ui's `#[prop(example)]` values reach the preview: Avatar starts
    /// with initials (not an empty disc), and its optional image starts at
    /// the sample image when switched on.
    #[test]
    fn idea_ui_examples_seed_the_preview() {
        let model = CatalogModel::build();
        let avatar = component(&model, "Avatar");
        let make = component_preview(&avatar.preview).expect("Avatar's preview is linked");
        runtime_core::__with_fresh_world(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let preview = make();
            let initials = preview.fields.iter().find(|f| f.name == "initials").unwrap();
            assert_eq!(initials.initial, ControlValue::Text("AB".into()));
            let src = preview.fields.iter().find(|f| f.name == "src").unwrap();
            assert_eq!(src.initial, ControlValue::Optional(None), "initials show until the image is switched on");
            assert!(
                matches!(&src.control, Control::Optional { example: Some(e), .. }
                    if **e == ControlValue::Text(runtime_core::previews::SAMPLE_IMAGE_URL.into())),
                "{:?}",
                src.control
            );
        });
    }

    /// The registered icons back an icon prop: Icon's `data` starts at the
    /// first one (its example reads the registry when the preview is made).
    #[test]
    fn icon_preview_starts_at_the_first_registered_icon() {
        let model = CatalogModel::build();
        let icon = component(&model, "Icon");
        let make = component_preview(&icon.preview).expect("Icon's preview is linked");
        runtime_core::__with_fresh_world(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            register_icon_choices();
            let preview = make();
            let data = preview.fields.iter().find(|f| f.name == "data").unwrap();
            assert_eq!(data.initial, ControlValue::Choice("star".into()));
            assert!(matches!(&data.control, Control::Choice(keys) if keys.len() > 10));
            runtime_core::previews::set_icon_choices(Vec::new());
        });
    }

    /// The Usage snippet is the `ui!` call the controls make: props left at
    /// their default are omitted, an example counts as set, a choice is
    /// its Rust spelling, an attached optional callback shows a handler.
    #[test]
    fn usage_snippet_spells_the_changed_props() {
        let model = CatalogModel::build();
        let alert = component(&model, "Alert");
        let make = component_preview(&alert.preview).expect("Alert's preview is linked");
        runtime_core::__with_fresh_world(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let preview = make();
            let mut values = preview.initial_values();
            let at = |name: &str| preview.fields.iter().position(|f| f.name == name).unwrap();
            values[at("tone")] = ControlValue::Choice("success".into());
            let code = usage_code("Alert", &preview, &values);
            assert!(code.starts_with("Alert(\n"), "{code}");
            assert!(code.contains("    title = \"Changes saved\","), "an example is not the default: {code}");
            assert!(code.contains("    body = Some(String::from(\"Your profile is up to date.\")),"), "{code}");
            assert!(code.contains("    tone = tone::Success,"), "{code}");
            assert!(!code.contains("variant ="), "left at its default: {code}");

            // `close` is "attach a handler" (idea-ui's `AlertClose` impl);
            // `action` is an optional element slot that starts at a sample.
            let close = &preview.fields[at("close")];
            assert!(matches!(&close.control, Control::Optional { inner, .. } if **inner == Control::Handler));
            let action = &preview.fields[at("action")];
            assert!(
                matches!(&action.control, Control::Optional { inner, example: Some(e) }
                    if **inner == Control::Text && **e == ControlValue::Text("Action".into())),
                "{:?}",
                action.control
            );
            values[at("close")] = ControlValue::Optional(Some(Box::new(ControlValue::Handler)));
            let code = usage_code("Alert", &preview, &values);
            assert!(code.contains("    close = AlertClose::Button(Rc::new(|| {})),"), "{code}");
            values[at("action")] = ControlValue::Optional(Some(Box::new(ControlValue::Text("Undo".into()))));
            let code = usage_code("Alert", &preview, &values);
            assert!(code.contains("    action = Some(ui! { text { \"Undo\" } }),"), "an element slot spells its text: {code}");
            let _ = preview.render(&values);

            let all_default: Vec<_> = preview.fields.iter().map(|f| f.default.clone()).collect();
            assert_eq!(usage_code("Alert", &preview, &all_default), "Alert()");
        });
    }

    #[test]
    fn type_labels_drop_paths_and_the_reactive_wrapper() {
        assert_eq!(short_type(":: runtime_core :: Reactive < String >"), "String");
        assert_eq!(short_type("::runtime_core::Reactive<::idea_theme::ToneRef>"), "ToneRef");
        assert_eq!(short_type("Option < Rc < dyn Fn() > >"), "Option<Rc<dyn Fn()>>");
        assert_eq!(bare_type("::runtime_core::Reactive<Option<String>>"), "Reactive<Option<String>>");
    }

    /// Recipes render through the same symbol mechanism.
    #[test]
    fn idea_ui_recipe_renders_through_its_catalog_symbol() {
        let model = CatalogModel::build();
        let button = component(&model, "Button");
        let recipe = button
            .recipes
            .iter()
            .find(|r| r.primary && !r.preview.is_empty())
            .expect("Button has a zero-argument primary recipe");
        let render = recipe_renderer(&recipe.preview).expect("its render fn is linked");
        runtime_core::__with_fresh_world(|| {
            idea_ui::install_idea_theme(idea_ui::light_theme());
            let _ = render();
        });
    }
}

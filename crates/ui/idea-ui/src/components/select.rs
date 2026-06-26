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

use std::time::Duration;

use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use runtime_core::primitives::overlay::{overlay, BackdropMode};
use runtime_core::primitives::portal::{AnchorTarget, ElementAlign, ElementSide, ViewportPlacement};
use runtime_core::{
    component, effect, icon, signal, ui, Color, Element, FillRule, IconData, IdealystSchema,
    IntoElement, Length, Position, PressableHandle, Reactive, Ref, Signal, StyleApplication,
    StyleRules, StyleSheet, Tokenized, VariantEnum, VariantSet, ViewHandle,
};

use idea_theme::theme::IdeaThemeRef;

use crate::stylesheets::{SelectMenu, SelectOption as SelectOptionStyle, SelectTrigger};

/// Full-bleed, fully-transparent backdrop for the open menu: it catches the
/// outside click (and a re-press of the trigger) that dismisses the dropdown,
/// without dimming the page. Mirrors `Popover`'s backdrop. `backdrop_style`
/// *replaces* the backdrop's styling, so this must set the full-viewport inset
/// itself — a background-only sheet would collapse to zero and catch nothing.
fn transparent_backdrop_sheet() -> std::rc::Rc<StyleSheet> {
    std::rc::Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(0.0))),
        left: Some(Tokenized::Literal(Length::Px(0.0))),
        right: Some(Tokenized::Literal(Length::Px(0.0))),
        bottom: Some(Tokenized::Literal(Length::Px(0.0))),
        background: Some(Tokenized::Literal(Color("transparent".into()))),
        ..Default::default()
    }))
}

pub use crate::stylesheets::SelectTriggerSize as SelectSize;

/// Default trigger affordance: a chevron that rotates when the menu opens.
/// Authors override it via [`SelectProps::icon`].
const CHEVRON_DOWN: IconData = IconData {
    view_box: (24, 24),
    paths: &["M6 9l6 6 6-6"],
    fill_rule: FillRule::NonZero,
    filled: false,
};

/// How fast the trigger chevron flips between its closed (0°) and open
/// (180°) positions. Short + eased-out so it feels snappy, not floaty.
const CHEVRON_SPIN_MS: u64 = 130;

/// One selectable row in a [`Select`]. `id` is the value committed to
/// the bound signal when chosen; `label` is what the user sees.
#[derive(Clone, IdealystSchema)]
pub struct SelectOption {
    /// Stable value committed to the `Select`'s `value` signal when this
    /// row is chosen. Compared against the current value to mark the
    /// selected row.
    pub id: String,
    /// Row label. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
}

impl SelectOption {
    pub fn new(id: impl Into<String>, label: impl Into<Reactive<String>>) -> Self {
        Self { id: id.into(), label: label.into() }
    }
}

#[derive(IdealystSchema)]
pub struct SelectProps {
    /// Controlled selected value — the `id` of the chosen
    /// [`SelectOption`]. The host owns the signal; selecting a row sets
    /// it via `on_change`.
    pub value: Signal<String>,
    /// Fires with the chosen option's `id` when the user picks a row.
    pub on_change: Rc<dyn Fn(String)>,
    /// The rows to offer in the dropdown menu.
    pub options: Vec<SelectOption>,
    /// Trigger height. Default Md.
    pub size: SelectSize,
    /// Text shown on the trigger when no option matches `value`.
    pub placeholder: Option<String>,
    /// Trailing affordance icon, rotated 180° while the menu is open.
    /// Defaults to a chevron when `None`; pass `Some(icons_lucide::…)` to
    /// customize it.
    pub icon: Option<IconData>,
}

impl Default for SelectProps {
    fn default() -> Self {
        Self {
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            options: Vec::new(),
            size: SelectSize::default(),
            placeholder: None,
            icon: None,
        }
    }
}

/// Controlled dropdown: a pressable trigger showing the selected
/// option's label (or `placeholder`), opening an anchored menu of
/// [`SelectOption`] rows. Choosing a row fires `on_change` and closes
/// the menu.
#[component]
pub fn Select(props: SelectProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let size = props.size;
    let placeholder = props.placeholder.clone();
    let options = Rc::new(props.options);
    let icon_data = props.icon.unwrap_or(CHEVRON_DOWN);

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

    // Trailing chevron that rotates 180° while the menu is open. Bound to a
    // view (RotateZ needs a ViewHandle), tinted muted, springing on toggle.
    let chevron_ref: Ref<ViewHandle> = Ref::new();
    let rot = AnimatedValue::new(0.0);
    rot.bind(chevron_ref, AnimProp::RotateZ);
    effect!({
        let target = if open.get() { 180.0 } else { 0.0 };
        rot.animate(TweenTo::new(target, Duration::from_millis(CHEVRON_SPIN_MS)).ease_out());
    });
    let chevron_glyph = icon(icon_data)
        .size(16.0)
        .color(|| Tokenized::token("color-text-muted", Color("#6b7280".into())).resolve())
        .into_element();
    let chevron = runtime_core::view(vec![chevron_glyph]).bind(chevron_ref).into_element();

    let trigger_style = move || {
        let _ = idea_theme::active_theme_untracked()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        StyleApplication::new(SelectTrigger::sheet())
            .with("size", size.as_variant_str().to_string())
            // Highlight (focus-ring border) while the menu is open.
            .with("open", if open.get() { "on" } else { "off" }.to_string())
    };
    // Open on press; the menu's dismiss backdrop handles close — clicking
    // away OR clicking the trigger again lands on the (invisible) backdrop
    // and dismisses, so an open Select closes on the next click anywhere.
    let on_open: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let trigger = runtime_core::pressable(vec![label_child, chevron], move || (on_open)())
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
        || ui! { view {} }.into_element(),
    );

    ui! {
        view {
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
            let _ = idea_theme::active_theme_untracked()
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

    // Click-away + re-press-to-close: a FULLSCREEN, transparent dismiss
    // backdrop. An anchored overlay's own backdrop only fills its small
    // anchored box (the portal is positioned at the trigger, and there's no
    // `position: fixed` to escape it), so it can't catch a click elsewhere.
    // A fullscreen `overlay()` portal IS viewport-sized, so its transparent
    // Dismiss backdrop catches every outside click — including on the trigger,
    // which it covers while open — and fires `on_dismiss`.
    let catcher_close = menu_close.clone();
    let catcher = overlay(Vec::new())
        // FullScreen so the portal (and its inset-0 backdrop) is viewport-sized
        // — the default Center placement sizes the portal to its (empty)
        // content, collapsing the backdrop to 0×0.
        .placement(ViewportPlacement::FullScreen)
        .backdrop(BackdropMode::Dismiss)
        .backdrop_style(StyleApplication::new(transparent_backdrop_sheet()))
        .on_dismiss(move || (catcher_close)())
        .into_element();

    // The anchored menu sits ABOVE the catcher (same layer, rendered after).
    // Its own backdrop is None — the catcher owns dismissal; Escape still
    // closes via this `on_dismiss`.
    let anchored = runtime_core::anchored_overlay(
        AnchorTarget::from(trigger_ref),
        vec![ui! { view(style = menu_style) { rows } }],
    )
    .side(ElementSide::Below)
    .align(ElementAlign::Start)
    .offset(4.0)
    .backdrop(BackdropMode::None)
    .trap_focus(false)
    .on_dismiss(move || (menu_close)())
    .into_element();

    runtime_core::view(vec![catcher, anchored]).into_element()
}

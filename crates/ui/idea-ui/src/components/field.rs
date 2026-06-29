//! `Field` — labeled text input with optional helper/error text.
//!
//! ```ignore
//! ui! {
//!     Field(
//!         label = "Email",
//!         value = email,
//!         on_change = move |v: String| email.set(v),
//!         placeholder = "you@example.com",
//!         help = "We'll never share your email.",
//!     )
//! }
//! ```
//!
//! `tone` (optional) drives the input border + help-text color. When
//! `error` is `Some(...)`, `tone::Danger` is applied automatically if
//! no explicit tone is given. `size` is a closed enum (`FieldSize`).
//!
//! Styles are resolved from a programmatic sheet (size × tone axes +
//! focused/disabled states), installed lazily. The input style is a
//! STATIC `StyleApplication` (applied at build time, theme-swapped in
//! bulk by the cohort — no per-node Effect, no first-paint flicker)
//! WHENEVER the tone is fixed at build. When the tone is *derived from a
//! live `error` signal*, the input style is instead attached as a
//! reactive closure so the border color re-resolves on each validation
//! change (see the `INVARIANT (D9)` note in [`Field`]).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    component, pressable, recipe, ui, AlignItems, Color, Cursor, Easing, Element, FlexDirection,
    IconData, IdealystSchema, IntoElement, JustifyContent, Length, Reactive, Signal,
    StyleApplication, StyleRules, StyleSheet, Tokenized, Transition, VariantEnum, VariantSet,
};

use crate::components::icon::Icon;

/// Horizontal inset on the BARE (adorned) input. Just enough that the glyph's
/// left/right bearing doesn't clip against the input edge (macOS draws the cell
/// text flush). The row gap is reduced by this so the icon↔text spacing stays
/// visually equal to the edge↔icon padding.
const FIELD_BARE_H_PAD: f32 = 2.0;

use idea_theme::active_theme;
use idea_theme::extensible::{tone as tones, RefBuiltins, ResolutionCtx, ToneRef};
use idea_theme::theme::{IdeaTheme, IdeaThemeRef};

use crate::stylesheets::{FieldGroup, FieldLabel};
pub use crate::stylesheets::{FieldAppearance, FieldSize};

/// A leading/trailing adornment inside a [`Field`]'s box — an icon or a custom
/// element rendered beside the input (e.g. a search glyph, a unit suffix, a
/// clear button). Three shapes:
///
/// - [`Adornment::None`] — nothing (the default).
/// - [`Adornment::Icon`] — a vector icon, rendered in the field's muted text
///   color at a size derived from the field `size` (it "inherits" the field's
///   styling so you just pass the icon).
/// - [`Adornment::Element`] — any element, rendered as-is, for full control
///   (a button, badge, spinner, …).
///
/// Adornments compose into a flex row alongside the input, so any width works.
#[derive(Clone)]
pub enum Adornment {
    /// No adornment.
    None,
    /// An arbitrary element, built on demand. `Element` isn't `Clone`, so the
    /// variant holds a builder closure (the same shape as the modal's content)
    /// — use [`Adornment::element`] to construct it from a `move || ui! { … }`.
    Element(Rc<dyn Fn() -> Element>),
    /// A vector icon, auto-sized + muted to match the field.
    Icon(IconData),
    /// A TAPPABLE icon — same auto-sized, muted glyph as [`Adornment::Icon`]
    /// but wrapped in a `pressable` with a press handler and a subtle
    /// hover/press dim. Use this for a clear button, a password-visibility
    /// toggle, etc.: unlike dropping an `IconButton` into [`Adornment::Element`]
    /// (which stacks the button's own square padding on top of the field's),
    /// this stays icon-sized so it never inflates the field box.
    Button(IconData, Rc<dyn Fn()>),
}

impl Adornment {
    /// Build an [`Adornment::Element`] from a closure: `Adornment::element(move
    /// || ui! { Button(…) })`.
    pub fn element(builder: impl Fn() -> Element + 'static) -> Self {
        Adornment::Element(Rc::new(builder))
    }

    /// Build an icon-sized [`Adornment::Button`]: `Adornment::button(icon, move
    /// || visible.set(!visible.get()))`.
    pub fn button(icon: IconData, on_press: impl Fn() + 'static) -> Self {
        Adornment::Button(icon, Rc::new(on_press))
    }
}

impl Default for Adornment {
    fn default() -> Self {
        Adornment::None
    }
}

/// Icon point size for an adornment at a given field size.
fn adornment_icon_px(size: FieldSize) -> f32 {
    match size.as_variant_str() {
        "sm" => 14.0,
        "lg" => 18.0,
        _ => 16.0,
    }
}

/// The muted glyph color shared by `Icon`/`Button` adornments.
fn adornment_icon_color() -> Color {
    Tokenized::token("color-text-muted", Color("#8a8270".into())).resolve()
}

/// Resolve an adornment to a renderable element (or `None`). `Icon`/`Button`
/// are sized from the field `size` and painted in the theme's muted text color.
fn render_adornment(adornment: &Adornment, size: FieldSize) -> Option<Element> {
    match adornment {
        Adornment::None => None,
        Adornment::Element(build) => Some(build()),
        Adornment::Icon(data) => {
            let px = adornment_icon_px(size);
            let muted = adornment_icon_color();
            Some(ui! { Icon(data = data.clone(), size = px, color = Some(muted)) })
        }
        Adornment::Button(data, on_press) => {
            let px = adornment_icon_px(size);
            let muted = adornment_icon_color();
            let glyph = ui! { Icon(data = data.clone(), size = px, color = Some(muted)) };
            let on_press = on_press.clone();
            // An icon-sized pressable — no button chrome/padding, so it never
            // inflates the field box (the whole point of `Button` vs an
            // `IconButton` in an `Element` adornment).
            Some(
                pressable(vec![glyph], move || on_press())
                    .with_style(StyleApplication::new(adornment_button_sheet()))
                    .into_element(),
            )
        }
    }
}

/// Lazy stylesheet for [`Adornment::Button`]: a pointer cursor, centered glyph,
/// and a subtle hover/press dim. No padding — it stays icon-sized.
fn adornment_button_sheet() -> Rc<StyleSheet> {
    thread_local! {
        static SHEET: Rc<StyleSheet> = Rc::new(
            StyleSheet::new(|_| StyleRules {
                cursor: Some(Cursor::Pointer),
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                ..Default::default()
            })
            .variant("__state_hovered", "on", |_| StyleRules {
                opacity: Some(Tokenized::Literal(0.65)),
                ..Default::default()
            })
            .variant("__state_pressed", "on", |_| StyleRules {
                opacity: Some(Tokenized::Literal(0.4)),
                ..Default::default()
            }),
        );
    }
    SHEET.with(|s| s.clone())
}

// Reactive-by-default: `#[props]` rewrites each scalar-DATA field `T` →
// `Reactive<T>` so a `ui!` call site can pass a `Signal`/`rx!` and have it
// carry through live (a bare value stays a zero-overhead `Static` snapshot).
// EVERY data prop here is reactive. The exclusions are a different category,
// NOT "static": `on_change` (a handler — invoked on events, never rendered to
// a sink), the controlled `value` `Signal` (already a reactive *source*), the
// already-`Reactive` text props, and the `Adornment` slots — which are
// ELEMENT-BUILDERS (`Rc<dyn Fn() -> Element>`), the *children* category, whose
// reactivity is structural/internal (reactive content inside
// `Adornment::element`, e.g. the password recipe), not data-reactive.
// `Reactive<Adornment>` can't even route: `switch` needs `PartialEq` and an
// element-builder isn't comparable — so a bare `Adornment` is the right type.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct FieldProps {
    /// Optional field label. `Reactive<Option<String>>` — static
    /// (`None`/`Some`) or live (signal/`rx!`).
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub label: Reactive<Option<String>>,
    /// The input's current text. The host owns this signal; the Field
    /// reads it to populate the input and writes via `on_change`.
    pub value: Signal<String>,
    /// Fires with the new text on each edit.
    pub on_change: Rc<dyn Fn(String)>,
    /// Placeholder shown when the input is empty.
    pub placeholder: Option<String>,
    /// Helper text below the input. `Reactive<Option<String>>` — static
    /// or live (signal/`rx!`).
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub help: Reactive<Option<String>>,
    /// Error text below the input; takes precedence over `help` when
    /// present. `Reactive<Option<String>>` — typically the live one
    /// (validation result).
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub error: Reactive<Option<String>>,
    /// Optional tone overlay (border + help-text color). Write
    /// `Some(tone::Warning.into())`; orphan rule blocks a bare-tone
    /// `Into<Option<ToneRef>>`.
    pub tone: Option<ToneRef>,
    /// Input density (`Sm`/`Md`/`Lg`) — drives padding + font size.
    /// Default `Md`.
    pub size: FieldSize,
    /// Visual shell: `Outline` (bordered, default), `Contained` (filled),
    /// or `Bare` (no chrome). All three keep a focus ring.
    pub variant: FieldAppearance,
    /// Mask the entered text (password entry). Forwarded to the underlying
    /// `text_input` primitive's `secure` flag. `Reactive<bool>` — a static
    /// `bool` (the common case) or a live `Signal`/`rx!` so the mask can
    /// toggle at runtime (password show/hide) without rebuilding the input.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub secure: Reactive<bool>,
    /// Leading adornment (icon/element) rendered before the input, inside the
    /// field box. Default [`Adornment::None`]. See [`Adornment`]. An
    /// element-builder (children category) — kept bare; reactivity is internal
    /// (`Adornment::element(move || …reactive content…)`), not `Reactive<_>`.
    #[prop(static)]
    pub leading: Adornment,
    /// Trailing adornment rendered after the input, inside the field box.
    /// Default [`Adornment::None`]. See [`Adornment`]. Element-builder
    /// (children category) — kept bare; reactivity is internal, not
    /// `Reactive<_>` (an element-builder isn't `PartialEq`, so it can't route).
    #[prop(static)]
    pub trailing: Adornment,
    /// Pin an exact minimum input height in pixels. When set, it layers a
    /// `min-height` on top of the size-derived height so the input can't
    /// shrink below it — use this to match a design's exact px floor (e.g.
    /// `min_height = Some(48.0)`) instead of guessing a `size`. `None`
    /// (the default) leaves the height fully size-derived.
    #[schema(constraint = "pixels; None = size-derived height")]
    pub min_height: Option<f32>,
    /// Pin an exact input width in pixels. `None` (the default) lets the
    /// input fill its column as usual; `Some(px)` fixes the width.
    #[schema(constraint = "pixels; None = fill column")]
    pub width: Option<f32>,
    /// Bind the field's underlying input to a `Ref<TextInputHandle>` for
    /// imperative `focus()` / `blur()` / `select_all()`. `None` (default) binds
    /// nothing. Works for both plain and adorned layouts (the ref tracks the
    /// inner input either way).
    #[prop(static)]
    pub field_ref: Option<runtime_core::Ref<runtime_core::primitives::text_input::TextInputHandle>>,
}

impl Default for FieldProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(None),
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            placeholder: Reactive::Static(None),
            help: Reactive::Static(None),
            error: Reactive::Static(None),
            tone: Reactive::Static(None),
            size: Reactive::Static(FieldSize::default()),
            variant: Reactive::Static(FieldAppearance::default()),
            secure: Reactive::Static(false),
            leading: Adornment::None,
            trailing: Adornment::None,
            min_height: Reactive::Static(None),
            width: Reactive::Static(None),
            field_ref: None,
        }
    }
}

// -----------------------------------------------------------------------------
// Programmatic Field input + help stylesheets (lazily installed)
// -----------------------------------------------------------------------------

thread_local! {
    static FIELD_INPUT_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
    static FIELD_HELP_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}

/// Install a custom Field input stylesheet (e.g. with app tones).
pub fn install_field_input_sheet(sheet: Rc<StyleSheet>) {
    FIELD_INPUT_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}

pub(crate) fn field_input_sheet() -> Rc<StyleSheet> {
    FIELD_INPUT_SHEET.with(|s| {
        if s.borrow().is_none() {
            let tones: Vec<ToneRef> = ToneRef::builtins().into_iter().map(|(_, t)| t).collect();
            *s.borrow_mut() = Some(build_field_input_sheet(tones));
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

pub(crate) fn field_help_sheet() -> Rc<StyleSheet> {
    FIELD_HELP_SHEET.with(|s| {
        if s.borrow().is_none() {
            let tones: Vec<ToneRef> = ToneRef::builtins().into_iter().map(|(_, t)| t).collect();
            *s.borrow_mut() = Some(build_field_help_sheet(tones));
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

/// Build the Field input sheet — size axis, tone axis (default +
/// per-tone border color), and focused/disabled state overlays.
pub fn build_field_input_sheet(tones: Vec<ToneRef>) -> Rc<StyleSheet> {
    let surface = || Tokenized::token("color-surface", runtime_core::Color("#ffffff".into()));
    let text = || Tokenized::token("color-text", runtime_core::Color("#1a1a1f".into()));
    let border = || Tokenized::token("color-border", runtime_core::Color("#e4e6ef".into()));
    let radius = || Tokenized::token("radius-md", Length::Px(8.0));

    let mut sheet = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
        background: Some(surface()),
        color: Some(text()),
        border_top_left_radius: Some(radius()),
        border_top_right_radius: Some(radius()),
        border_bottom_left_radius: Some(radius()),
        border_bottom_right_radius: Some(radius()),
        border_top_width: Some(Tokenized::Literal(1.0)),
        border_right_width: Some(Tokenized::Literal(1.0)),
        border_bottom_width: Some(Tokenized::Literal(1.0)),
        border_left_width: Some(Tokenized::Literal(1.0)),
        border_top_color: Some(border()),
        border_right_color: Some(border()),
        border_bottom_color: Some(border()),
        border_left_color: Some(border()),
        background_transition: Some(Transition::new(250, Easing::EaseInOut)),
        color_transition: Some(Transition::new(250, Easing::EaseInOut)),
        border_top_color_transition: Some(Transition::new(150, Easing::EaseOut)),
        border_right_color_transition: Some(Transition::new(150, Easing::EaseOut)),
        border_bottom_color_transition: Some(Transition::new(150, Easing::EaseOut)),
        border_left_color_transition: Some(Transition::new(150, Easing::EaseOut)),
        ..Default::default()
    });

    // Size arms.
    let pad = |t: &'static str, px: f32| Tokenized::token(t, Length::Px(px));
    sheet = sheet
        .variant("size", "sm", move |_vs| StyleRules {
            padding_top: Some(pad("spacing-xs", 4.0)),
            padding_bottom: Some(pad("spacing-xs", 4.0)),
            padding_left: Some(pad("spacing-sm", 8.0)),
            padding_right: Some(pad("spacing-sm", 8.0)),
            font_size: Some(pad("typography-body-sm-size", 13.0)),
            ..Default::default()
        })
        .variant("size", "md", move |_vs| StyleRules {
            padding_top: Some(pad("spacing-sm", 8.0)),
            padding_bottom: Some(pad("spacing-sm", 8.0)),
            padding_left: Some(pad("spacing-md", 12.0)),
            padding_right: Some(pad("spacing-md", 12.0)),
            font_size: Some(pad("typography-body-size", 14.0)),
            ..Default::default()
        })
        .variant("size", "lg", move |_vs| StyleRules {
            padding_top: Some(pad("spacing-md", 12.0)),
            padding_bottom: Some(pad("spacing-md", 12.0)),
            padding_left: Some(pad("spacing-lg", 16.0)),
            padding_right: Some(pad("spacing-lg", 16.0)),
            font_size: Some(pad("typography-body-lg-size", 18.0)),
            ..Default::default()
        });

    // Appearance axis — the input shell. Declared BEFORE the tone arms so
    // an explicit tone (e.g. Danger on error) repaints a visible border
    // over contained/bare's transparent one. Border WIDTH stays 1 (set on
    // the base) in every variant so the focused-state ring renders.
    //   - outline: bordered surface (the base look) — no override.
    //   - contained: filled, borderless.
    //   - bare: no fill, no border.
    let surface_alt =
        || Tokenized::token("color-surface-alt", runtime_core::Color("#eef0f7".into()));
    let clear = || Tokenized::Literal(runtime_core::Color("transparent".into()));
    sheet = sheet
        .variant("appearance", "outline", |_vs| StyleRules::default())
        .variant("appearance", "contained", move |_vs| StyleRules {
            background: Some(surface_alt()),
            border_top_color: Some(clear()),
            border_right_color: Some(clear()),
            border_bottom_color: Some(clear()),
            border_left_color: Some(clear()),
            ..Default::default()
        })
        .variant("appearance", "bare", move |_vs| StyleRules {
            background: Some(clear()),
            border_top_color: Some(clear()),
            border_right_color: Some(clear()),
            border_bottom_color: Some(clear()),
            border_left_color: Some(clear()),
            ..Default::default()
        });

    // Tone arms — "default" = neutral base border; each tone overrides
    // the border color with its stroke color.
    sheet = sheet.variant("tone", "default", |_vs| StyleRules::default());
    for tone in &tones {
        let tone_c = tone.clone();
        sheet = sheet.variant("tone", tone.current_key(), move |_vs| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
            let neutral = tones::Neutral;
            let ctx = ResolutionCtx {
                theme: theme_ref,
                tone: &neutral,
            };
            let _ = ctx;
            let stroke = tone_c.0.stroke_color(theme_ref);
            StyleRules {
                border_top_color: Some(stroke.clone()),
                border_right_color: Some(stroke.clone()),
                border_bottom_color: Some(stroke.clone()),
                border_left_color: Some(stroke),
                ..Default::default()
            }
        });
    }

    // State overlays (web handles natively via pseudo-classes).
    sheet = sheet
        .variant("__state_focused", "on", |_vs| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
            let ring = theme_ref.colors().focus_ring.clone();
            StyleRules {
                border_top_color: Some(ring.clone()),
                border_right_color: Some(ring.clone()),
                border_bottom_color: Some(ring.clone()),
                border_left_color: Some(ring),
                ..Default::default()
            }
        })
        .variant("__state_disabled", "on", |_vs| StyleRules {
            opacity: Some(Tokenized::Literal(0.55)),
            ..Default::default()
        });

    sheet = sheet
        .variant_default("size", "md")
        .variant_default("tone", "default")
        .variant_default("appearance", "outline");

    Rc::new(sheet)
}

/// Build the Field help-text sheet — a tone axis driving the text
/// color (default = muted, each tone = its soft foreground).
pub fn build_field_help_sheet(tones: Vec<ToneRef>) -> Rc<StyleSheet> {
    let mut sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
        font_size: Some(Tokenized::token(
            "typography-caption-size",
            Length::Px(12.0),
        )),
        color_transition: Some(Transition::new(250, Easing::EaseInOut)),
        ..Default::default()
    });
    sheet = sheet.variant("tone", "default", |_vs| {
        let theme_rc = active_theme();
        let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
        StyleRules {
            color: Some(theme_ref.colors().text_muted.clone()),
            ..Default::default()
        }
    });
    for tone in &tones {
        let tone_c = tone.clone();
        sheet = sheet.variant("tone", tone.current_key(), move |_vs| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
            StyleRules {
                color: Some(tone_c.0.soft_fg(theme_ref)),
                ..Default::default()
            }
        });
    }
    sheet = sheet.variant_default("tone", "default");
    Rc::new(sheet)
}

fn size_key(size: FieldSize) -> &'static str {
    size.as_variant_str()
}

/// Renders a labeled text input with optional helper/error text. Composes
/// an optional label, a `text_input` styled by the size × tone × variant
/// axes, and a helper/error line (error takes precedence) into a column.
#[component]
pub fn Field(props: &FieldProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();

    // `size` snapshot, used only for the adornment SIZING + adorned-shell
    // layout below (size-derived icon px / row gap). A live `size` re-styles
    // the INPUT via the reactive style closure below; making the adornment
    // glyph size + row gap track it too needs those sinks routed reactively
    // as well — the same shape, tracked as a follow-on.
    let size = props.size.get();

    let error = props.error.clone();

    // The input style is REACTIVE when any style-driving prop is live: a
    // single apply-style Effect re-resolves the whole `StyleApplication`
    // (size × appearance × tone × dims) whenever any of them changes,
    // because `make_input_style` reads each prop's `.get()` INSIDE the
    // closure (so the Effect subscribes to them). When every input is
    // `Static` it collapses to one build-time resolution — no Effect, no
    // first-paint flicker. The tone arm preserves the D9 rule: tone matters
    // reactively only when it's a live prop, or absent-and-derived from a
    // live `error` (an explicit fixed tone makes `error` style-irrelevant).

    // Resolve the effective tone key, reading the `tone`/`error` props LIVE.
    // Explicit tone wins; otherwise Danger when a live error is present.
    let tone_key_for = {
        let tone = props.tone.clone();
        let error = error.clone();
        move || -> String {
            let resolved: Option<ToneRef> = tone.get().or_else(|| {
                if error.get().is_some() {
                    Some(tones::Danger.into())
                } else {
                    None
                }
            });
            resolved.as_ref().map(|t| t.key()).unwrap_or("default").to_string()
        }
    };

    // Build the input's `StyleApplication` for a tone key, reading every
    // other style prop LIVE inside so the apply-style Effect subscribes to
    // them. Called once (static path) or per apply-style fire (reactive
    // path); see the dispatch below.
    let make_input_style = {
        let size = props.size.clone();
        let variant = props.variant.clone();
        let min_height = props.min_height.clone();
        let width = props.width.clone();
        move |tone_key: String, focused: bool| -> StyleApplication {
            let size_str = size_key(size.get()).to_string();
            let appearance = variant.get().as_variant_str().to_string();
            let min_height = min_height.get();
            let width = width.get();
            let mut app = StyleApplication::new(field_input_sheet())
                .with("size", size_str)
                .with("appearance", appearance)
                .with("tone", tone_key);
            // Pin min-height / width AND the focus ring in ONE computed layer.
            // `with_computed` is single-slot (a second call overwrites the
            // first), so dims and the focus border MUST share one layer — else
            // focusing a field with `min_height` would drop it (the same
            // single-slot trap that made the adorned shell grow on focus).
            if min_height.is_some() || width.is_some() || focused {
                let ring = if focused {
                    let theme_rc = active_theme();
                    let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
                    Some(theme_ref.colors().focus_ring.clone())
                } else {
                    None
                };
                app = app.with_computed(
                    format!("field-dim-{:?}-{:?}-{}", min_height, width, focused),
                    move || StyleRules {
                        min_height: min_height.map(|h| Tokenized::Literal(Length::Px(h))),
                        width: width.map(|w| Tokenized::Literal(Length::Px(w))),
                        border_top_color: ring.clone(),
                        border_right_color: ring.clone(),
                        border_bottom_color: ring.clone(),
                        border_left_color: ring.clone(),
                        ..Default::default()
                    },
                );
            }
            app
        }
    };

    // The help-text tone tracks the input tone. When the tone is reactive
    // the error text already re-paints via `help_combined` below; the help
    // *color* is a static decision keyed off the build-time tone (a tone
    // flip from a live error still shows the right color because the help
    // node only exists when `error`/`help` is `Some`, and Danger is the
    // only error tone). Resolve the build-time key for it.
    let help_style =
        StyleApplication::new(field_help_sheet()).with("tone", tone_key_for());

    let label_node =
        crate::components::optional_reactive_text(props.label.clone(), FieldLabel());

    // error wins over help. Combine into one `Reactive<Option<String>>`,
    // staying `Static` when both inputs are (no Effect, and no empty
    // help slot when both are absent).
    let help_combined = match (props.error.clone(), props.help.clone()) {
        (Reactive::Static(e), Reactive::Static(h)) => Reactive::Static(e.or(h)),
        (e, h) => Reactive::Dynamic(Rc::new(move || e.get().or_else(|| h.get()))),
    };
    let help_node = crate::components::optional_reactive_text(help_combined, help_style);

    let secure = props.secure.clone();
    let leading = render_adornment(&props.leading, size);
    let trailing = render_adornment(&props.trailing, size);
    let adorned = leading.is_some() || trailing.is_some();

    // `placeholder` is routed LIVE: a reactive source updates the native
    // placeholder in place (no rebuild); a `Static` one sets it once.
    let mut input = runtime_core::text_input(value, move |v: String| (on_change)(v))
        .secure(secure)
        .placeholder_reactive(props.placeholder.clone());
    if let Some(field_ref) = props.field_ref.clone() {
        input = input.bind(field_ref);
    }

    // The "field box" — either the bare input (no adornments) or a flex-row
    // SHELL wrapping a bare input with leading/trailing adornments.
    let field_box: Element = if adorned {
        // ADORNED: the chrome lives on the row SHELL; the input is bare so it
        // sits flush beside the adornments (any width works — flex layout).
        //
        // FOCUS RING: a plain `view` shell can't receive the inner input's
        // FOCUSED state, so the shell can't resolve the sheet's `__state_focused`
        // overlay on its own. We bridge it with the input's `on_focus` event: it
        // sets a `focused` signal, and the shell style is a reactive closure that
        // overlays the theme `focus_ring` border colors while focused — the same
        // ring the non-adorned branch gets natively, now on the shell.
        let focused = Signal::new(false);
        let size_str = size_key(size).to_string();
        let bare_style = StyleApplication::new(field_input_sheet())
            .with("size", size_str)
            .with("appearance", "bare")
            .with("tone", "default")
            .with_computed("field-input-bare", || StyleRules {
                flex_grow: Some(Tokenized::Literal(1.0)),
                // NB: do NOT add `flex_basis: 0` / `min_width: 0` here. The
                // shell fills the column via `width: 100%`, and that percent is
                // resolved through the shell's CONTENT size on macOS — collapse
                // the input's content contribution to zero and the shell hugs
                // the lone icon (regressed to an icon-only box). Letting the
                // input keep its auto basis is what makes `width: 100%` resolve
                // to the full field width; `flex_grow: 1` then fills the row.
                // KEEP the size-derived VERTICAL padding on the input: it's
                // what establishes the field's height, so adornments center
                // within it instead of stretching the row (an Element adornment
                // shouldn't inflate the field — only the auto-sized Icon adapts
                // the other way). Horizontal padding moves to the shell; a 2px
                // inset stays so the glyph's bearing doesn't clip the edge.
                padding_left: Some(Tokenized::Literal(Length::Px(FIELD_BARE_H_PAD))),
                padding_right: Some(Tokenized::Literal(Length::Px(FIELD_BARE_H_PAD))),
                border_top_width: Some(Tokenized::Literal(0.0)),
                border_right_width: Some(Tokenized::Literal(0.0)),
                border_bottom_width: Some(Tokenized::Literal(0.0)),
                border_left_width: Some(Tokenized::Literal(0.0)),
                background: Some(Tokenized::Literal(Color("transparent".into()))),
                ..Default::default()
            });
        let input_node = input
            .on_focus(move |f| focused.set(f))
            .with_style(bare_style)
            .into_element();

        // Shell = the field chrome (border/bg/radius/padding, tone border) +
        // a row layout. Static tone snapshot here (live-validation borders on
        // adorned fields are the same follow-on as the focus ring).
        //
        // The row GAP matches the shell's horizontal padding so the icon↔text
        // spacing equals the edge↔icon spacing — minus the input's 2px anti-clip
        // inset, so the *visual* gaps end up identical. Size-derived to track
        // the field_input_sheet size variant (sm/md/lg → 8/12/16).
        let edge_pad = match size.as_variant_str() {
            "sm" => 8.0_f32,
            "lg" => 16.0,
            _ => 12.0,
        };
        let gap_px = (edge_pad - FIELD_BARE_H_PAD).max(0.0);
        let size_key_str = size.as_variant_str().to_string();
        // Reactive shell style: base chrome + tone border + row layout, plus a
        // `focus_ring` border overlay while the inner input is focused (driven
        // by `focused`, set from the input's `on_focus`). Reading `focused.get()`
        // makes the apply-style Effect re-resolve on focus change, so the ring
        // lights/clears in place — the adorned analogue of the sheet's
        // `__state_focused` overlay the non-adorned input gets natively.
        //
        // CRITICAL: the row layout AND the focus border MUST live in ONE
        // `with_computed`. `StyleApplication::with_computed` is single-slot —
        // a second call OVERWRITES the first — so splitting them made focus drop
        // the row layout's `padding_top/bottom: 0`, and the shell sprang back to
        // the size variant's vertical padding (the field visibly grew ~16px
        // taller on focus). One layer, keyed by size+focus, keeps both.
        let make_shell = make_input_style.clone();
        let tone_for_shell = tone_key_for.clone();
        let shell_style = move || {
            let is_focused = focused.get();
            let key = format!("field-shell-{}-{}", size_key_str, is_focused);
            // `false`: the shell paints its OWN focus ring in the computed layer
            // below; make_input_style's focus would just be overwritten here.
            make_shell(tone_for_shell(), false).with_computed(key, move || {
                let ring = if is_focused {
                    let theme_rc = active_theme();
                    let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
                    Some(theme_ref.colors().focus_ring.clone())
                } else {
                    None
                };
                StyleRules {
                    flex_direction: Some(FlexDirection::Row),
                    align_items: Some(AlignItems::Center),
                    gap: Some(Tokenized::Literal(Length::Px(gap_px))),
                    // Fill the FieldGroup (it stretches to its container), so the
                    // row spans the field and the input has room to grow.
                    width: Some(Tokenized::Literal(Length::pct(100.0))),
                    // Vertical padding lives on the INPUT (it drives the box
                    // height); zeroing it here keeps the row from stretching.
                    padding_top: Some(Tokenized::Literal(Length::Px(0.0))),
                    padding_bottom: Some(Tokenized::Literal(Length::Px(0.0))),
                    // Focus ring (same layer — see CRITICAL note above).
                    border_top_color: ring.clone(),
                    border_right_color: ring.clone(),
                    border_bottom_color: ring.clone(),
                    border_left_color: ring,
                    ..Default::default()
                }
            })
        };

        let mut shell_children: Vec<Element> = Vec::with_capacity(3);
        if let Some(l) = leading {
            shell_children.push(l);
        }
        shell_children.push(input_node);
        if let Some(t) = trailing {
            shell_children.push(t);
        }
        // Builder form (not `ui!`): the shell style is a reactive CLOSURE (it
        // reads `focused`), and `with_style(closure)` is the canonical way to
        // attach a live style source — mirrors switch.rs / segmented_control.rs.
        runtime_core::view(shell_children)
            .with_style(shell_style)
            .into_element()
    } else {
        // PLAIN: chrome + focus ring on the input itself. The ring is driven the
        // SAME way as the adorned shell — `on_focus` → `focused` → a theme
        // `focus_ring` border overlay — NOT the sheet's `__state_focused`
        // variant. Reason: on macOS the state-overlay re-resolution does not
        // repaint a native text input's border (the input shows no ring at all),
        // whereas `on_focus` fires reliably from the field-editor engage/resign.
        // Using one mechanism for both branches also makes adorned + non-adorned
        // rings identical. Web keeps its native `:focus` too (harmless overlap).
        //
        // INVARIANT (D9): the style is ALWAYS a reactive closure now (it reads
        // `focused`, and also `tone`/`error` live), so an error tone still turns
        // the border red and the focus ring still lights — both re-resolve in
        // place through the apply-style Effect. (The former static fast path is
        // gone; a non-adorned Field now always carries one style Effect.)
        let focused = Signal::new(false);
        let make_input_style = make_input_style.clone();
        let tone_key_for = tone_key_for.clone();
        // The focus ring is baked into `make_input_style`'s single computed
        // layer (alongside any min_height/width), so it can't be clobbered.
        let input_style = move || make_input_style(tone_key_for(), focused.get());
        input
            .on_focus(move |f| focused.set(f))
            .with_style(input_style)
            .into_element()
    };

    let mut children: Vec<Element> = Vec::with_capacity(3);
    if let Some(l) = label_node {
        children.push(l);
    }
    children.push(field_box);
    if let Some(h) = help_node {
        children.push(h);
    }

    ui! { view(style = FieldGroup()) { children } }
}

recipe!(
    Field,
    /// Password field with a show/hide toggle, built from a reactive `secure`
    /// plus a trailing adornment. `secure = rx!(!visible.get())` makes the mask
    /// itself reactive: the Field is NOT wrapped in a `switch`, the underlying
    /// `text_input` is never rebuilt, and the typed `value` is never disturbed
    /// when the mask toggles — the framework flips the native secure-entry mode
    /// in place (on macOS, an in-place `NSSecureTextFieldCell` swap). The eye
    /// glyph is a reactive `text` leaf that flips with the same `visible`
    /// signal, so nothing in the tree is rebuilt on toggle. Swap the emoji for
    /// `icon = Some(icons_lucide::EYE/EYE_OFF)` in an app with an icon pack.
    pub fn field_password_with_visibility_toggle() -> ::runtime_core::Element {
        use crate::components::field::{Adornment, Field};
        use ::runtime_core::{pressable, rx, signal, text, ui, IntoElement};
        use ::std::rc::Rc;

        let value = signal!(String::new());
        let visible = signal!(false);
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v: String| value.set(v));
        let toggle: Rc<dyn Fn()> = Rc::new(move || visible.set(!visible.get()));

        ui! {
            Field(
                value = value,
                on_change = on_change,
                placeholder = "Password".to_string(),
                secure = rx!(!visible.get()),
                trailing = Adornment::element(move || {
                    let toggle = toggle.clone();
                    let glyph = text(move || {
                        if visible.get() { "🙈".to_string() } else { "👁".to_string() }
                    })
                    .into_element();
                    pressable(vec![glyph], move || (toggle)()).into_element()
                }),
            )
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::install_idea_theme;
    use idea_theme::theme::light_theme;
    use runtime_core::{resolve_style, StyleSource};

    /// Pull the `StyleSource` off the `text_input` node inside a built
    /// `Field` element tree (a `view` wrapping label/input/help).
    fn input_style_source(field: Element) -> StyleSource {
        let children = match field {
            Element::View { children, .. } => children,
            _ => panic!("Field renders a view wrapper"),
        };
        for c in children {
            if let Element::TextInput { style, .. } = c {
                return style.expect("Field's text_input always has a style");
            }
        }
        panic!("Field tree has no text_input node");
    }

    fn theme() {
        install_idea_theme(light_theme());
    }

    // D9 regression: a Field whose `error` is a live signal must attach
    // its input style as `StyleSource::Reactive`, and flipping the error
    // signal must change the resolved border color. Before the fix the
    // tone was snapshotted into a `StyleSource::Static` at build, so the
    // border never turned red on live validation (only the error TEXT did).
    #[test]
    fn reactive_error_drives_border_color_live() {
        theme();
        let err: Signal<Option<String>> = Signal::new(None);
        let props = FieldProps {
            error: err.into(),
            ..Default::default()
        };
        let src = input_style_source(Field(&props));

        let closure = match src {
            StyleSource::Reactive(f) => f,
            _ => panic!(
                "a Field with a reactive `error` must attach a reactive style \
                 source so the border re-resolves on validation (D9 regression)"
            ),
        };

        // No error → neutral border. The closure reads the signal each call.
        let border_none = resolve_style(&closure()).border_top_color.clone();
        err.set(Some("Required".into()));
        let border_err = resolve_style(&closure()).border_top_color.clone();

        assert!(
            border_none.is_some() && border_err.is_some(),
            "border color is set in both states"
        );
        assert_ne!(
            border_none, border_err,
            "flipping the error signal must change the input's border color \
             (Danger tone vs neutral)"
        );
    }

    // An explicit tone (or a static error) keeps the static fast path —
    // no per-Field Effect, no first-paint flicker.
    #[test]
    fn fixed_tone_uses_static_style_source() {
        theme();
        // Static error: tone is fixed at build → Static.
        let props = FieldProps {
            error: Reactive::Static(Some("bad".into())),
            ..Default::default()
        };
        assert!(
            matches!(input_style_source(Field(&props)), StyleSource::Static(_)),
            "a static error fixes the tone at build → static fast path"
        );

        // Explicit tone with a reactive error: explicit tone wins → Static.
        let err: Signal<Option<String>> = Signal::new(None);
        let props = FieldProps {
            error: err.into(),
            // Hand-written struct literals don't get `ui!`'s `.into()`, so a
            // now-reactive prop is set with an explicit `Static`.
            tone: Reactive::Static(Some(tones::Warning.into())),
            ..Default::default()
        };
        assert!(
            matches!(input_style_source(Field(&props)), StyleSource::Static(_)),
            "an explicit tone overrides error-derived tone → static fast path"
        );
    }

    // D6: a `min_height` prop pins the exact min-height in px.
    #[test]
    fn min_height_prop_sets_min_height_style() {
        theme();
        let props = FieldProps {
            min_height: Reactive::Static(Some(48.0)),
            ..Default::default()
        };
        let rules = match input_style_source(Field(&props)) {
            StyleSource::Static(app) => resolve_style(&app),
            _ => panic!("a Field without a reactive error is static"),
        };
        assert_eq!(
            rules.min_height,
            Some(Tokenized::Literal(Length::Px(48.0))),
            "min_height prop must pin an exact px min-height"
        );
    }

    // D6: width prop pins an exact px width.
    #[test]
    fn width_prop_sets_width_style() {
        theme();
        let props = FieldProps {
            width: Reactive::Static(Some(240.0)),
            ..Default::default()
        };
        let rules = match input_style_source(Field(&props)) {
            StyleSource::Static(app) => resolve_style(&app),
            _ => unreachable!(),
        };
        assert_eq!(rules.width, Some(Tokenized::Literal(Length::Px(240.0))));
    }

    /// Is the built Field's `text_input` secure flag a `Static` snapshot?
    fn input_secure_is_static(field: Element) -> bool {
        let children = match field {
            Element::View { children, .. } => children,
            _ => panic!("Field renders a view wrapper"),
        };
        for c in children {
            if let Element::TextInput { secure, .. } = c {
                return secure.is_static();
            }
        }
        panic!("Field tree has no text_input node");
    }

    // A live `secure` source must thread to the `text_input` as
    // `Reactive::Dynamic` — NOT snapshotted at build — so the mask can toggle
    // at runtime without rebuilding the Field (the password show/hide path
    // that previously needed a `switch`).
    #[test]
    fn reactive_secure_threads_through_not_flattened() {
        theme();
        let visible: Signal<bool> = Signal::new(false);
        let props = FieldProps {
            secure: runtime_core::rx!(!visible.get()),
            ..Default::default()
        };
        assert!(
            !input_secure_is_static(Field(&props)),
            "a reactive `secure` must reach the text_input as Reactive::Dynamic"
        );
    }

    // A live `size` (or any style-driving prop) must attach the input style
    // as `Reactive` so the field re-styles IN PLACE when it changes — props
    // are routed into the style sink, not snapshotted at build.
    #[test]
    fn reactive_size_drives_reactive_input_style() {
        theme();
        let big: Signal<bool> = Signal::new(false);
        let props = FieldProps {
            size: runtime_core::rx!(if big.get() { FieldSize::Lg } else { FieldSize::Sm }),
            ..Default::default()
        };
        assert!(
            matches!(input_style_source(Field(&props)), StyleSource::Reactive(_)),
            "a reactive `size` must attach a reactive input style (routed to the \
             style sink, not snapshotted)"
        );
    }

    // All-`Static` style props keep the build-time fast path — one resolution,
    // no per-Field apply-style Effect.
    #[test]
    fn all_static_style_props_use_static_fast_path() {
        theme();
        let props = FieldProps {
            size: Reactive::Static(FieldSize::Lg),
            ..Default::default()
        };
        assert!(
            matches!(input_style_source(Field(&props)), StyleSource::Static(_)),
            "static style props must keep the no-Effect fast path"
        );
    }

    // A bare `bool` stays a `Static` mask — the zero-overhead common case (no
    // per-input effect).
    #[test]
    fn static_secure_stays_static() {
        theme();
        let props = FieldProps {
            secure: true.into(),
            ..Default::default()
        };
        assert!(
            input_secure_is_static(Field(&props)),
            "a static `secure` stays Reactive::Static"
        );
    }
}

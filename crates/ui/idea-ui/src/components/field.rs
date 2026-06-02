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
//! Styles are STATIC `StyleApplication`s resolved from a programmatic
//! sheet (size × tone axes + focused/disabled states), installed
//! lazily. Static = applied at build time, theme-swapped in bulk by
//! the cohort — no per-node Effect, no first-paint transition flicker.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    component, ui, Easing, IdealystSchema, Length, Element, Reactive, Signal, StyleApplication,
    StyleRules, StyleSheet, Tokenized, Transition, VariantEnum, VariantSet,
};

use idea_theme::active_theme;
use idea_theme::extensible::{tone as tones, RefBuiltins, ResolutionCtx, ToneRef};
use idea_theme::theme::{IdeaTheme, IdeaThemeRef};

use crate::stylesheets::{FieldGroup, FieldLabel};
pub use crate::stylesheets::{FieldAppearance, FieldSize};

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
    /// `text_input` primitive's `secure` flag.
    pub secure: bool,
}

impl Default for FieldProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(None),
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            placeholder: None,
            help: Reactive::Static(None),
            error: Reactive::Static(None),
            tone: None,
            size: FieldSize::default(),
            variant: FieldAppearance::default(),
            secure: false,
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
    let placeholder = props.placeholder.clone();
    let size = props.size;
    // Error TEXT is reactive (see `help_combined` below), but the
    // error-driven TONE is a static style decision (per the framework's
    // static-style fast path), so it reads the error's value once. A
    // reactive border-on-validation would use the reactive-style path.
    let has_error = props.error.get().is_some();

    // Tone resolution: explicit `tone` wins; Danger on error; else none.
    let tone: Option<ToneRef> = props.tone.clone().or_else(|| {
        if has_error {
            Some(tones::Danger.into())
        } else {
            None
        }
    });
    let tone_key = tone.as_ref().map(|t| t.key()).unwrap_or("default").to_string();

    // STATIC styles — no per-node Effect, no first-paint flicker.
    let input_style = StyleApplication::new(field_input_sheet())
        .with("size", size_key(size).to_string())
        .with("appearance", props.variant.as_variant_str().to_string())
        .with("tone", tone_key.clone());
    let help_style = StyleApplication::new(field_help_sheet()).with("tone", tone_key);

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

    let secure = props.secure;
    let input_node: Element = if let Some(p) = placeholder {
        ui! {
            text_input(
                value = value,
                on_change = move |v: String| (on_change)(v),
                placeholder = p,
                secure = secure,
                style = input_style
            )
        }
    } else {
        ui! {
            text_input(
                value = value,
                on_change = move |v: String| (on_change)(v),
                secure = secure,
                style = input_style
            )
        }
    };

    let mut children: Vec<Element> = Vec::with_capacity(3);
    if let Some(l) = label_node {
        children.push(l);
    }
    children.push(input_node);
    if let Some(h) = help_node {
        children.push(h);
    }

    ui! { view(style = FieldGroup()) { children } }
}

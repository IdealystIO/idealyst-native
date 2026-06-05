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
    component, ui, Easing, IdealystSchema, IntoElement, Length, Element, Reactive, Signal,
    StyleApplication, StyleRules, StyleSheet, Tokenized, Transition, VariantEnum, VariantSet,
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
            min_height: None,
            width: None,
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
    let appearance = props.variant.as_variant_str().to_string();
    let min_height = props.min_height;
    let width = props.width;

    // The effective tone is reactive iff it's *derived* from a live error
    // signal — i.e. no explicit tone was given AND `error` is `Dynamic`.
    // An explicit `tone`, or a `Static` error, fixes the tone at build.
    let explicit_tone = props.tone.clone();
    let error = props.error.clone();
    let tone_is_reactive = explicit_tone.is_none() && !error.is_static();

    // Derive the effective tone key from the *current* error value.
    // Explicit tone wins; otherwise Danger when error is present.
    let tone_key_for = {
        let explicit_tone = explicit_tone.clone();
        let error = error.clone();
        move || -> String {
            let tone: Option<ToneRef> = explicit_tone.clone().or_else(|| {
                if error.get().is_some() {
                    Some(tones::Danger.into())
                } else {
                    None
                }
            });
            tone.as_ref().map(|t| t.key()).unwrap_or("default").to_string()
        }
    };

    // Build the input's `StyleApplication` for a given tone key. Factored
    // into a closure so it can be evaluated once (static fast path) or per
    // apply-style fire (reactive path); see the dispatch below.
    let make_input_style = {
        let appearance = appearance.clone();
        let size_str = size_key(size).to_string();
        move |tone_key: String| -> StyleApplication {
            let mut app = StyleApplication::new(field_input_sheet())
                .with("size", size_str.clone())
                .with("appearance", appearance.clone())
                .with("tone", tone_key);
            // Pin an exact min-height / width on top of the size-derived
            // box. Keyed by the px values so identical configs dedupe to
            // one backend class.
            if min_height.is_some() || width.is_some() {
                app = app.with_computed(
                    format!("field-dim-{:?}-{:?}", min_height, width),
                    move || StyleRules {
                        min_height: min_height
                            .map(|h| Tokenized::Literal(Length::Px(h))),
                        width: width.map(|w| Tokenized::Literal(Length::Px(w))),
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

    let secure = props.secure;
    let mut input = runtime_core::text_input(value, move |v: String| (on_change)(v))
        .secure(secure);
    if let Some(p) = placeholder {
        input = input.placeholder(p);
    }
    // INVARIANT (D9): when the tone is derived from a live `error` signal,
    // the input style MUST be attached as a *reactive* closure (read
    // `error.get()` inside it) so the apply-style Effect re-subscribes and
    // re-resolves the border on every validation change. A pre-built
    // `StyleApplication` is `StyleSource::Static` — applied once at mount,
    // only re-run on theme swaps — so it would snapshot the border color at
    // build time and never turn it red on live validation (the error TEXT
    // updates regardless, via `help_combined`, which is why only the border
    // regressed). Keep the static fast path when the tone is fixed to avoid
    // a per-Field Effect + first-paint flicker.
    let input_node: Element = if tone_is_reactive {
        let make_input_style = make_input_style.clone();
        let tone_key_for = tone_key_for.clone();
        input
            .with_style(move || make_input_style(tone_key_for()))
            .into_element()
    } else {
        input.with_style(make_input_style(tone_key_for())).into_element()
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
            tone: Some(tones::Warning.into()),
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
            min_height: Some(48.0),
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
            width: Some(240.0),
            ..Default::default()
        };
        let rules = match input_style_source(Field(&props)) {
            StyleSource::Static(app) => resolve_style(&app),
            _ => unreachable!(),
        };
        assert_eq!(rules.width, Some(Tokenized::Literal(Length::Px(240.0))));
    }
}

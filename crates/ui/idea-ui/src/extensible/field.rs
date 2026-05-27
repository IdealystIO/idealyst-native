//! `Field` — labeled text input with optional helper/error text,
//! built on the extensible Tone trait surface.
//!
//! ```ignore
//! use idea_theme::extensible::tone;
//!
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
//! `tone` (optional) drives the input border and help-text color.
//! Default is no tone — the field uses the theme's neutral border.
//! When `error` is `Some(...)`, the field auto-applies `tone::Danger`
//! if no explicit tone is given; otherwise the explicit tone wins.
//!
//! `size` stays a closed enum (`FieldSize`) — it controls intrinsic
//! input height which doesn't fit the `ButtonSize` slot vocabulary.

use std::rc::Rc;

use runtime_core::{ui, Primitive, Signal, StyleApplication, StyleRules, Tokenized, VariantEnum};

use idea_theme::extensible::{tone as tones, ResolutionCtx, Tone};
use idea_theme::theme::IdeaThemeRef;

use crate::stylesheets::{Field as FieldSheet, FieldGroup, FieldHelp, FieldLabel};
pub use crate::stylesheets::FieldSize;

pub struct FieldProps {
    pub label: Option<String>,
    pub value: Signal<String>,
    pub on_change: Rc<dyn Fn(String)>,
    pub placeholder: Option<String>,
    pub help: Option<String>,
    pub error: Option<String>,
    /// Optional tone overlay. When `Some`, the field's border + help
    /// text use this tone's `stroke_color` / `soft_fg`. When `None`
    /// and `error` is set, `tone::Danger` is applied automatically.
    pub tone: Option<Rc<dyn Tone>>,
    pub size: FieldSize,
}

impl Default for FieldProps {
    fn default() -> Self {
        Self {
            label: None,
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            placeholder: None,
            help: None,
            error: None,
            tone: None,
            size: FieldSize::default(),
        }
    }
}

fn size_key(size: FieldSize) -> &'static str {
    size.as_variant_str()
}

pub fn field(props: &FieldProps) -> Primitive {
    let value = props.value;
    let on_change = props.on_change.clone();
    let placeholder = props.placeholder.clone();
    let size = props.size;
    let has_error = props.error.is_some();

    // Tone resolution: explicit `tone` prop wins; falls back to
    // tone::Danger when error is present; otherwise None.
    let tone: Option<Rc<dyn Tone>> = props.tone.clone().or_else(|| {
        if has_error {
            Some(Rc::new(tones::Danger))
        } else {
            None
        }
    });

    let tone_key_for_cache = tone.as_ref().map(|t| t.key()).unwrap_or("_");
    let input_cache_key = format!("field+{}+{}", tone_key_for_cache, size_key(size));
    let help_cache_key = format!("field-help+{}", tone_key_for_cache);

    // The compute closure either returns tone-driven border styles
    // (when a tone is present) or an empty StyleRules (when None — the
    // base sheet's neutral border is used as-is). Same code path
    // either way.
    let input_style = {
        let tone = tone.clone();
        let cache_key = input_cache_key;
        let size_str = size_key(size);
        move || {
            let _ = idea_theme::active_theme()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            let tn = tone.clone();
            let compute = move || -> StyleRules {
                let Some(t) = tn.as_ref() else {
                    return StyleRules::default();
                };
                let theme = idea_theme::active_theme();
                let theme_ref = theme
                    .downcast_ref::<IdeaThemeRef>()
                    .expect("idea-ui: no IdeaTheme installed");
                let ctx = ResolutionCtx {
                    theme: theme_ref,
                    tone: &**t,
                };
                let stroke = ctx.tone.stroke_color(ctx.theme);
                let one_px: Tokenized<f32> = Tokenized::Literal(1.0);
                StyleRules {
                    border_top_width: Some(one_px.clone()),
                    border_right_width: Some(one_px.clone()),
                    border_bottom_width: Some(one_px.clone()),
                    border_left_width: Some(one_px),
                    border_top_color: Some(stroke.clone()),
                    border_right_color: Some(stroke.clone()),
                    border_bottom_color: Some(stroke.clone()),
                    border_left_color: Some(stroke),
                    ..Default::default()
                }
            };
            StyleApplication::new(FieldSheet::sheet())
                .with("size", size_str.to_string())
                .with_computed(cache_key.clone(), compute)
        }
    };

    let help_style = {
        let tone = tone.clone();
        let cache_key = help_cache_key;
        move || {
            let _ = idea_theme::active_theme()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            let tn = tone.clone();
            let compute = move || -> StyleRules {
                let Some(t) = tn.as_ref() else {
                    return StyleRules::default();
                };
                let theme = idea_theme::active_theme();
                let theme_ref = theme
                    .downcast_ref::<IdeaThemeRef>()
                    .expect("idea-ui: no IdeaTheme installed");
                let ctx = ResolutionCtx {
                    theme: theme_ref,
                    tone: &**t,
                };
                StyleRules {
                    color: Some(ctx.tone.soft_fg(ctx.theme)),
                    ..Default::default()
                }
            };
            StyleApplication::new(FieldHelp::sheet()).with_computed(cache_key.clone(), compute)
        }
    };

    let label_text = props.label.clone();
    let help_text = props.error.clone().or_else(|| props.help.clone());

    let input_node: Primitive = if let Some(p) = placeholder {
        ui! {
            TextInput(
                value = value,
                on_change = move |v: String| (on_change)(v),
                placeholder = p,
                style = input_style
            )
        }
    } else {
        ui! {
            TextInput(
                value = value,
                on_change = move |v: String| (on_change)(v),
                style = input_style
            )
        }
    };

    let mut children: Vec<Primitive> = Vec::with_capacity(3);
    if let Some(l) = label_text {
        children.push(ui! { Text(style = FieldLabel()) { l } });
    }
    children.push(input_node);
    if let Some(h) = help_text {
        children.push(ui! { Text(style = help_style) { h } });
    }

    ui! { View(style = FieldGroup()) { children } }
}

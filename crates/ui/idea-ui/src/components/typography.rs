//! `Typography` — text component driven by the extensible
//! `TypographyKind` trait.
//!
//! ```ignore
//! ui! { Typography(content = "Welcome".into(), kind = typography_kind::H1) }
//! ```
//!
//! Styling routes through the installed Typography stylesheet (set by
//! `install_idea_theme`). Three axes: `kind` (font characteristics),
//! `color` (default / muted / tone-driven), `align`. Every combination
//! is pre-generated, so apply-style is a className lookup.
//!
//! Color precedence: `tone: Some(...)` wins, then `muted: true`, then
//! the theme's default text color.

use runtime_core::{
    component, text, FontFamily, IdealystSchema, IntoElement, Element, Reactive, StyleApplication,
    StyleRules, TextAlign,
};

use idea_theme::extensible::{installed_typography_sheet, ToneRef, TypographyKindRef};

// Reactive-by-default: `#[props]` wraps each data field `T` → `Reactive<T>`.
// `content` routes to the `text()` sink; the style-driving props (kind/tone/
// muted/font/align) route to the style sink. A bare value stays a zero-cost
// `Static` snapshot (the no-flicker fast path); a `Signal`/`rx!` re-styles in
// place.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct TypographyProps {
    /// Text content. `Reactive<String>` so it can carry live text: a
    /// string literal / `String` is static, a `Signal<String>` or
    /// `rx!(…)` re-renders the text in place when its signals change —
    /// no parent rebuild. The `ui!`/`jsx!` dispatch coerces all of these
    /// via `.into()`, so call sites are unchanged for the static case.
    pub content: Reactive<String>,
    /// Typographic role (font family/size/weight/line-height), e.g.
    /// H1/Body/Caption. Default Body.
    pub kind: TypographyKindRef,
    /// Optional intent-colored text. When `Some`, overrides `muted`.
    pub tone: Option<ToneRef>,
    /// When `true` and `tone` is `None`, use the theme's muted text color.
    pub muted: bool,
    /// Optional per-instance font family override. `None` inherits the
    /// theme's default font (a system-sans stack out of the box). Set a
    /// `FontFamily::Typeface(...)` — built via the framework's
    /// `typeface!` macro — to render this text in a registered brand
    /// face, or a `FontFamily::System("Courier New, monospace".into())`
    /// to name a platform/system family. The framework registers a
    /// `Typeface` with the backend on first use.
    ///
    /// Skipped from DocControls — `FontFamily` isn't a doc-control
    /// input type (no enumerable variants / text field), so the panel
    /// omits it.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub font: Option<FontFamily>,
    /// Skipped from DocControls — `TextAlign` is a framework enum
    /// without `VariantEnum`, and the docs-derive heuristic flags any
    /// `*Align` field as a VariantEnum by convention.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub align: TextAlign,
}

impl Default for TypographyProps {
    fn default() -> Self {
        Self {
            content: Reactive::Static(String::new()),
            kind: Reactive::Static(TypographyKindRef::default()),
            tone: Reactive::Static(None),
            muted: Reactive::Static(false),
            font: Reactive::Static(None),
            align: Reactive::Static(TextAlign::Left),
        }
    }
}

/// Themed text. Renders `content` at the given `kind` (H1…H6, Body,
/// Caption, …) using the theme's type scale — the standard way to put
/// text on screen with consistent typography.
#[component]
pub fn Typography(props: &TypographyProps) -> Element {
    let content = props.content.clone();

    // The style is REACTIVE when any style-driving prop is live; otherwise it
    // stays the build-time fast path (applied before first paint, theme-swapped
    // in bulk — no per-node Effect, no first-paint color flicker). The
    // closure reads each prop live INSIDE, so the apply-style Effect subscribes
    // to whichever are dynamic.
    let style_is_reactive = !props.kind.is_static()
        || !props.tone.is_static()
        || !props.muted.is_static()
        || !props.font.is_static()
        || !props.align.is_static();

    let make_style = {
        let kind = props.kind.clone();
        let tone = props.tone.clone();
        let muted = props.muted.clone();
        let font = props.font.clone();
        let align = props.align.clone();
        move || -> StyleApplication {
            let kind_key = kind.get().key().to_string();
            // Color precedence: tone wins, then muted, then default.
            let color_key = match (tone.get(), muted.get()) {
                (Some(t), _) => t.key().to_string(),
                (None, true) => "muted".to_string(),
                (None, false) => "default".to_string(),
            };
            let align_key = match align.get() {
                TextAlign::Left => "left",
                TextAlign::Center => "center",
                TextAlign::Right => "right",
                TextAlign::Justify => "justify",
            }
            .to_string();

            let mut style = StyleApplication::new(installed_typography_sheet())
                .with("kind", kind_key)
                .with("color", color_key)
                .with("align", align_key);

            // Per-instance font override, layered over the sheet base. The
            // cache key encodes the family identity so identical faces share
            // one resolved class.
            if let Some(font) = font.get() {
                let key = format!("font:{}", font_override_key(&font));
                style = style.with_computed(key, move || StyleRules {
                    font_family: Some(font.clone()),
                    ..Default::default()
                });
            }
            style
        }
    };

    if style_is_reactive {
        text(content).with_style(make_style).into_element()
    } else {
        text(content).with_style(make_style()).into_element()
    }
}

/// Stable cache-key fragment for a font override. A `System` family is
/// keyed by its stack string; a `Typeface` by its registry id (the same
/// dedup key the framework's `FontFamily` equality uses). Two overrides
/// with the same key MUST resolve to the same `font_family`, which holds
/// because identical families produce identical keys here.
fn font_override_key(font: &FontFamily) -> String {
    match font {
        FontFamily::System(name) => format!("sys:{name}"),
        FontFamily::Typeface(tf) => format!("tf:{}", tf.id.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::{install_idea_theme, light_theme, DEFAULT_FONT_STACK};
    use runtime_core::{resolve_style, StyleSource};

    /// Pull the `StyleSource` off the `text` node a `Typography` renders.
    fn typography_style(t: Element) -> StyleSource {
        match t {
            Element::Text { style, .. } => style.expect("Typography text always carries a style"),
            _ => panic!("Typography renders a text node"),
        }
    }

    fn resolve(t: Element) -> runtime_core::StyleRules {
        match typography_style(t) {
            StyleSource::Static(app) => (*resolve_style(&app)).clone(),
            _ => panic!("Typography uses a static style source"),
        }
    }

    /// Field report 3.1(b): with the default theme and no per-instance
    /// override, Typography must still carry a font_family — the theme's
    /// sans stack — so web text isn't left in the browser serif fallback.
    #[test]
    fn default_typography_inherits_theme_sans_font() {
        install_idea_theme(light_theme());
        let rules = resolve(Typography(&TypographyProps::default()));
        match rules.font_family {
            Some(FontFamily::System(stack)) => {
                assert_eq!(stack, DEFAULT_FONT_STACK);
                assert!(stack.contains("sans-serif"));
            }
            other => panic!("expected the theme's sans font_family, got {other:?}"),
        }
    }

    /// Field report 3.1(a): a per-instance `font` override carries into
    /// the resolved style's `font_family`, overriding the theme default.
    #[test]
    fn font_prop_override_carries_into_resolved_style() {
        install_idea_theme(light_theme());
        let props = TypographyProps {
            font: Reactive::Static(Some(FontFamily::System("Courier New, monospace".to_string()))),
            ..Default::default()
        };
        let rules = resolve(Typography(&props));
        match rules.font_family {
            Some(FontFamily::System(stack)) => assert_eq!(stack, "Courier New, monospace"),
            other => panic!("expected the overridden font_family, got {other:?}"),
        }
    }

    /// A registered `Typeface` override resolves through too — the path
    /// authors use for a real brand face (`typeface!` → `.into()`).
    #[test]
    fn typeface_override_carries_into_resolved_style() {
        install_idea_theme(light_theme());
        // Minimal Typeface value; only `id`/family identity matters for
        // resolution + cache keying.
        let tf = runtime_core::Typeface {
            id: runtime_core::TypefaceId(0xBEEF),
            family_name: "BrandSans",
            faces: &[],
            fallback: runtime_core::SystemFallback::SansSerif,
        };
        let props = TypographyProps {
            font: Reactive::Static(Some(FontFamily::Typeface(tf))),
            ..Default::default()
        };
        let rules = resolve(Typography(&props));
        match rules.font_family {
            Some(FontFamily::Typeface(got)) => assert_eq!(got.id, tf.id),
            other => panic!("expected the typeface font_family, got {other:?}"),
        }
    }
}

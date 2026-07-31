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
    component, text, FontFamily, FontWeight, IdealystSchema, IntoElement, Element, Reactive,
    Role, StyleApplication, StyleRules, TextAlign,
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
    /// H1/Body/Caption. Default Body. VISUAL type scale only — a heading
    /// `kind` does not set an accessibility heading role (see the
    /// component doc for how to mark a real heading).
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
    /// Optional per-instance font-weight override. `None` inherits the
    /// weight baked into `kind` (e.g. Body → Normal, H2 → SemiBold). Set a
    /// `FontWeight` to keep a `kind`'s size/line-height/tracking while
    /// rendering at a different weight — the common case being themed text
    /// at a body size that needs Medium/SemiBold emphasis (nav links,
    /// labels) without inventing a bespoke `kind`. Layered over the sheet
    /// base, so it wins over the kind's weight.
    ///
    /// Skipped from DocControls — `FontWeight` is a framework enum the
    /// docs-derive heuristic doesn't enumerate as a VariantEnum.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub weight: Option<FontWeight>,
    /// Skipped from DocControls — `TextAlign` is a framework enum
    /// without `VariantEnum`, and the docs-derive heuristic flags any
    /// `*Align` field as a VariantEnum by convention.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub align: TextAlign,
    /// Accessibility role override. Default `None` = AUTO: a heading `kind`
    /// (`display`, `h1`…`h6`, or a custom kind whose `is_heading()` is true)
    /// gets `Role::Header` so screen readers, locators, and platform a11y
    /// trees see a real heading — matching what the visual scale implies;
    /// non-heading kinds get no role (natural text). Set `Some(role)` to
    /// force a specific role, including `Some(Role::Text)` to opt a heading
    /// kind OUT of the heading role (e.g. large text that isn't a section
    /// title).
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub a11y_role: Option<Role>,
}

impl Default for TypographyProps {
    fn default() -> Self {
        Self {
            content: Reactive::Static(String::new()),
            kind: Reactive::Static(TypographyKindRef::default()),
            tone: Reactive::Static(None),
            muted: Reactive::Static(false),
            font: Reactive::Static(None),
            weight: Reactive::Static(None),
            align: Reactive::Static(TextAlign::Left),
            a11y_role: Reactive::Static(None),
        }
    }
}

/// Themed text. Renders `content` at the given `kind` (H1…H6, Body,
/// Caption, …) using the theme's type scale — the standard way to put
/// text on screen with consistent typography.
///
/// **Accessibility**: a heading `kind` (`display`, `h1`…`h6`) automatically
/// gets `Role::Header`, so screen readers, locators, and platform a11y
/// trees treat it as a real heading — the visual scale and the semantics
/// stay in sync by default. Override with the `a11y_role` prop:
/// `a11y_role = Some(Role::Text)` opts a heading kind OUT (large text that
/// isn't a section title); `a11y_role = Some(other)` forces a specific
/// role. A reactive `kind` that changes heading-ness after build does NOT
/// re-derive the role (the role is resolved once at build) — pass an
/// explicit `a11y_role` if you animate between heading and body kinds.
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
        || !props.weight.is_static()
        || !props.align.is_static();

    let make_style = {
        let kind = props.kind.clone();
        let tone = props.tone.clone();
        let muted = props.muted.clone();
        let font = props.font.clone();
        let weight = props.weight.clone();
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

            // Per-instance weight override, layered over the kind's baked-in
            // weight (added AFTER `kind` so it wins). The cache key encodes the
            // weight so identical overrides share one resolved class.
            if let Some(w) = weight.get() {
                let key = format!("weight:{w:?}");
                style = style.with_computed(key, move || StyleRules {
                    font_weight: Some(w),
                    ..Default::default()
                });
            }
            style
        }
    };

    // Resolve the accessibility role ONCE at build. Explicit `a11y_role`
    // wins; otherwise a heading kind auto-derives `Role::Header`. Reading
    // `kind.get()` here is a build-time snapshot — a reactive kind that
    // later flips heading-ness won't re-derive (documented on the component
    // + `a11y_role` prop); the static kind case is the overwhelming norm.
    let role = match props.a11y_role.get() {
        Some(r) => Some(r),
        None if props.kind.get().is_heading() => Some(Role::Header),
        None => None,
    };

    // Both branches produce the same wrapper type, so the role is folded
    // in after the style split (no `Bound<_>` type annotation — the
    // builder's own type carries it).
    let styled = if style_is_reactive {
        text(content).with_style(make_style)
    } else {
        text(content).with_style(make_style())
    };
    match role {
        Some(r) => styled.a11y_role(r).into_element(),
        None => styled.into_element(),
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
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;
    use idea_theme::{install_idea_theme, light_theme, DEFAULT_FONT_STACK};
    use runtime_core::resolve_style;

    /// Pull the normalized style slot off the `text` node a `Typography` renders.
    fn typography_style(t: Element) -> TStyle {
        match classify(t) {
            P::Text { style, .. } => style.expect("Typography text always carries a style"),
            _ => panic!("Typography renders a text node"),
        }
    }

    fn resolve(t: Element) -> runtime_core::StyleRules {
        match typography_style(t) {
            TStyle::App(app) => (*resolve_style(&app)).clone(),
            _ => panic!("Typography uses a static style source"),
        }
    }

    /// Field report 3.1(b): with the default theme and no per-instance
    /// override, Typography text must still land on the theme's sans
    /// stack, so web text isn't left in the browser serif fallback.
    ///
    /// The sheet deliberately does NOT carry the family (it used to —
    /// see `TypographySheetBuilder::build`). A baked `FontFamily` is not
    /// `Tokenized`, so it is exactly the shape premint cannot honour: a
    /// build-time class would freeze whichever theme the dump installed.
    /// The guarantee now rides the framework's default-text-font channel
    /// instead, which BOTH style paths already consume — the live path
    /// via `fill_default_text_font` at apply time, the preminted path via
    /// the `--iy-default-font` custom property the dump emits into the
    /// base rule.
    ///
    /// So this pins the two halves of the new chain that are reachable
    /// from here: the theme feeds the channel on install, and the sheet
    /// leaves the slot empty for the fill to occupy. (That the fill then
    /// happens is `style_attach`'s contract, and the end-to-end result is
    /// covered by the live-vs-preminted computed-style A/B, which found
    /// `font-family` identical on all 400 catalog elements.)
    #[test]
    fn default_typography_inherits_theme_sans_font() {
        with_test_world(|| {
            install_idea_theme(light_theme());

            // 1. Installing the theme publishes its family on the channel
            //    the fill reads.
            match runtime_core::default_text_font() {
                Some(FontFamily::System(stack)) => {
                    assert_eq!(stack, DEFAULT_FONT_STACK);
                    assert!(stack.contains("sans-serif"));
                }
                other => panic!("theme did not publish its sans font: {other:?}"),
            }

            // 2. The sheet leaves the slot empty, so nothing shadows the
            //    fill and no stale family can be preminted.
            let rules = resolve(Typography(&TypographyProps::default()));
            assert!(
                rules.font_family.is_none(),
                "Typography's sheet must leave font_family to the \
                 default-text-font channel; baking one breaks premint \
                 across a theme swap (got {:?})",
                rules.font_family,
            );
    });
    }

    /// Field report 3.1(a): a per-instance `font` override carries into
    /// the resolved style's `font_family`, overriding the theme default.
    #[test]
    fn font_prop_override_carries_into_resolved_style() {
        with_test_world(|| {
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
    });
    }

    /// A registered `Typeface` override resolves through too — the path
    /// authors use for a real brand face (`typeface!` → `.into()`).
    #[test]
    fn typeface_override_carries_into_resolved_style() {
        with_test_world(|| {
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
    });
    }

    fn a11y_role(t: Element) -> Option<Role> {
        match classify(t) {
            P::Text { accessibility, .. } => accessibility.role,
            _ => panic!("Typography renders a text node"),
        }
    }

    /// Regression (arena nav-notes, 2026-07-21): a heading `kind` produced
    /// large text with NO heading role, so locators/screen readers saw
    /// plain text and the "title as a heading" requirement was unearnable.
    /// A heading kind must now auto-attach `Role::Header`.
    #[test]
    fn regression_heading_kind_auto_attaches_header_role() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            for kind in [
                TypographyKindRef::from(idea_theme::extensible::typography::H1),
                TypographyKindRef::from(idea_theme::extensible::typography::H2),
                TypographyKindRef::from(idea_theme::extensible::typography::H3),
                TypographyKindRef::from(idea_theme::extensible::typography::Display),
            ] {
                let props = TypographyProps {
                    kind: Reactive::Static(kind.clone()),
                    ..Default::default()
                };
                assert_eq!(
                    a11y_role(Typography(&props)),
                    Some(Role::Header),
                    "kind {:?} must auto-attach Role::Header",
                    kind.key()
                );
            }
    });
    }

    #[test]
    fn body_kind_gets_no_role_and_explicit_override_wins() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            // Body (non-heading) → natural text, no role.
            assert_eq!(a11y_role(Typography(&TypographyProps::default())), None);

            // Explicit Role::Text opts a heading kind OUT of the heading role.
            let opted_out = TypographyProps {
                kind: Reactive::Static(TypographyKindRef::from(
                    idea_theme::extensible::typography::H1,
                )),
                a11y_role: Reactive::Static(Some(Role::Text)),
                ..Default::default()
            };
            assert_eq!(a11y_role(Typography(&opted_out)), Some(Role::Text));
    });
    }
}

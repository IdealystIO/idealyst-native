//! `Typography` — themed text.
//!
//! ```ignore
//! ui! { Typography(content = "Welcome".into(), kind = typography_kind::H1) }
//! ```
//!
//! Styling is [`TypographyStyles`]' `text` slot: `kind` (font size, weight,
//! line height, tracking), `tone` (`default` / `muted` / a tone's
//! foreground), `weight` and `align` arms. Every combination ships as
//! build-time CSS, so applying a style is a class lookup. An app adds a
//! kind with `TypographyKind::define("lead")` plus a restyle arm.
//!
//! Color precedence: `tone: Some(...)` wins, then `muted: true`, then
//! the theme's default text color.

use runtime_core::{
    component, component_styles, text, Element, FontFamily, FontWeight, IdealystSchema, IntoElement, Reactive,
    Role, SlotRestyle, StyleRules, TextAlign,
};

use idea_theme::extensible::{semantic_tones, tone_arms, ToneRef};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Typography`].
    pub Typography<IdeaThemeRef> {
        /// The text node.
        slot text {
            extend(tone_colors)
            // Deliberately sets NO `font_family`: the theme's font reaches
            // every Typography through the framework's default-text-font
            // channel (the live path fills an absent family at apply time,
            // the preminted path emits `var(--iy-default-font, inherit)`).
            // A family baked here is not `Tokenized`, so a build-time class
            // would freeze whichever theme the dump build installed.
            base(_t) {}
            /// The typographic role. Line heights and tracking are px.
            /// H4–H6 borrow the body ramp's size tokens at heading weight.
            variant kind {
                display(t) { font_size: t.typography.display_size(), font_weight: FontWeight::Bold, line_height: 44.0, letter_spacing: -1.0 }
                h1(t) { font_size: t.typography.h1_size(), font_weight: FontWeight::Bold, line_height: 36.0, letter_spacing: -0.8 }
                h2(t) { font_size: t.typography.h2_size(), font_weight: FontWeight::SemiBold, line_height: 28.0, letter_spacing: -0.5 }
                h3(t) { font_size: t.typography.h3_size(), font_weight: FontWeight::SemiBold, line_height: 24.0, letter_spacing: -0.2 }
                h4(t) { font_size: t.typography.body_xl_size(), font_weight: FontWeight::SemiBold, line_height: 24.0, letter_spacing: -0.1 }
                h5(t) { font_size: t.typography.body_lg_size(), font_weight: FontWeight::SemiBold, line_height: 22.0, letter_spacing: 0.0 }
                h6(t) { font_size: t.typography.body_size(), font_weight: FontWeight::SemiBold, line_height: 20.0, letter_spacing: 0.0 }
                body_xl(t) { font_size: t.typography.body_xl_size(), font_weight: FontWeight::Normal, line_height: 27.0, letter_spacing: 0.0 }
                body_lg(t) { font_size: t.typography.body_lg_size(), font_weight: FontWeight::Normal, line_height: 25.0, letter_spacing: 0.0 }
                #[default]
                body(t) { font_size: t.typography.body_size(), font_weight: FontWeight::Normal, line_height: 22.0, letter_spacing: 0.0 }
                body_sm(t) { font_size: t.typography.body_sm_size(), font_weight: FontWeight::Normal, line_height: 21.0, letter_spacing: 0.0 }
                caption(t) { font_size: t.typography.caption_size(), font_weight: FontWeight::Medium, line_height: 17.0, letter_spacing: 0.0 }
                overline(t) { font_size: t.typography.overline_size(), font_weight: FontWeight::Bold, line_height: 14.0, letter_spacing: 1.5 }
            }
            /// The color: the theme's text, its muted text, or a tone's
            /// foreground (one arm per registered tone — `tone_colors`).
            variant tone {
                #[default]
                default(t) { color: t.color.text() }
                muted(t) { color: t.color.text_muted() }
            }
            /// A per-instance weight over the kind's — an axis, so it
            /// premints.
            variant weight {
                #[default]
                inherit(_t) {}
                thin(_t) { font_weight: FontWeight::Thin }
                extra_light(_t) { font_weight: FontWeight::ExtraLight }
                light(_t) { font_weight: FontWeight::Light }
                normal(_t) { font_weight: FontWeight::Normal }
                medium(_t) { font_weight: FontWeight::Medium }
                semi_bold(_t) { font_weight: FontWeight::SemiBold }
                bold(_t) { font_weight: FontWeight::Bold }
                extra_bold(_t) { font_weight: FontWeight::ExtraBold }
                black(_t) { font_weight: FontWeight::Black }
            }
            variant align {
                #[default]
                left(_t) { text_align: TextAlign::Left }
                center(_t) { text_align: TextAlign::Center }
                right(_t) { text_align: TextAlign::Right }
                justify(_t) { text_align: TextAlign::Justify }
            }
            transitions {
                color: 250ms EaseInOut,
            }
        }
    }
}

/// One `tone` arm per registered tone: the tone's stroke foreground (its
/// text color over the page surface).
fn tone_colors(s: &mut SlotRestyle<'_, TypographyStyles>) {
    tone_arms(s, &semantic_tones(), |ctx| StyleRules {
        color: Some(ctx.tone.stroke_fg(ctx.theme)),
        ..Default::default()
    });
}

impl TypographyKind {
    /// Whether this kind is a heading (`display`, `h1`…`h6`) — a heading
    /// kind gets `Role::Header` unless `a11y_role` says otherwise. An
    /// app-defined kind is not a heading; give it `a11y_role =
    /// Some(Role::Header)` when it is one.
    pub fn is_heading(self) -> bool {
        matches!(self.key(), "display" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
    }
}

/// The built-in kinds by their short names, so `kind =
/// typography_kind::H1` reads as it always has.
#[allow(non_upper_case_globals)]
pub mod typography_kind {
    use super::TypographyKind;
    pub const Display: TypographyKind = TypographyKind::Display;
    pub const H1: TypographyKind = TypographyKind::H1;
    pub const H2: TypographyKind = TypographyKind::H2;
    pub const H3: TypographyKind = TypographyKind::H3;
    pub const H4: TypographyKind = TypographyKind::H4;
    pub const H5: TypographyKind = TypographyKind::H5;
    pub const H6: TypographyKind = TypographyKind::H6;
    pub const BodyXl: TypographyKind = TypographyKind::BodyXl;
    pub const BodyLg: TypographyKind = TypographyKind::BodyLg;
    pub const Body: TypographyKind = TypographyKind::Body;
    pub const BodySm: TypographyKind = TypographyKind::BodySm;
    pub const Caption: TypographyKind = TypographyKind::Caption;
    pub const Overline: TypographyKind = TypographyKind::Overline;
}

/// The `weight` arm for a `FontWeight`.
fn weight_value(w: FontWeight) -> TypographyWeight {
    match w {
        FontWeight::Thin => TypographyWeight::Thin,
        FontWeight::ExtraLight => TypographyWeight::ExtraLight,
        FontWeight::Light => TypographyWeight::Light,
        FontWeight::Normal => TypographyWeight::Normal,
        FontWeight::Medium => TypographyWeight::Medium,
        FontWeight::SemiBold => TypographyWeight::SemiBold,
        FontWeight::Bold => TypographyWeight::Bold,
        FontWeight::ExtraBold => TypographyWeight::ExtraBold,
        FontWeight::Black => TypographyWeight::Black,
    }
}

/// The `align` arm for a `TextAlign`.
fn align_value(a: TextAlign) -> TypographyAlign {
    match a {
        TextAlign::Left => TypographyAlign::Left,
        TextAlign::Center => TypographyAlign::Center,
        TextAlign::Right => TypographyAlign::Right,
        TextAlign::Justify => TypographyAlign::Justify,
    }
}

// Reactive-by-default: `#[props]` wraps each data field `T` → `Reactive<T>`.
// `content` routes to the `text()` sink; the style-driving props (kind/tone/
// muted/font/align) route to the style sink. A bare value stays a zero-cost
// `Static` snapshot (the no-flicker fast path); a `Signal`/`rx!` re-styles in
// place.
#[runtime_core::props(styles = TypographyStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct TypographyProps {
    /// Text content. `Reactive<String>` so it can carry live text: a
    /// string literal / `String` is static, a `Signal<String>` or
    /// `rx!(…)` re-renders the text in place when its signals change —
    /// no parent rebuild. The `ui!`/`jsx!` dispatch coerces all of these
    /// via `.into()`, so call sites are unchanged for the static case.
    pub content: Reactive<String>,
    /// Typographic role (font size/weight/line-height): `Display`,
    /// `H1`…`H6`, `BodyXl`/`BodyLg`/`Body`/`BodySm`, `Caption`,
    /// `Overline`, or an app-defined [`TypographyKind`]. Default Body. A
    /// heading kind also gets `Role::Header` unless `a11y_role` says
    /// otherwise.
    pub kind: TypographyKind,
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
    /// Show at most this many lines, ending the last with "…" where it is
    /// cut (`Some(1)`: one line, truncated). The text can then shrink in a
    /// row so a neighbour keeps its room — a name next to a number in a
    /// table cell. `None` (default): no limit. See the framework's
    /// `StyleRules::max_lines`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub lines: Option<u32>,
    /// Accessibility role override. Default `None` = AUTO: a heading `kind`
    /// (`display`, `h1`…`h6`) gets `Role::Header` so screen readers, locators, and platform a11y
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
            kind: Reactive::Static(TypographyKind::default()),
            tone: Reactive::Static(None),
            muted: Reactive::Static(false),
            font: Reactive::Static(None),
            weight: Reactive::Static(None),
            align: Reactive::Static(TextAlign::Left),
            lines: Reactive::Static(None),
            a11y_role: Reactive::Static(None),
            styles: Default::default(),
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

    // Color precedence: tone wins, then muted, then default.
    let tone_axis = {
        let (tone, muted) = (props.tone.clone(), props.muted.clone());
        let pick = |t: Option<ToneRef>, m: bool| -> &'static str {
            match (t, m) {
                (Some(t), _) => t.key(),
                (None, true) => "muted",
                (None, false) => "default",
            }
        };
        match (tone, muted) {
            (Reactive::Static(t), Reactive::Static(m)) => Reactive::Static(pick(t, m)),
            (t, m) => Reactive::Dynamic(std::rc::Rc::new(move || pick(t.get(), m.get()))),
        }
    };
    let style = TypographyStyles::text()
        .set(props.kind.clone())
        .set(props.weight.clone().map(|w| w.map(weight_value).unwrap_or(TypographyWeight::Inherit)))
        .set(props.align.clone().map(align_value));
    let style = match tone_axis {
        Reactive::Static(key) => style.raw("tone", key),
        live => style.raw_with("tone", move || live.get()),
    };
    // Per-instance font override and line limit ride the INLINE layer: a
    // typeface is an app-supplied asset, not a closed set the slot could
    // enumerate, and inline keeps the application premintable.
    let inline = |font: Option<FontFamily>, lines: Option<u32>| StyleRules {
        font_family: font,
        max_lines: lines,
        ..Default::default()
    };
    let style = match (props.font.clone(), props.lines.clone()) {
        (Reactive::Static(None), Reactive::Static(None)) => style,
        (Reactive::Static(f), Reactive::Static(l)) => style.inline(inline(f, l)),
        (f, l) => style.inline_with(move || inline(f.get(), l.get())),
    };
    let style = style.overridden(props.styles.text.as_ref());

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
    let styled = text(content).with_style(style);
    match role {
        Some(r) => styled.a11y_role(r).into_element(),
        None => styled.into_element(),
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
    /// The `text` slot deliberately does NOT carry the family. A baked `FontFamily` is not
    /// `Tokenized`, so it is exactly the shape premint cannot honour: a
    /// build-time class would freeze whichever theme the dump installed.
    /// The guarantee rides the framework's default-text-font channel
    /// instead, which BOTH style paths already consume — the live path
    /// via `fill_default_text_font` at apply time, the preminted path via
    /// the `--iy-default-font` custom property the dump emits into the
    /// base rule.
    ///
    /// So this pins the two halves of the chain that are reachable
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

    /// Regression (arena nav-notes): a heading `kind` produced
    /// large text with NO heading role, so locators/screen readers saw
    /// plain text and the "title as a heading" requirement was unearnable.
    /// A heading kind must now auto-attach `Role::Header`.
    #[test]
    fn regression_heading_kind_auto_attaches_header_role() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            for kind in [
                TypographyKind::H1,
                TypographyKind::H2,
                TypographyKind::H3,
                TypographyKind::H4,
                TypographyKind::H5,
                TypographyKind::H6,
                TypographyKind::Display,
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
                kind: Reactive::Static(TypographyKind::H1),
                a11y_role: Reactive::Static(Some(Role::Text)),
                ..Default::default()
            };
            assert_eq!(a11y_role(Typography(&opted_out)), Some(Role::Text));
    });
    }

    /// Setting BOTH `font` and `weight` must apply both.
    ///
    /// `StyleApplication` has exactly one computed slot — `with_computed`
    /// assigns rather than stacks. While `font` and `weight` each rode a
    /// computed layer, the weight layer (added second) silently overwrote the
    /// font layer, so a Typography with both set rendered the theme font at
    /// the requested weight. `weight` is now a sheet axis and `font` rides
    /// the inline layer, so nothing shares a slot.
    #[test]
    fn lines_truncates_and_keeps_a_font_override() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let r = resolve(Typography(&TypographyProps {
                content: Reactive::Static("Alexandra Bartholomew-Smith".into()),
                lines: Reactive::Static(Some(1)),
                ..Default::default()
            }));
            assert_eq!(r.max_lines, Some(1));
            let both = resolve(Typography(&TypographyProps {
                lines: Reactive::Static(Some(2)),
                font: Reactive::Static(Some(FontFamily::System("Courier New, monospace".into()))),
                ..Default::default()
            }));
            assert_eq!(both.max_lines, Some(2), "one inline layer carries both");
            assert!(both.font_family.is_some());
            let none = resolve(Typography(&TypographyProps::default()));
            assert_eq!(none.max_lines, None);
        });
    }

    #[test]
    fn regression_font_and_weight_overrides_both_apply() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let rules = resolve(Typography(&TypographyProps {
                content: Reactive::Static("Hi".to_string()),
                font: Reactive::Static(Some(FontFamily::System("Courier New".to_string()))),
                weight: Reactive::Static(Some(FontWeight::Bold)),
                ..Default::default()
            }));
            assert_eq!(rules.font_weight, Some(FontWeight::Bold));
            assert_eq!(
                rules.font_family,
                Some(FontFamily::System("Courier New".to_string())),
                "the font override must survive alongside a weight override"
            );
        });
    }

    /// A Typography with a `font` override must still PREMINT: the typeface
    /// rides the INLINE layer (out-of-band, applied over the preminted
    /// classes) — a `with_computed` layer is a premint disqualifier, so
    /// any `font = …` Typography would panic at mount in a
    /// `--premint-only` app. Fails against the computed spelling
    /// (`preminted_class_list()` is `None` for a computed-carrying
    /// application).
    #[test]
    fn regression_font_override_premints_via_inline_layer() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let style = typography_style(Typography(&TypographyProps {
                content: Reactive::Static("Hi".to_string()),
                font: Reactive::Static(Some(FontFamily::System("Courier New".to_string()))),
                ..Default::default()
            }));
            let app = match style {
                TStyle::App(a) => a,
                TStyle::AppFn(f) => f(),
                _ => panic!("Typography style is an application"),
            };
            assert!(
                app.preminted_class_list().is_some(),
                "a font-overridden Typography must premint (font rides inline)"
            );
            let inline = app.inline().expect("font override rides the inline layer");
            assert_eq!(
                inline.font_family,
                Some(FontFamily::System("Courier New".to_string()))
            );
        });
    }

    /// Each `weight` value must reach the resolved rules through the sheet's
    /// axis — a weight missing an arm would silently fall back to the kind's
    /// baked-in weight.
    #[test]
    fn every_font_weight_resolves_through_the_axis() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            for w in [
                FontWeight::Thin,
                FontWeight::ExtraLight,
                FontWeight::Light,
                FontWeight::Normal,
                FontWeight::Medium,
                FontWeight::SemiBold,
                FontWeight::Bold,
                FontWeight::ExtraBold,
                FontWeight::Black,
            ] {
                let rules = resolve(Typography(&TypographyProps {
                    content: Reactive::Static("Hi".to_string()),
                    weight: Reactive::Static(Some(w)),
                    ..Default::default()
                }));
                assert_eq!(rules.font_weight, Some(w), "weight {w:?} did not resolve");
            }
        });
    }

    /// Regression #6: the docs promised `H1…H6` but `typography_kind` stopped
    /// at `H3` — `typography_kind::H4` did not compile. H4–H6 must exist, be
    /// reachable from a `ui!` call site, and continue the heading ramp:
    /// each strictly smaller than the one above, starting below H3.
    #[test]
    fn regression_typography_kind_h4_to_h6_exist_and_continue_the_ramp() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let size_of = |kind: TypographyKind| -> f32 {
                let rules = resolve(Typography(&TypographyProps {
                    content: Reactive::Static("Hi".to_string()),
                    kind: Reactive::Static(kind),
                    ..Default::default()
                }));
                match rules.font_size.as_ref().map(|t| t.resolve()) {
                    Some(runtime_core::Length::Px(v)) => v,
                    other => panic!("expected a px font size, got {other:?}"),
                }
            };
            let sizes = [
                size_of(typography_kind::H3),
                size_of(typography_kind::H4),
                size_of(typography_kind::H5),
                size_of(typography_kind::H6),
            ];
            assert!(
                sizes.windows(2).all(|w| w[0] > w[1]),
                "H3 > H4 > H5 > H6, got {sizes:?}"
            );
            for kind in [typography_kind::H4, typography_kind::H5, typography_kind::H6] {
                assert!(TypographyKind::BUILTINS.contains(&kind), "{} is a built-in arm", kind.key());
            }
            // The `ui!` struct-literal path coerces the bare marker.
            let el = runtime_core::ui! { Typography(content = "Section", kind = typography_kind::H6) };
            assert_eq!(a11y_role(el), Some(Role::Header));
        });
    }
}

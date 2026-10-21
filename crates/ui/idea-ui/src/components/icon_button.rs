//! `IconButton` — square clickable for a glyph, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use idea_ui::{tone, variant, IconButton, IconButtonSize};
//!
//! ui! {
//!     IconButton(
//!         glyph = "×",
//!         on_click = on_dismiss,
//!         tone = tone::Neutral,
//!         variant = variant::Ghost,
//!         size = IconButtonSize::Md,
//!     )
//! }
//! ```
//!
//! Styling is [`IconButtonStyles`]: `root` (the square — tone × variant
//! compounds with hover/press feedback, the `selected` accent fill, the
//! `size` steps), `icon` (the vector icon's square) and `glyph` (the text
//! glyph, in the fill's foreground).

use std::rc::Rc;

use runtime_core::{
    component, component_styles, icon, resolve_style, text, ui, AlignItems, AlignSelf, Color, Cursor, Element,
    FontWeight, IconData, IdealystSchema, IntoElement, JustifyContent, Length, PressableHandle, Reactive, Ref,
    SlotRestyle, StyleRules, TextAlign, Tokenized, UserSelect,
};

use crate::components::tone_slots::{foreground_arms, foreground_compounds, surface_arms, with_tone};
use crate::components::tooltip::Tooltip;
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;
use idea_theme::extensible::{
    semantic_tones, tone, tone_variant_arms, tone_variant_state_overlays, with_idea_theme, ResolutionCtx, ToneRef,
};

component_styles! {
    /// The styled sections of [`IconButton`].
    pub IconButton<IdeaThemeRef> {
        /// The surface treatment — a tone-aware recipe per value.
        axis variant { #[default] filled, soft, outlined, ghost }

        /// The square.
        slot root {
            extend(root_surfaces)
            base(_t) {
                font_weight: FontWeight::SemiBold,
                text_align: TextAlign::Center,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                // Hug + center on the cross axis: a flex parent's default
                // `align-items: stretch` otherwise top-aligns the square in
                // a row of mixed sizes.
                align_self: AlignSelf::Center,
                cursor: Cursor::Pointer,
                user_select: UserSelect::None,
                // A resting value for the hover/press dim to animate back to.
                opacity: 1.0,
            }
            /// The square's side: 24 / 32 / 48 px, fully round.
            variant size {
                sm(t) {
                    padding: t.spacing.xs(),
                    font_size: t.typography.body_sm_size(),
                    width: 24.0,
                    height: 24.0,
                    border_radius: t.radius.pill(),
                }
                #[default]
                md(t) {
                    padding: t.spacing.sm(),
                    font_size: t.typography.body_size(),
                    width: 32.0,
                    height: 32.0,
                    border_radius: t.radius.pill(),
                }
                lg(t) {
                    padding: t.spacing.md(),
                    font_size: t.typography.body_lg_size(),
                    width: 48.0,
                    height: 48.0,
                    border_radius: t.radius.pill(),
                }
            }
            /// The active-toggle look; the accent fill itself is per tone
            /// (`root_surfaces`).
            variant selected {
                #[default]
                off(_t) {}
                on(_t) {}
            }
            state hovered(_t) {}
            state pressed(_t) {}
            transitions {
                background: 120ms EaseOut,
                color: 200ms EaseOut,
                opacity: 200ms EaseOut,
                border_color: 150ms EaseOut,
                shadow: 180ms EaseOut,
            }
        }

        /// The vector icon: a fixed square (icons have no intrinsic size),
        /// sized to the button's content box per step.
        slot icon {
            base(_t) { flex_shrink: 0.0 }
            variant size {
                sm(_t) { width: 16.0, height: 16.0 }
                #[default]
                md(_t) { width: 18.0, height: 18.0 }
                lg(_t) { width: 24.0, height: 24.0 }
            }
        }

        /// The text glyph, in the fill's foreground (native text inherits no
        /// color from its container).
        slot glyph {
            extend(glyph_foregrounds)
            base(_t) {}
            variant selected {
                #[default]
                off(_t) {}
                on(_t) {}
            }
        }
    }
}

/// The square's `(tone, variant)` fills, their hover/press feedback, and
/// the `selected` accent fill — the tone's solid fill and its on-fill
/// text, so a Ghost tool button fills with the accent when active.
fn root_surfaces(s: &mut SlotRestyle<'_, IconButtonStyles>) {
    let tones = semantic_tones();
    surface_arms(s, &tones);
    for (key, _) in variant::builtin_recipes() {
        tone_variant_state_overlays(s, &tones, key);
    }
    selected_accent(s, &tones, &[], true);
}

fn glyph_foregrounds(s: &mut SlotRestyle<'_, IconButtonStyles>) {
    let tones = semantic_tones();
    foreground_arms(s, &tones);
    selected_accent(s, &tones, &[], false);
}

/// The `selected` accent per tone — the tone's solid fill (`fill`, the
/// square) and its on-fill text — as a compound on `tone ∧ selected=on`
/// plus the `extra` legs.
fn selected_accent(s: &mut SlotRestyle<'_, IconButtonStyles>, tones: &[ToneRef], extra: &[(&str, &str)], fill: bool) {
    with_idea_theme(|theme| {
        for tone in tones {
            let ctx = ResolutionCtx { theme, tone: &*tone.0 };
            let mut legs = vec![("tone", tone.key()), ("selected", "on")];
            legs.extend_from_slice(extra);
            s.raw_compound(
                &legs,
                StyleRules {
                    background: fill.then(|| ctx.tone.fill_bg(ctx.theme)),
                    color: Some(ctx.tone.fill_fg(ctx.theme)),
                    ..Default::default()
                },
            );
        }
    });
}

impl IconButtonRestyle<'_, '_> {
    /// Give a variant value a tone-aware look from a surface recipe, the
    /// way the built-in variants get theirs: the square's fill per tone,
    /// its hover / press feedback, and the text glyph's foreground (a
    /// vector icon takes the square's foreground). The same helper as
    /// [`ButtonRestyle::tone_variant`](crate::ButtonRestyle::tone_variant),
    /// so a design system gives its buttons and icon buttons one recipe.
    ///
    /// ```ignore
    /// const RAISED: IconButtonVariant = IconButtonVariant::define("raised");
    /// IconButtonStyles::restyle(|s| s.tone_variant(RAISED, &Raised));
    /// ```
    ///
    /// `selected` keeps winning over the new fill: its accent compounds are
    /// restated with the variant as a third leg, because a restyle's
    /// compounds come after the declaration's and would otherwise paint
    /// over a selected button's accent.
    pub fn tone_variant(&mut self, value: IconButtonVariant, recipe: &dyn idea_theme::extensible::Variant) {
        let tones = semantic_tones();
        let key = value.key();
        let mut root = self.root();
        tone_variant_arms(&mut root, &tones, key, recipe);
        tone_variant_state_overlays(&mut root, &tones, key);
        selected_accent(&mut root, &tones, &[("variant", key)], true);
        let mut glyph = self.glyph();
        foreground_compounds(&mut glyph, &tones, key, recipe);
        selected_accent(&mut glyph, &tones, &[("variant", key)], false);
    }
}

marker_into_axis!(IconButtonVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

// Reactive-by-default: `#[props]` wraps each data field `T` → `Reactive<T>`;
// `on_click` (Rc handler) is skipped. The style-driving props (tone/variant/
// size/disabled/selected/size_px/radius/color) route to the `make_style`
// sink reading `.get()` INSIDE; `glyph`/`icon`/`size_px`/`size` route to the
// child sink. A bare value stays a zero-cost `Static` snapshot (the
// no-flicker fast path); a `Signal`/`rx!` re-styles in place.
#[runtime_core::props(styles = IconButtonStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct IconButtonProps {
    /// The single glyph/character rendered inside the square (e.g. `"×"`).
    /// Used only when `icon` is `None` — pass `icon` to render a vector
    /// (Lucide) icon instead of a text glyph.
    pub glyph: String,
    /// Optional vector icon to render inside the square. When `Some`, it
    /// takes precedence over `glyph` — e.g. `icon = Some(icons_lucide::X)`
    /// for a Lucide close button. Tinted to the button's tone × variant
    /// foreground, stamped on the icon's own node — native backends have
    /// no cascade to inherit it from. Override with `color`.
    pub icon: Option<IconData>,
    /// Fires on press/click.
    pub on_click: Rc<dyn Fn()>,
    /// Semantic color palette (Neutral, Primary, Danger, …). Default Neutral.
    pub tone: ToneRef,
    /// Surface treatment (Filled, Ghost, Soft, …, or an app value). Default
    /// Filled.
    pub variant: IconButtonVariant,
    /// Square dimension preset (Sm, Md, Lg). Default Md.
    pub size: IconButtonSize,
    /// When `true`, blocks the press and dims the button. Default `false`.
    /// For reactive disabling, read a signal in the enclosing render scope
    /// (`disabled = some_state.get()`); the scope re-renders on change.
    pub disabled: bool,
    /// When `true`, paints the button with the tone's accent fill — the
    /// "active toggle" look (a selected tool in a toolbar). Default `false`.
    /// Pair with `variant = Ghost` for the transparent-resting tool button that
    /// fills with the accent when active. For reactive selection, read a signal
    /// in the enclosing render scope (`selected = tool.get() == Tool::Pen`).
    pub selected: bool,
    /// Override the square's side length, in px. When `Some`, it replaces the
    /// `size` step's width/height and rescales the icon — for fitting a specific
    /// layout (e.g. a 42px toolbar button) while still using the themed
    /// IconButton. Default `None` (the `size` enum decides).
    pub size_px: Option<f32>,
    /// Override the corner radius, in px. Default `None` uses the sheet's pill
    /// radius; set e.g. `12.0` for a rounded-square tool button.
    pub radius: Option<f32>,
    /// Override the icon/glyph color. Default `None` uses the tone × variant
    /// foreground. Useful when the resting color should be neutral (ink) while
    /// the `selected` accent fill still supplies its own contrasting text — pass
    /// the ink color only on the un-selected instance.
    pub color: Option<Color>,
    /// When `Some`, fills the given `Ref<PressableHandle>` on mount. Useful for
    /// anchoring an `Overlay`/`Popover` to this button. A `Ref` fills exactly
    /// once; IconButton always snapshots its child structure at build, so the
    /// bind is unconditional (no `switch` rebuild that could re-fill it).
    pub bind_to: Option<Ref<PressableHandle>>,
    /// Accessible name: what a screen reader announces and what a robot /
    /// e2e test finds the button by (`find_element(label = …)`). A glyph or
    /// icon has no text to derive one from, so every IconButton should get
    /// a `label` or a `tooltip`. Default `None` falls back to `tooltip`.
    #[prop(static)]
    pub label: Option<String>,
    /// Hint shown in a [`Tooltip`](crate::components::tooltip::Tooltip)
    /// around the button (hover on desktop, long-press on touch), and the
    /// accessible name when `label` is `None`. A Tooltip wrapped around an
    /// IconButton from outside can't name it: the button is built before
    /// the Tooltip runs. Default `None` (no tooltip).
    #[prop(static)]
    pub tooltip: Option<String>,
}

impl Default for IconButtonProps {
    fn default() -> Self {
        Self {
            glyph: Reactive::Static(String::new()),
            icon: Reactive::Static(None),
            on_click: Rc::new(|| {}),
            tone: tone::Neutral.into(),
            variant: Reactive::Static(IconButtonVariant::default()),
            size: Reactive::Static(IconButtonSize::default()),
            disabled: Reactive::Static(false),
            selected: Reactive::Static(false),
            size_px: Reactive::Static(None),
            radius: Reactive::Static(None),
            color: Reactive::Static(None),
            bind_to: None,
            label: None,
            tooltip: None,
            styles: Default::default(),
        }
    }
}

/// Renders a square, single-glyph clickable styled by the tone × variant
/// × size axes of the installed IconButton sheet.
#[component]
pub fn IconButton(props: &IconButtonProps) -> Element {
    let on_click = props.on_click.clone();

    // A live `icon` whose presence doesn't change (one vector glyph swapped
    // for another — a light/dark toggle's sun ↔ moon) follows in place
    // through the icon primitive's `.data()` setter, as `Button` does.
    //
    // TODO(reactive-sweep): the rest of the child structure is snapshotted
    // at build. Which child is rendered (vector `icon` vs text `glyph`) and
    // the icon's pinned square are STRUCTURAL: flipping `icon` between
    // `Some` and `None`, or a live `glyph`/`size`/`size_px`, needs a
    // `switch`/`when` wrapper, not an in-place sink.
    let icon_data = props.icon.get();
    let glyph = props.glyph.get();
    let size_snapshot = props.size.get();
    let size_px_snapshot = props.size_px.get();

    let styles = props.styles.clone();
    let values = with_tone(
        IconButtonStyles::values()
            .set(props.variant.clone())
            .set(props.size.clone())
            .set(props.selected.clone().map(IconButtonSelected::from)),
        &props.tone,
    );
    // The author's continuous overrides — a custom side length, a corner
    // radius, a tint — ride the INLINE layer (out of the cache identity),
    // so the square still premints whatever the values.
    let custom = |size_px: Option<f32>, radius: Option<f32>, color: Option<Color>| {
        let mut rules = StyleRules::default();
        if let Some(px) = size_px {
            rules.width = Some(Tokenized::Literal(Length::Px(px)));
            rules.height = Some(Tokenized::Literal(Length::Px(px)));
        }
        if let Some(r) = radius {
            let r = Tokenized::Literal(Length::Px(r));
            rules.border_top_left_radius = Some(r.clone());
            rules.border_top_right_radius = Some(r.clone());
            rules.border_bottom_left_radius = Some(r.clone());
            rules.border_bottom_right_radius = Some(r);
        }
        if let Some(c) = color {
            rules.color = Some(Tokenized::Literal(c));
        }
        rules
    };
    let make_style = {
        let values = values.clone();
        let (size_px, radius, color) = (props.size_px.clone(), props.radius.clone(), props.color.clone());
        let styles = styles.clone();
        move || {
            let style = IconButtonStyles::root().values(&values);
            let style = match (size_px.clone(), radius.clone(), color.clone()) {
                (Reactive::Static(None), Reactive::Static(None), Reactive::Static(None)) => style,
                (Reactive::Static(s), Reactive::Static(r), Reactive::Static(c)) => style.inline(custom(s, r, c)),
                (s, r, c) => style.inline_with(move || custom(s.get(), r.get(), c.get())),
            };
            style.overridden(styles.root.as_ref())
        }
    };
    let style_is_reactive = make_style().is_reactive();

    // The container's resolved foreground, read for the vector icon's tint.
    //
    // THE ICON CARRIES IT ON ITS OWN NODE, because nothing hands it down:
    // native backends have no style cascade, so an `icon` with no explicit
    // color resolves a SYSTEM constant (`UIColor.labelColor` on iOS) that
    // bears no relation to whatever its ancestor is painted. One resolve
    // answers every input: the tone × variant arm, the `selected` accent
    // fill, and an author `color` (on the inline layer, which `resolve`
    // folds in).
    //
    // WEB PREMINT is the exception, and why this is gated. When the
    // container attaches a preminted class its CSS carries the fill's
    // `color`, so the icon inherits it as `currentColor` — which also
    // tracks `:hover` — and under `--premint-only` the read-back itself is
    // the panic the stripped rule closure names.
    let fg = {
        let make_style = make_style.clone();
        move || {
            let app = make_style().into_style_application();
            if app.attaches_preminted() {
                None
            } else {
                resolve_style(&app).color.clone()
            }
        }
    };

    // A vector `icon` wins over the text `glyph`. Sized per step by the
    // `icon` slot; a custom `size_px` scales it to half the side.
    let icon_style = {
        let style = IconButtonStyles::icon().values(&values);
        let style = match size_px_snapshot {
            Some(s) => {
                let half = Tokenized::Literal(Length::Px((s * 0.5).round()));
                style.inline(StyleRules { width: Some(half.clone()), height: Some(half), ..Default::default() })
            }
            None => style,
        };
        style.overridden(styles.icon.as_ref())
    };
    let _ = size_snapshot;
    let child = match icon_data {
        Some(data) => {
            let mut el = icon(data).with_style(icon_style);
            if !props.icon.is_static() {
                // Presence is fixed for this build (see above), so the
                // fallback only covers a live slot that turned `None`.
                let slot = props.icon.clone();
                el = el.data(move || slot.get().unwrap_or(crate::components::button::EMPTY_ICON_DATA));
            }
            if style_is_reactive {
                // A live style axis re-resolves per read, so the tint tracks
                // the container in place instead of freezing this build's.
                let fg_live = fg.clone();
                match fg() {
                    Some(_) => el
                        .color(move || {
                            fg_live()
                                .map(|c| c.resolve())
                                .unwrap_or_else(|| Color("#000000".into()))
                        })
                        .into_element(),
                    None => el.into_element(),
                }
            } else {
                match fg() {
                    // A reactive read even on the static path: `resolve()`
                    // re-runs on a theme swap, so the tint tracks the token.
                    Some(c) => el.color(move || c.resolve()).into_element(),
                    None => el.into_element(),
                }
            }
        }
        // The glyph's foreground is its own slot's arms (plus the author's
        // `color` on its inline layer), so it premints and colors native
        // text with no read-back.
        None => {
            let glyph_style = IconButtonStyles::glyph().values(&values);
            let glyph_style = match props.color.clone() {
                Reactive::Static(None) => glyph_style,
                Reactive::Static(c) => glyph_style.inline(custom(None, None, c)),
                live => glyph_style.inline_with(move || custom(None, None, live.get())),
            };
            text(glyph).with_style(glyph_style.overridden(styles.glyph.as_ref())).into_element()
        }
    };
    let bound = runtime_core::pressable(vec![child], move || (on_click)()).with_style(make_style());
    let mut bound = bound;

    // `disabled` routes reactively: `.disabled()` accepts a `Fn() -> bool`, so
    // reading `.get()` inside subscribes the disabled state to a live signal
    // (it dims + blocks the press in place). A static value reads once.
    if !props.disabled.is_static() {
        let disabled = props.disabled.clone();
        bound = bound.disabled(move || disabled.get());
    } else if props.disabled.get() {
        bound = bound.disabled(true);
    }

    // `bind_to` fills its `Ref` on mount. IconButton's child structure is
    // always built statically (no reactive `switch` — see the TODO above), so
    // the bind is unconditional and fills exactly once.
    if let Some(r) = props.bind_to {
        bound = bound.bind(r);
    }
    if let Some(name) = props.label.clone().or_else(|| props.tooltip.clone()) {
        bound = bound.a11y_label(name);
    }
    let button = bound.into_element();
    match props.tooltip.clone() {
        Some(text) => ui! {
            Tooltip(text = text) {
                button
            }
        },
        None => button,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{FillRule, VariantEnum};

    fn theme() {
        install_idea_theme(light_theme());
    }

    const TRASH: IconData = IconData {
        view_box: (24, 24),
        paths: &["M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6"],
        fill_rule: FillRule::NonZero,
        filled: false,
    };

    fn only_child(el: Element) -> Element {
        match classify(el) {
            P::Pressable { mut children, .. } => {
                assert_eq!(children.len(), 1, "IconButton has a single child");
                children.remove(0)
            }
            _ => panic!("IconButton renders a Pressable"),
        }
    }

    fn reset_styles() {
        runtime_core::component_styles::__reset::<IconButtonStyles>();
    }

    /// The square's style application (the IconButton's pressable).
    fn root_app(el: Element) -> runtime_core::StyleApplication {
        match classify(el) {
            P::Pressable { style, .. } => style.expect("the square is styled").application(),
            _ => panic!("IconButton renders a Pressable"),
        }
    }

    /// `tone_variant` gives an app value Button's treatment: the recipe's
    /// surface on the square, the hover / press feedback, and the glyph's
    /// foreground — and a `selected` button of that value still shows the
    /// tone's accent fill.
    #[test]
    fn tone_variant_adds_surface_feedback_and_glyph_color() {
        with_test_world(|| {
            reset_styles();
            theme();
            const LIFTED: IconButtonVariant = IconButtonVariant::define("lifted");
            IconButtonStyles::restyle(|s| s.tone_variant(LIFTED, &variant::Gradient));
            let props = |selected: bool| IconButtonProps {
                glyph: Reactive::Static("+".into()),
                tone: Reactive::Static(ToneRef::default()),
                variant: Reactive::Static(LIFTED),
                selected: Reactive::Static(selected),
                ..Default::default()
            };

            let app = root_app(IconButton(&props(false)));
            let rest = resolve_style(&app);
            assert!(rest.background_gradient.is_some(), "the square paints the recipe's gradient");
            let hovered = resolve_style(&app.clone().with("__state_hovered", "on"));
            let pressed = resolve_style(&app.with("__state_pressed", "on"));
            assert_ne!(*hovered, *rest, "hover feedback is drawn for the new value");
            assert_ne!(*pressed, *rest, "press feedback is drawn for the new value");

            let glyph = match classify(only_child(IconButton(&props(false)))) {
                P::Text { style, .. } => style.expect("the glyph is styled").resolve(),
                _ => panic!("expected the text glyph"),
            };
            assert!(glyph.color.is_some(), "the glyph carries the recipe's foreground");
            assert_eq!(glyph.color, rest.color, "glyph and square agree on the foreground");

            let selected = resolve_style(&root_app(IconButton(&props(true))));
            let accent = with_idea_theme(|t| ToneRef::default().0.fill_bg(t));
            assert_eq!(selected.background, Some(accent), "selected keeps the accent fill over the new value");
            reset_styles();
        });
    }

    fn pressable_name(el: Element) -> Option<String> {
        match classify(el) {
            P::Pressable { accessibility, .. } => accessibility.label,
            _ => panic!("expected the IconButton pressable"),
        }
    }

    /// An IconButton has no text to derive a name from, so screen readers
    /// and robot/e2e lookups by name found nothing. `label` names it.
    #[test]
    fn regression_icon_button_label_is_the_accessible_name() {
        with_test_world(|| {
            theme();
            let props = IconButtonProps {
                icon: Reactive::Static(Some(TRASH)),
                label: Some("Delete row".into()),
                ..Default::default()
            };
            assert_eq!(pressable_name(IconButton(&props)).as_deref(), Some("Delete row"));
            let bare = IconButtonProps { icon: Reactive::Static(Some(TRASH)), ..Default::default() };
            assert_eq!(pressable_name(IconButton(&bare)), None, "no label, no tooltip → unnamed");
        });
    }

    /// `tooltip` wraps the button in a Tooltip AND names it with the same
    /// text — a Tooltip wrapped around it from outside can't, since the
    /// button is built first. An explicit `label` wins over the tooltip.
    #[test]
    fn tooltip_wraps_the_button_and_names_it() {
        with_test_world(|| {
            theme();
            let built = |label: Option<&str>| {
                IconButton(&IconButtonProps {
                    icon: Reactive::Static(Some(TRASH)),
                    tooltip: Some("Delete".into()),
                    label: label.map(Into::into),
                    ..Default::default()
                })
            };
            let button_in = |el: Element| -> Element {
                let P::Fragment { mut children } = classify(el) else {
                    panic!("a tooltip IconButton is the Tooltip's [anchor, bubble] fragment");
                };
                let P::View { mut children, on_hover, .. } = classify(children.remove(0)) else {
                    panic!("the Tooltip anchor view");
                };
                assert!(on_hover, "the anchor shows the bubble on hover");
                children.remove(0)
            };
            assert_eq!(pressable_name(button_in(built(None))).as_deref(), Some("Delete"));
            assert_eq!(
                pressable_name(button_in(built(Some("Remove item")))).as_deref(),
                Some("Remove item"),
                "an explicit label wins over the tooltip text"
            );
        });
    }

    // D5: passing `icon` (IconData) renders a vector icon instead of the
    // text glyph — so Lucide SVGs work where only a String glyph did.
    #[test]
    fn icon_data_renders_an_icon_child() {
        with_test_world(|| {
            theme();
            let props = IconButtonProps {
                icon: Reactive::Static(Some(TRASH)),
                glyph: Reactive::Static("×".into()), // present but overridden by `icon`
                ..Default::default()
            };
            assert!(
                matches!(classify(only_child(IconButton(&props))), P::Icon { .. }),
                "an `icon` prop must render an Icon child (it wins over glyph)"
            );
    });
    }

    // The glyph path still works when no `icon` is given.
    #[test]
    fn glyph_falls_back_to_text() {
        with_test_world(|| {
            theme();
            let props = IconButtonProps {
                glyph: Reactive::Static("×".into()),
                ..Default::default()
            };
            assert!(
                matches!(classify(only_child(IconButton(&props))), P::Text { .. }),
                "with no `icon`, the glyph renders as text"
            );
    });
    }

    // `bind_to` wires a `ref_fill` closure onto the root Pressable so the
    // framework fills the caller's `Ref<PressableHandle>` on mount (for
    // anchoring an Overlay/Popover to the icon button). Absent `bind_to`,
    // no fill is installed.
    #[test]
    fn bind_to_installs_ref_fill_on_pressable() {
        with_test_world(|| {
            theme();
            let anchor: Ref<PressableHandle> = Ref::new();
            let bound = IconButtonProps {
                bind_to: Some(anchor),
                ..Default::default()
            };
            match classify(IconButton(&bound)) {
                P::Pressable { ref_fill, .. } => {
                    assert!(ref_fill, "bind_to must install a ref_fill on the Pressable");
                }
                _ => panic!("IconButton renders a Pressable"),
            }

            match classify(IconButton(&IconButtonProps::default())) {
                P::Pressable { ref_fill, .. } => {
                    assert!(!ref_fill, "without bind_to there is no ref_fill");
                }
                _ => panic!("IconButton renders a Pressable"),
            }
    });
    }

    #[test]
    fn icon_size_scales_with_button_size() {
        with_test_world(|| {
            theme();
            for (size, px) in [(IconButtonSize::Sm, 16.0), (IconButtonSize::Md, 18.0), (IconButtonSize::Lg, 24.0)] {
                let rules = runtime_core::resolve_style(&IconButtonStyles::icon().set(size).into_style_application());
                assert_eq!(rules.width, Some(Tokenized::Literal(Length::Px(px))), "{}", size.as_variant_str());
            }
        });
    }

    // Regression: an IconButton must center on its parent's cross axis
    // (`align_self: Center`) so a row of mixed-size icon buttons centers
    // instead of top-aligning under the parent's default `align-items:
    // stretch` (the IconButton "Sizes" row report).
    #[test]
    fn icon_button_centers_on_cross_axis() {
        with_test_world(|| {
            theme();
            let app = match classify(IconButton(&IconButtonProps::default())) {
                P::Pressable { style, .. } => match style.expect("IconButton attaches a style") {
                    TStyle::App(a) => a,
                    _ => panic!("IconButton uses a static style source"),
                },
                _ => panic!("IconButton renders a Pressable"),
            };
            assert_eq!(
                runtime_core::resolve_style(&app).align_self,
                Some(AlignSelf::Center),
                "an IconButton centers on the cross axis instead of stretching/top-aligning"
            );
    });
    }

    // Regression: nothing hands a color down to the icon. Native backends
    // have no style cascade, so an `icon` with no explicit color resolves a
    // SYSTEM constant — `UIColor.labelColor` on iOS — which bears no
    // relation to the disc its ancestor painted. Only on web does CSS
    // `color` inherit into the glyph.
    //
    // Relying on inheritance, a Filled IconButton painted a near-black glyph
    // on its own near-black disc: measured on an iOS 26.5 simulator,
    // container (24, 24, 27) and glyph (0, 0, 0) — a control that is
    // present, sized and pressable, and invisible. `Button` already stamps
    // its leading/trailing icons for this reason; this asserts IconButton
    // does the same.
    #[test]
    fn regression_filled_icon_button_icon_carries_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = IconButtonProps {
                icon: Reactive::Static(Some(TRASH)),
                tone: Reactive::Static(ToneRef::default()), // Primary
                variant: Reactive::Static(IconButtonVariant::default()), // Filled
                ..Default::default()
            };
            match classify(only_child(IconButton(&props))) {
                P::Icon { color, .. } => {
                    let c = color
                        .expect("a filled IconButton's icon must carry an explicit color");
                    assert_eq!(
                        c.0.to_ascii_lowercase(),
                        "#ffffff",
                        "the icon tint is the intent text color, not the ambient label color"
                    );
                }
                _ => panic!("expected an Icon child"),
            }
    });
    }

    // The glyph sits in the same position as the vector icon and had the
    // same hole: `text(glyph)` carried no style at all, so on a backend
    // with no cascade it fell back to the theme's ink and a Filled button
    // drew a dark character on its dark disc. Less visible than the icon
    // case only because most call sites pass `icon` — the close-button
    // recipe (`glyph = "×"`) is the one that ships it.
    #[test]
    fn regression_filled_icon_button_glyph_carries_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = IconButtonProps {
                glyph: Reactive::Static("×".into()),
                tone: Reactive::Static(ToneRef::default()), // Primary
                variant: Reactive::Static(IconButtonVariant::default()), // Filled
                ..Default::default()
            };
            match classify(only_child(IconButton(&props))) {
                P::Text { style, .. } => {
                    let app = match style.expect("a filled IconButton's glyph must carry a style") {
                        TStyle::App(a) => a,
                        _ => panic!("the glyph uses a static style source"),
                    };
                    let c = runtime_core::resolve_style(&app)
                        .color
                        .clone()
                        .expect("the glyph's style must carry a color");
                    assert_eq!(
                        c.resolve().0.to_ascii_lowercase(),
                        "#ffffff",
                        "the glyph tint is the intent text color, not the theme's ink"
                    );
                }
                _ => panic!("expected a Text child"),
            }
    });
    }

    // The author's `color` needs no special case in the icon path: it is
    // applied to the container as an `override_color`, so the single
    // resolve that tints the icon picks it up. Asserted because that is a
    // load-bearing property of the design rather than an accident — an
    // icon path that read the tone × variant arm directly would silently
    // ignore the override.
    #[test]
    fn an_explicit_color_wins_over_the_tone_variant_foreground() {
        with_test_world(|| {
            theme();
            let props = IconButtonProps {
                icon: Reactive::Static(Some(TRASH)),
                tone: Reactive::Static(ToneRef::default()),
                variant: Reactive::Static(IconButtonVariant::default()),
                color: Reactive::Static(Some(Color("#ff00ff".into()))),
                ..Default::default()
            };
            match classify(only_child(IconButton(&props))) {
                P::Icon { color, .. } => {
                    let c = color.expect("an explicit color must reach the icon");
                    assert_eq!(c.0.to_ascii_lowercase(), "#ff00ff");
                }
                _ => panic!("expected an Icon child"),
            }
    });
    }
}

//! `Chip` — a selectable pill toggle.
//!
//! Where [`Tag`](super::tag::Tag) is a status pill, optionally removable,
//! `Chip` is the *selectable* member of the family: tapping it reports a select, and its `selected`
//! flag drives a distinct on/off appearance. It's the everyday building
//! block for filter rows, choice chips, and multi-select toggles.
//!
//! ```ignore
//! let on = signal(false);
//! let on_select: Rc<dyn Fn()> = Rc::new(move || on.set(!on.get()));
//! ui! {
//!     Chip(
//!         label = "Rust",
//!         selected = on.get(),
//!         on_select = Some(on_select.clone()),
//!         tone = tone::Primary,
//!     )
//! }
//! ```
//!
//! Like every selection control in idea-ui, `Chip` is **controlled**: it
//! never owns the boolean. The host keeps the selected state (a signal, a
//! set membership test, a route flag, …), passes it in as `selected`, and
//! flips it in `on_select`. That keeps a row of chips trivially
//! single-select *or* multi-select depending on what the host does in the
//! callback.
//!
//! ## Selected vs unselected appearance
//! The `selected` state paints with the caller's chosen `variant`; the
//! unselected state drops to the quieter `Ghost` variant of the same tone,
//! so a row of chips reads as "one (or some) lit up, the rest muted". Pass
//! an explicit `variant` to set the *selected* look (e.g. `Filled` for a
//! stronger highlight).
//!
//! ## Size
//! `size` ([`ChipSize`] — `Sm` / `Md` / `Lg`, default `Md`; a
//! `ControlSize` converts) picks the pill's padding (`root`) and the
//! label's font (`label`). `Md` is a Tag's geometry.
//!
//! ## Styling
//! [`ChipStyles`]: `root` (the pill — tone × variant compounds over the
//! categorization tones, `size`, the `interactive` cursor and a themed
//! focus ring), `label` (the fill's foreground on its own node), `icon` (a
//! leading icon) and `adornment` (a leading [`Adornment::Button`]). `leading`
//! puts an [`Adornment`] before the label, like `Tag` and `SegmentOption`.

use std::rc::Rc;

use runtime_core::{
    component, component_styles, pressable, recipe, ui, view, AlignItems, Element,
    FlexDirection, FontWeight, IdealystSchema, IntoElement, Length, Reactive, SlotRestyle, StyleRules,
    TextAlign,
};

use idea_theme::extensible::{categorization_tones, tone, ToneRef};

use crate::components::field::Adornment;
use crate::components::tone_slots::{
    focus_ring_compounds, foreground_arms, pill_adornment, recipe_compounds, surface_arms,
};
use crate::components::ControlSize;
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Chip`].
    pub Chip<IdeaThemeRef> {
        /// The selected look — a tone-aware recipe per value (unselected
        /// chips draw `ghost`).
        axis variant { filled, #[default] soft, outlined, ghost }

        /// The pill.
        slot root {
            extend(root_surfaces)
            base(t) {
                // Hug by size, not an `align_self`: it stops a flex parent's
                // `align-items: stretch` growing the pill and leaves the parent's
                // alignment in charge of where it goes.
                width: Length::FitContent,
                height: Length::FitContent,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.xs(),
                border_radius: t.radius.pill(),
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.4,
                text_align: TextAlign::Center,
            }
            /// The pill's inset. The vertical padding is a literal: the
            /// pill's height is set by its label's line box, and the
            /// theme's smallest spacing step (`xs`, 4) already doubles
            /// `md`'s 2px.
            variant size {
                sm(t) { padding_vertical: Length::Px(1.0), padding_horizontal: t.spacing.xs() }
                #[default]
                md(t) { padding_vertical: Length::Px(2.0), padding_horizontal: t.spacing.sm() }
                lg(t) { padding_vertical: Length::Px(4.0), padding_horizontal: t.spacing.md() }
            }
            /// A chip with an `on_select` shows the pointer (the 1px border
            /// it reserves for the focus ring is per `(tone, variant)` — see
            /// `root_surfaces`).
            variant interactive {
                #[default]
                off(_t) {}
                on(_t) { cursor: runtime_core::Cursor::Pointer }
            }
        }

        /// The label, in the fill's foreground, at the size's type.
        slot label {
            extend(label_foregrounds)
            base(_t) {
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.3,
            }
            variant size {
                sm(t) { font_size: t.typography.caption_size() }
                #[default]
                md(t) { font_size: t.typography.body_sm_size() }
                lg(t) { font_size: t.typography.body_size() }
            }
        }

        /// A leading [`Adornment::Icon`] / [`Adornment::Button`] glyph: a fixed
        /// square (icons have no intrinsic size) in the fill's foreground,
        /// scaled with the label.
        slot icon {
            extend(label_foregrounds)
            base(_t) { flex_shrink: 0.0 }
            variant size {
                sm(_t) { width: 12.0, height: 12.0 }
                #[default]
                md(_t) { width: 14.0, height: 14.0 }
                lg(_t) { width: 16.0, height: 16.0 }
            }
        }

        /// A leading [`Adornment::Button`]'s hit box: no fill of its own, the
        /// glyph centered.
        slot adornment {
            base(_t) {
                align_items: AlignItems::Center,
                justify_content: runtime_core::JustifyContent::Center,
                cursor: runtime_core::Cursor::Pointer,
            }
        }
    }
}

fn root_surfaces(s: &mut SlotRestyle<'_, ChipStyles>) {
    let tones = categorization_tones();
    surface_arms(s, &tones);
    // An interactive chip reserves a 1px border for its focus ring: on a
    // content-sized pill the ring would otherwise add 2px per axis on
    // focus, and since a press focuses, pressing a chip nudged its
    // neighbours. Width only — an outlined chip keeps its stroke colour, a
    // borderless one the transparent colour its recipe gave it. A compound,
    // because the recipe's own border (a compound) must not win over it.
    recipe_compounds(s, &tones, &[("interactive", "on")], |_, _| StyleRules {
        border_top_width: Some(runtime_core::Tokenized::Literal(1.0)),
        border_right_width: Some(runtime_core::Tokenized::Literal(1.0)),
        border_bottom_width: Some(runtime_core::Tokenized::Literal(1.0)),
        border_left_width: Some(runtime_core::Tokenized::Literal(1.0)),
        ..Default::default()
    });
    // The themed focus ring (a pressable chip receives FOCUSED), declared
    // after the reservation so it wins over it.
    focus_ring_compounds(s, &tones, &[], 1.0);
}

fn label_foregrounds(s: &mut SlotRestyle<'_, ChipStyles>) {
    foreground_arms(s, &categorization_tones());
}

marker_into_axis!(ChipVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

/// The control-wide size scale converts into Chip's own size axis.
impl From<ControlSize> for ChipSize {
    fn from(s: ControlSize) -> Self {
        ChipSize::define(s.as_variant_str())
    }
}

impl From<ControlSize> for Reactive<ChipSize> {
    fn from(s: ControlSize) -> Self {
        Reactive::Static(s.into())
    }
}

// Reactive-by-default: `#[props]` wraps `selected`/`tone`/`variant`/`size` →
// `Reactive<…>`; `label` is already reactive, and `on_select` (an
// `Rc<dyn Fn()>` handler) is auto-skipped. Bare markers (`tone =
// tone::Primary`) coerce to `Reactive<ToneRef>` via the marker's generated
// `From`. The style-driving props route into the container and label style
// sinks, read `.get()` INSIDE so the apply-style Effect subscribes to
// whichever are live.
#[runtime_core::props(styles = ChipStyles)]
#[derive(IdealystSchema)]
pub struct ChipProps {
    /// Chip text. `Reactive<String>` — static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    #[prop(example = "Design")]
    pub label: Reactive<String>,
    /// Whether this chip is currently selected. Controlled by the host —
    /// the chip never owns it. Drives the lit/muted appearance.
    pub selected: bool,
    /// Fires when the chip is tapped. The host flips its `selected` source
    /// here (`move || on.set(!on.get())` for a toggle, or a set
    /// insert/remove for multi-select). When unset, the chip is inert
    /// (renders, but a tap does nothing) per idea-ui's §9.6 optional
    /// callback rule.
    pub on_select: Option<Rc<dyn Fn()>>,
    /// Semantic color palette (Primary, Neutral, Success, …). Default
    /// Neutral. Applies to both states (the selected state is lit, the
    /// unselected state is the muted Ghost of the same tone).
    pub tone: ToneRef,
    /// Surface treatment used for the **selected** state (Soft, Filled,
    /// Outlined, …, or an app value). Default Soft. The unselected state
    /// always uses the quieter Ghost variant of the same tone.
    pub variant: ChipVariant,
    /// Size scale (Sm, Md, Lg). Default Md. Picks the pill's padding and
    /// the label's font size (Sm: caption type, tight inset; Md: body-sm
    /// type, the Tag's geometry; Lg: body type, roomier inset).
    pub size: ChipSize,
    /// Drawn before the label: a status dot (`Adornment::element(|| ui! { … })`)
    /// or an icon (`Adornment::Icon(MAP)`, in the fill's foreground). Default
    /// [`Adornment::None`]. An element-builder, so it stays bare: reactivity
    /// lives inside the built tree.
    #[prop(static)]
    pub leading: Adornment,
    /// Optional robot/E2E test id, forwarded to the pill — the pressable
    /// when `on_select` is set, the plain view otherwise. Inert outside a
    /// robot build.
    pub test_id: Option<&'static str>,
}

impl Default for ChipProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            selected: Reactive::Static(false),
            on_select: None,
            tone: tone::Neutral.into(),
            variant: Reactive::Static(ChipVariant::default()),
            size: Reactive::Static(ChipSize::default()),
            leading: Adornment::None,
            test_id: None,
            styles: Default::default(),
        }
    }
}

/// Renders a selectable pill: a pressable whose tone × variant appearance
/// switches between a lit (`selected`) and a muted (unselected) state,
/// reporting taps via `on_select`. The host owns the selected boolean.
/// Without an `on_select` the chip is a plain view — nothing to press, so
/// nothing swallows the tap (§9.6).
#[component]
pub fn Chip(props: &ChipProps) -> Element {
    let label = props.label.clone();
    let on_select = props.on_select.clone();
    let clickable = on_select.is_some();

    // Selected → the caller's variant (lit); unselected → Ghost (muted) of
    // the same tone. The pill and the label draw the same keys, so the
    // label carries the tone's foreground itself (native text inherits no
    // color from its box). The pointer rides the `interactive` axis, so
    // `clickable` is part of the resolution identity: two chips with the
    // same tone + variant but different `on_select` never share a cursor.
    let shown_variant = {
        let selected = props.selected.clone();
        let variant = props.variant.clone();
        match (selected, variant) {
            (Reactive::Static(sel), Reactive::Static(v)) => {
                Reactive::Static(if sel { v } else { ChipVariant::Ghost })
            }
            (sel, v) => Reactive::Dynamic(std::rc::Rc::new(move || if sel.get() { v.get() } else { ChipVariant::Ghost })),
        }
    };
    let values = ChipStyles::values().set(shown_variant).set(props.size.clone());
    let values = crate::components::tone_slots::with_tone(values, &props.tone);
    let style = ChipStyles::root()
        .values(&values)
        .set(ChipInteractive::from(clickable))
        .overridden(props.styles.root.as_ref());
    let label_style = ChipStyles::label().values(&values).overridden(props.styles.label.as_ref());
    let label_el: Element = ui! { text(style = label_style) { label } };
    let mut children = {
        let icon_values = values.clone();
        let icon_styles = props.styles.clone();
        let button_styles = props.styles.clone();
        pill_adornment(
            &props.leading,
            &(Rc::new(move || ChipStyles::icon().values(&icon_values).overridden(icon_styles.icon.as_ref())) as Rc<dyn Fn() -> _>),
            &(Rc::new(move || ChipStyles::adornment().overridden(button_styles.adornment.as_ref())) as Rc<dyn Fn() -> _>),
        )
    };
    children.push(label_el);

    // §9.6: a pressable only when the host supplied a handler. An inert chip
    // is a plain view — a no-op pressable would swallow the tap and block
    // hit-test fall-through to whatever sits under it.
    match on_select {
        Some(cb) => {
            with_test_id!(pressable(children, move || (cb)()), props.test_id).with_style(style).into_element()
        }
        None => with_test_id!(view(children), props.test_id).with_style(style).into_element(),
    }
}

recipe!(
    Chip,
    /// A selectable filter chip. The host owns the selected state (here a
    /// `Signal<bool>`); `on_select` flips it. Drop several in a row for a
    /// filter bar — make it multi-select by toggling each independently,
    /// or single-select by clearing the others in the callback.
    pub fn chip_filter() -> ::runtime_core::Element {
        use crate::components::chip::Chip;
        use crate::{tone, variant};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let on = signal(false);
        let on_select: Rc<dyn Fn()> = Rc::new(move || on.set(!on.get()));
        ui! {
            Chip(
                label = "Rust",
                selected = on.get(),
                on_select = Some(on_select.clone()),
                tone = tone::Primary,
                variant = variant::Soft,
            )
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;

    /// The leading adornment sits before the label on a pressable chip and an
    /// inert one, and an icon recolors when the chip's lit state flips.
    #[test]
    fn leading_adornment_comes_first_and_its_icon_follows_selected() {
        with_test_world(|| {
            use idea_theme::theme::{install_idea_theme, light_theme};
            install_idea_theme(light_theme());
            let on_select: Option<std::rc::Rc<dyn Fn()>> = Some(std::rc::Rc::new(|| {}));
            let selected = runtime_core::signal(true);
            let build = |on_select: Option<std::rc::Rc<dyn Fn()>>| {
                Chip(&ChipProps {
                    label: Reactive::Static("Rust".to_string()),
                    selected: Reactive::Dynamic(std::rc::Rc::new(move || selected.get())),
                    on_select,
                    variant: Reactive::Static(ChipVariant::Filled),
                    tone: tone::Primary.into(),
                    leading: Adornment::Icon(crate::components::icon::EMPTY_ICON),
                    ..Default::default()
                })
            };
            let icon_color = |el: Element| {
                let children = match classify(el) {
                    P::Pressable { children, .. } | P::View { children, .. } => children,
                    _ => panic!("a chip is a pressable or a view"),
                };
                assert_eq!(children.len(), 2, "[icon, label]");
                let mut children = children.into_iter();
                let color = match classify(children.next().unwrap()) {
                    P::Icon { color, .. } => color.expect("the glyph is tinted"),
                    _ => panic!("the leading icon comes first"),
                };
                assert!(matches!(classify(children.next().unwrap()), P::Text { .. }));
                color
            };
            let lit = icon_color(build(on_select.clone()));
            assert_eq!(icon_color(build(None)), lit, "inert and pressable chips tint alike");
            selected.set(false);
            idea_theme::testing::commit();
            assert_ne!(icon_color(build(on_select)), lit, "a muted chip's glyph is not the fill's foreground");
        });
    }

    #[test]
    fn defaults_are_unselected_and_inert() {
        with_test_world(|| {
            let p = ChipProps::default();
            assert!(!p.selected.get());
            assert!(p.on_select.is_none());
            assert_eq!(p.size.get(), ChipSize::Md);
    });
    }

    /// Two chips with the SAME tone+variant but different `on_select` must
    /// resolve to different cursors.
    ///
    /// The pointer used to ride `with_computed("chip-box", …)`, whose closure
    /// captured `clickable` while the KEY was the constant `"chip-box"`. The
    /// resolution cache is keyed on
    /// `(sheet, variants, computed_key, overrides)`, so both chips hashed
    /// identically and shared one resolved `StyleRules` — whichever resolved
    /// first decided the cursor for both. Resolving both in ONE world is what
    /// exercises that shared cache; the fix moves `clickable` onto the sheet's
    /// `interactive` variant, which is part of the identity by construction.
    #[test]
    fn regression_clickable_and_inert_chips_do_not_share_a_cursor() {
        with_test_world(|| {
            use idea_theme::theme::{install_idea_theme, light_theme};
            install_idea_theme(light_theme());

            let cursor_of = |on_select: Option<std::rc::Rc<dyn Fn()>>| {
                let el = Chip(&ChipProps {
                    label: Reactive::Static("Tag".to_string()),
                    on_select,
                    ..Default::default()
                });
                // A clickable chip is a pressable, an inert one a view (§9.6);
                // both resolve the same tag sheet.
                match classify(el) {
                    P::Pressable { style, .. } | P::View { style, .. } => {
                        style.expect("chip carries a style").resolve().cursor.clone()
                    }
                    _ => panic!("a chip builds a pressable or a view"),
                }
            };

            // Inert FIRST so a shared cache entry would be seeded without a
            // cursor — the ordering that exposes the shared-cache bug.
            let inert = cursor_of(None);
            let clickable = cursor_of(Some(std::rc::Rc::new(|| {})));

            assert_ne!(inert, Some(runtime_core::Cursor::Pointer));
            assert_eq!(clickable, Some(runtime_core::Cursor::Pointer));
        });
    }

    /// A chip with no `on_select` still renders (it just doesn't react to
    /// taps) — never panics, per §9.6. (Resolving the chip's appearance
    /// reads the installed Tag sheet, so install the theme first like the
    /// other style-resolving component tests do.)
    #[test]
    fn inert_chip_renders_without_callback() {
        with_test_world(|| {
            use idea_theme::theme::{install_idea_theme, light_theme};
            install_idea_theme(light_theme());

            let el = Chip(&ChipProps {
                label: Reactive::Static("Tag".to_string()),
                ..Default::default()
            });
            assert!(matches!(classify(el), P::View { .. }));
    });
    }

    /// Regression (§9.6): a chip with no `on_select` used to build a
    /// pressable bound to a no-op closure, which consumes the tap and
    /// blocks hit-test fall-through on some backends. With nothing to
    /// press it must build a plain view.
    #[test]
    fn regression_inert_chip_is_not_a_noop_pressable() {
        with_test_world(|| {
            use idea_theme::theme::{install_idea_theme, light_theme};
            install_idea_theme(light_theme());
            let inert = Chip(&ChipProps {
                label: Reactive::Static("Tag".to_string()),
                ..Default::default()
            });
            assert!(
                !matches!(classify(inert), P::Pressable { .. }),
                "an inert chip must not be a pressable"
            );
            let live = Chip(&ChipProps {
                label: Reactive::Static("Tag".to_string()),
                on_select: Some(std::rc::Rc::new(|| {})),
                ..Default::default()
            });
            assert!(matches!(classify(live), P::Pressable { .. }));
        });
    }

    /// `(container padding-left, container padding-top, label font-size)`
    /// for a clickable chip of `size`.
    fn chip_metrics(size: ControlSize) -> (f32, f32, f32) {
        use runtime_core::Length;
        let el = Chip(&ChipProps {
            label: Reactive::Static("Tag".to_string()),
            on_select: Some(std::rc::Rc::new(|| {})),
            size: size.into(),
            ..Default::default()
        });
        let (children, style) = match classify(el) {
            P::Pressable { children, style, .. } => (children, style),
            _ => panic!("a clickable chip builds a pressable"),
        };
        let rules = style.expect("chip carries a style").resolve();
        let px = |l: Option<Length>| match l {
            Some(Length::Px(v)) => v,
            other => panic!("expected px, got {other:?}"),
        };
        let pad_left = px(rules.padding_left.as_ref().map(|t| t.resolve()));
        let pad_top = px(rules.padding_top.as_ref().map(|t| t.resolve()));
        let label_rules = match classify(children.into_iter().next().expect("a label")) {
            P::Text { style, .. } => style.expect("label carries a style").resolve(),
            _ => panic!("chip's child is its label text"),
        };
        let font = px(label_rules.font_size.as_ref().map(|t| t.resolve()));
        (pad_left, pad_top, font)
    }

    /// Regression (Wave-41): `ChipProps::size` was documented but never
    /// read — every size rendered the Md pill. Each size must now resolve
    /// its own padding and label font, growing Sm → Md → Lg, with Md
    /// keeping the Tag's original geometry (8px inset, 13px body-sm type).
    #[test]
    fn regression_chip_size_prop_changes_padding_and_font() {
        with_test_world(|| {
            use idea_theme::theme::{install_idea_theme, light_theme};
            install_idea_theme(light_theme());
            let sm = chip_metrics(ControlSize::Sm);
            let md = chip_metrics(ControlSize::Md);
            let lg = chip_metrics(ControlSize::Lg);
            assert_eq!(md, (8.0, 2.0, 13.0), "Md is the Tag's pre-size-axis geometry");
            assert!(sm.0 < md.0 && md.0 < lg.0, "inset grows with size: {sm:?} {md:?} {lg:?}");
            assert!(sm.1 < md.1 && md.1 < lg.1, "vertical padding grows with size");
            assert!(sm.2 < md.2 && md.2 < lg.2, "label font grows with size");
        });
    }

    /// Regression: the chip label used the colorless `TagLabel` sheet, so on
    /// native backends (text inherits no color from its box) a lit chip's
    /// label kept the default text color instead of the tone's foreground.
    /// The label must carry the same appearance foreground as the fill.
    #[test]
    fn regression_chip_label_carries_the_tone_foreground() {
        with_test_world(|| {
            use idea_theme::theme::{install_idea_theme, light_theme};
            install_idea_theme(light_theme());
            let el = Chip(&ChipProps {
                label: Reactive::Static("Tag".to_string()),
                selected: Reactive::Static(true),
                on_select: Some(std::rc::Rc::new(|| {})),
                tone: tone::Danger.into(),
                variant: variant::Filled.into(),
                ..Default::default()
            });
            let (children, style) = match classify(el) {
                P::Pressable { children, style, .. } => (children, style),
                _ => panic!("a clickable chip builds a pressable"),
            };
            let fill_color = style.expect("style").resolve().color.clone();
            assert!(fill_color.is_some(), "a filled chip paints a foreground");
            let label_color = match classify(children.into_iter().next().unwrap()) {
                P::Text { style, .. } => style.expect("label style").resolve().color.clone(),
                _ => panic!("chip's child is its label text"),
            };
            assert_eq!(label_color, fill_color, "the label is painted in the fill's foreground");
        });
    }

    /// An interactive chip keeps its 1px focus-ring reservation and draws
    /// the ring on focus even over a borderless (Filled) recipe: the
    /// recipe's fill is a compound, which merges after single-axis arms
    /// and states, so the reservation and the ring are compounds too.
    #[test]
    fn interactive_chip_reserves_and_draws_its_focus_ring_over_a_filled_recipe() {
        with_test_world(|| {
            use idea_theme::theme::{install_idea_theme, light_theme};
            install_idea_theme(light_theme());
            let app = ChipStyles::root()
                .raw("tone", "primary")
                .set(ChipVariant::Filled)
                .set(ChipInteractive::On)
                .into_style_application();
            let rest = runtime_core::resolve_style(&app);
            assert_eq!(rest.border_left_width.as_ref().map(|w| w.resolve()), Some(1.0), "the reservation wins");
            let focused = runtime_core::resolve_style(&app.with("__state_focused", "on"));
            assert_eq!(
                focused.border_left_color.as_ref().and_then(|c| c.name()),
                Some("color-focus-ring"),
                "the ring wins over the recipe's transparent border"
            );
        });
    }
}
//! `Chip` — a selectable pill toggle.
//!
//! Where [`Badge`](super::badge::Badge) is a static status pill and
//! [`Tag`](super::tag::Tag) is a removable one, `Chip` is the *selectable*
//! member of the family: tapping it reports a select, and its `selected`
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
//! Both states resolve from the installed Tag stylesheet (chips and tags
//! share the pill shape and the tone × variant axes). The `selected`
//! state paints with the caller's chosen `variant`; the unselected state
//! drops to the quieter `Ghost` variant of the same tone, so a row of
//! chips reads as "one (or some) lit up, the rest muted" without the
//! caller wiring two stylesheets. Pass an explicit `variant` to set the
//! *selected* look (e.g. `Filled` for a stronger highlight).
//!
//! ## Size
//! `size` (`ControlSize::Sm` / `Md` / `Lg`, default `Md`) rides the Tag
//! sheets' `size` axis: the pill's padding on the container sheet and the
//! label's font size on the label sheet. `Md` is the Tag's own geometry.

use std::rc::Rc;

use runtime_core::{
    component, pressable, recipe, ui, view, Element, IdealystSchema, IntoElement, Reactive,
    StyleApplication,
};

use idea_theme::extensible::{
    installed_tag_sheet, installed_tag_text_sheets, tone, variant, ToneRef, Variant, VariantRef,
};

use crate::components::ControlSize;

// Reactive-by-default: `#[props]` wraps `selected`/`tone`/`variant`/`size` →
// `Reactive<…>`; `label` is already reactive, and `on_select` (an
// `Rc<dyn Fn()>` handler) is auto-skipped. Bare markers (`tone =
// tone::Primary`) coerce to `Reactive<ToneRef>` via the marker's generated
// `From`. The style-driving props route into the container and label style
// sinks, read `.get()` INSIDE so the apply-style Effect subscribes to
// whichever are live.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct ChipProps {
    /// Chip text. `Reactive<String>` — static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
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
    /// Outline, …). Default Soft. The unselected state always uses the
    /// quieter Ghost variant of the same tone.
    pub variant: VariantRef,
    /// Size scale (Sm, Md, Lg). Default Md. Picks the pill's padding and
    /// the label's font size (Sm: caption type, tight inset; Md: body-sm
    /// type, the Tag's geometry; Lg: body type, roomier inset).
    pub size: ControlSize,
}

impl Default for ChipProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            selected: Reactive::Static(false),
            on_select: None,
            tone: tone::Neutral.into(),
            variant: variant::Soft.into(),
            size: Reactive::Static(ControlSize::Md),
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

    // The styles are REACTIVE when any of selected/tone/variant/size is
    // live; else the build-time fast path (no first-paint flicker). The
    // closures read each prop's `.get()` INSIDE so the apply-style Effect
    // subscribes to whichever are dynamic.
    let style_is_reactive = !props.selected.is_static()
        || !props.tone.is_static()
        || !props.variant.is_static()
        || !props.size.is_static();

    // Selected → caller's variant (lit); unselected → Ghost (muted) of the
    // same tone. The container AND the label resolve the same key from the
    // installed Tag sheets, so a chip looks like a tag of the matching
    // tone/variant — and the label carries the tone's foreground itself,
    // because native text inherits no color from its box.
    let appearance = {
        let selected = props.selected.clone();
        let tone = props.tone.clone();
        let variant = props.variant.clone();
        move || {
            let variant_key = if selected.get() {
                variant.get().key()
            } else {
                variant::Ghost.key()
            };
            format!("{}_{}", tone.get().key(), variant_key)
        }
    };
    let make_style = {
        let appearance = appearance.clone();
        let size = props.size.clone();
        move || {
            // Hug lives in the tag sheet's base (it's unconditional); `size`
            // picks the padding arm. The pointer cursor — "anything
            // selectable shows a pointer" — rides the sheet's `interactive`
            // variant, so `clickable` is part of the resolution cache
            // identity. It used to ride a `with_computed` layer under the
            // constant key `"chip-box"`, which left `clickable` out of that
            // identity and let two chips with the same tone+variant but
            // different `on_select` share one resolved style.
            StyleApplication::new(installed_tag_sheet())
                .with("appearance", appearance())
                .with("size", size.get().as_variant_str().to_string())
                .with("interactive", if clickable { "on" } else { "off" }.to_string())
        }
    };
    let make_label_style = {
        let size = props.size.clone();
        let sheet = installed_tag_text_sheets().label.clone();
        move || {
            // The label half of the size axis (font size) + the fill's
            // foreground — see `TagSheetBuilder::build_text`.
            StyleApplication::new(sheet.clone())
                .with("appearance", appearance())
                .with("size", size.get().as_variant_str().to_string())
        }
    };

    let label_el: Element = if style_is_reactive {
        ui! { text(style = make_label_style) { label } }
    } else {
        let label_style = make_label_style();
        ui! { text(style = label_style) { label } }
    };

    // §9.6: a pressable only when the host supplied a handler. An inert chip
    // is a plain view — a no-op pressable would swallow the tap and block
    // hit-test fall-through to whatever sits under it.
    match on_select {
        Some(cb) => {
            let node = pressable(vec![label_el], move || (cb)());
            if style_is_reactive {
                node.with_style(make_style).into_element()
            } else {
                node.with_style(make_style()).into_element()
            }
        }
        None => {
            let node = view(vec![label_el]);
            if style_is_reactive {
                node.with_style(make_style).into_element()
            } else {
                node.with_style(make_style()).into_element()
            }
        }
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

    #[test]
    fn defaults_are_unselected_and_inert() {
        with_test_world(|| {
            let p = ChipProps::default();
            assert!(!p.selected.get());
            assert!(p.on_select.is_none());
            assert_eq!(p.size.get(), ControlSize::Md);
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
            // cursor — the ordering that made the old bug visible.
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
            size: Reactive::Static(size),
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
}

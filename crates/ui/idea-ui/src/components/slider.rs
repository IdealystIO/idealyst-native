//! `Slider` — a horizontal, draggable value track.
//!
//! A muted rail with a tone-colored fill from the left edge to a round thumb;
//! dragging anywhere on the track sets the value. Controlled: the host owns a
//! `Signal<f32>` and updates it in `on_change`.
//!
//! ```ignore
//! let volume = signal(0.5_f32);
//! ui! {
//!     Slider(
//!         value = volume,
//!         on_change = move |v: f32| volume.set(v),
//!         tone = tone::Primary,
//!     )
//! }
//! ```
//!
//! `thumb_icon` puts a glyph inside the thumb, tinted with the variant
//! foreground (the color a Button label takes on the same tone/variant). An
//! icon thumb is larger than the plain one so the glyph stays legible.
//!
//! Styling is [`SliderStyles`]: `container` (the drag box), `track` (the
//! muted rail), `fill`, `thumb`, `thumb_icon`, `row` (the icons-flanked
//! layout) and `side_icon`.
//!
//! ## Drag stability
//!
//! The drag math is `value = x / width`, so the track needs a **known, fixed
//! width** — hence the `width` prop (px, default 184). A percentage/fill width
//! would leave the divisor unknown inside the touch handler. Equally important:
//! the *host* must not re-key or rebuild the Slider mid-drag (e.g. a `when()`
//! keyed on the value being dragged) — that destroys the pointer-capturing view
//! and the drag stalls after the first move. Only the fill/thumb style layers
//! patch as the value changes; the element identity stays stable.

use std::rc::Rc;

use runtime_core::{
    component, component_styles, icon, ui, AlignItems, Cursor, Element, FlexDirection, IconData, IdealystSchema,
    IntoElement, JustifyContent, Length, Overflow, Position, Reactive, SlotRestyle, SlotStyle, StyleRules,
    Tokenized, TouchPhase, TouchResponse,
};

use idea_theme::extensible::{semantic_tones, ToneRef};

use crate::components::tone_slots::{recipe_compounds, with_tone};
use crate::components::ControlSize;
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;
use idea_theme::tokens;

component_styles! {
    /// The styled sections of [`Slider`].
    pub Slider<IdeaThemeRef> {
        /// The fill and thumb treatment — a tone-aware recipe per value.
        axis variant { #[default] filled, soft, outlined, ghost }

        /// The fixed-width drag box (track + thumb). Its per-instance width
        /// and the thumb-sized height ride the inline layer.
        slot container {
            base(_t) {
                position: Position::Relative,
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Stretch,
            }
            /// The disabled look.
            variant dimmed {
                #[default]
                off(_t) { cursor: Cursor::Pointer }
                on(_t) {
                    cursor: Cursor::Default,
                    opacity: 0.45,
                }
            }
        }

        /// The muted rail.
        slot track {
            base(t) {
                position: Position::Relative,
                background: t.color.border(),
                width: Length::pct(100.0),
                overflow: Overflow::Hidden,
                border_radius: t.radius.pill(),
            }
            /// Rail thickness.
            variant size {
                sm(_t) { height: 3.0 }
                #[default]
                md(_t) { height: 4.0 }
                lg(_t) { height: 6.0 }
            }
        }

        /// The tone bar from the left edge to the thumb; its width is the
        /// value (inline).
        slot fill {
            extend(tone_backgrounds)
            base(t) {
                position: Position::Absolute,
                left: 0.0,
                top: 0.0,
                height: Length::pct(100.0),
                border_radius: t.radius.pill(),
            }
            transitions {
                background: 120ms EaseOut,
            }
        }

        /// The round handle; `left` (the value position) rides the inline
        /// layer.
        slot thumb {
            extend(tone_backgrounds)
            base(t) {
                position: Position::Absolute,
                top: 0.0,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border_radius: t.radius.pill(),
                shadow: runtime_core::Shadow {
                    x: 0.0,
                    y: 1.0,
                    blur: 4.0,
                    color: runtime_core::Color("rgba(0,0,0,0.25)".into()).into(),
                },
            }
            /// Diameter. An icon thumb is larger (inline) so its glyph stays
            /// legible.
            variant size {
                sm(_t) { width: 12.0, height: 12.0 }
                #[default]
                md(_t) { width: 16.0, height: 16.0 }
                lg(_t) { width: 20.0, height: 20.0 }
            }
        }

        /// The glyph inside the thumb, in the variant's foreground (what a
        /// Button label takes on the same tone/variant).
        slot thumb_icon {
            extend(tone_foregrounds)
            base(_t) { flex_shrink: 0.0 }
            variant size {
                sm(_t) { width: 12.0, height: 12.0 }
                #[default]
                md(_t) { width: 14.0, height: 14.0 }
                lg(_t) { width: 16.0, height: 16.0 }
            }
        }

        /// Leading icon, slider, trailing icon.
        slot row {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.sm(),
            }
        }

        /// A leading / trailing icon (outside the drag area, so it can't
        /// perturb the `x / width` drag math).
        slot side_icon {
            base(_t) {
                width: 16.0,
                height: 16.0,
                flex_shrink: 0.0,
            }
        }
    }
}

fn tone_backgrounds(s: &mut SlotRestyle<'_, SliderStyles>) {
    recipe_compounds(s, &semantic_tones(), &[], |ctx, recipe| StyleRules {
        background: recipe.render(ctx).background,
        ..Default::default()
    });
}

fn tone_foregrounds(s: &mut SlotRestyle<'_, SliderStyles>) {
    recipe_compounds(s, &semantic_tones(), &[], |ctx, recipe| StyleRules {
        color: recipe.render(ctx).color,
        ..Default::default()
    });
}

marker_into_axis!(SliderVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

/// The control-wide size scale converts into Slider's own size axis.
impl From<ControlSize> for SliderSize {
    fn from(s: ControlSize) -> Self {
        SliderSize::define(s.as_variant_str())
    }
}

impl From<ControlSize> for Reactive<SliderSize> {
    fn from(s: ControlSize) -> Self {
        Reactive::Static(s.into())
    }
}

/// Thumb diameter (px) per size — the `thumb` slot's size arm, or the
/// larger icon-thumb diameter. Used to offset the thumb's `left` so its
/// *center* tracks the value, and as the container height. An app-defined
/// size takes `md`'s.
fn thumb_diameter(size: SliderSize, has_icon: bool) -> f32 {
    match (size, has_icon) {
        (SliderSize::Sm, false) => 12.0,
        (SliderSize::Lg, false) => 20.0,
        (_, false) => 16.0,
        (SliderSize::Sm, true) => 20.0,
        (SliderSize::Lg, true) => 28.0,
        (_, true) => 24.0,
    }
}

/// Normalized `0..1` position of `v` within `[min, max]`.
fn norm_pos(v: f32, min: f32, max: f32) -> f32 {
    if max <= min {
        0.0
    } else {
        runtime_core::num::clamp_f32((v - min) / (max - min), 0.0, 1.0)
    }
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>` (tone/variant/size/disabled + min/max/step/width + the icon
// slots). `value` is already `Reactive<f32>` (auto-skipped), `on_change` is a
// handler. NOTE min/max/step/width/size feed the DRAG MATH + the fixed
// container dims (the divisor `x / width` must stay stable — see the module
// docs); those are read once at build and not re-tracked. tone/variant/size
// re-style the fill/track/thumb in place; `disabled`'s DIM rides the reactive
// container style, and its press-BLOCK is read live in `on_touch` (per touch),
// so a live `disabled` blocks and unblocks in place.
#[runtime_core::props(styles = SliderStyles)]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct SliderProps {
    /// The current value. `Reactive<f32>` — a `Signal<f32>` the host owns, a
    /// static literal, or a model-derived `rx!(...)` getter (so the Slider works
    /// when the value lives in a document/model, read through a closure, not just
    /// a standalone signal). The host applies edits in `on_change`.
    pub value: Reactive<f32>,
    /// Fires with the new value while the user drags.
    pub on_change: Rc<dyn Fn(f32)>,
    /// Lower bound. Default `0.0`.
    pub min: f32,
    /// Upper bound. Default `1.0`.
    pub max: f32,
    /// Snap increment. `0.0` (default) is continuous; `> 0` snaps the value to
    /// the nearest `min + n*step`.
    pub step: f32,
    /// Track width in px. Default `184.0`. A fixed width keeps the drag math
    /// (`x / width`) stable — see the module docs.
    pub width: f32,
    /// Semantic palette for the fill + thumb. Default Primary.
    pub tone: ToneRef,
    /// Surface skeleton for the fill + thumb (or an app value). Default
    /// Filled.
    pub variant: SliderVariant,
    /// Rail thickness + thumb size (a `ControlSize` converts). Default Md.
    pub size: SliderSize,
    /// When `true`, blocks dragging and dims the control. Default `false`.
    /// A live value (`Signal` / `rx!`) blocks and unblocks in place.
    pub disabled: bool,
    /// Optional icon to the LEFT of the track (e.g. a "min"/volume-low glyph).
    /// Tinted muted. Sits outside the drag area so it can't perturb the
    /// `x / width` drag math.
    pub leading_icon: Option<IconData>,
    /// Optional icon to the RIGHT of the track (e.g. a "max"/volume-high glyph).
    pub trailing_icon: Option<IconData>,
    /// Optional icon INSIDE the thumb, tinted with the variant foreground.
    /// The thumb grows to hold it (Sm 20 / Md 24 / Lg 28 px). Whether a thumb
    /// icon is present is read once at build; a live value swaps the glyph in
    /// place.
    pub thumb_icon: Option<IconData>,
}

impl Default for SliderProps {
    fn default() -> Self {
        Self {
            value: Reactive::Static(0.0),
            on_change: Rc::new(|_| {}),
            min: Reactive::Static(0.0),
            max: Reactive::Static(1.0),
            step: Reactive::Static(0.0),
            width: Reactive::Static(184.0),
            tone: Reactive::Static(ToneRef::default()),
            variant: Reactive::Static(SliderVariant::default()),
            size: Reactive::Static(SliderSize::default()),
            disabled: Reactive::Static(false),
            leading_icon: Reactive::Static(None),
            trailing_icon: Reactive::Static(None),
            thumb_icon: Reactive::Static(None),
            styles: Default::default(),
        }
    }
}

/// A horizontal draggable value slider — see the module docs.
#[component]
pub fn Slider(props: &SliderProps) -> Element {
    let value = props.value.clone();
    let on_change = props.on_change.clone();
    // STRUCTURAL: the drag divisor (`x / width`) + the fixed container dims
    // must stay stable across renders, so min/max/step/width/size are read
    // once at build (see the module docs / props comment). A live one re-reads
    // only on a parent rebuild, not in place.
    let (min, max, step) = (props.min.get(), props.max.get(), props.step.get());
    let w = props.width.get();
    let size = props.size.get();
    // The press-BLOCK is read per touch, not once at build: a build-time read
    // left a live `disabled` dimming the slider while it still dragged (and a
    // slider mounted disabled undraggable after it was enabled). The DIM
    // rides the reactive container style below.
    let disabled = props.disabled.clone();
    // STRUCTURAL like `size`: presence picks the thumb diameter, which feeds
    // the `left` offset and the container height.
    let has_thumb_icon = props.thumb_icon.get().is_some();
    let dia = thumb_diameter(size, has_thumb_icon);

    let styles = props.styles.clone();
    // Tone, variant and size are read live, so a reactive one re-styles the
    // fill/track/thumb in place; every continuous value (the fill's width,
    // the thumb's `left`, the container's dimensions) rides an inline layer,
    // so all of it premints.
    let values =
        with_tone(SliderStyles::values().set(props.variant.clone()).set(props.size.clone()), &props.tone);

    // --- fill: width tracks the value% ---
    let fill = {
        let value = value.clone();
        let style = SliderStyles::fill()
            .values(&values)
            .inline_with(move || {
                let pct = norm_pos(value.get(), min, max) * 100.0;
                StyleRules { width: Some(Tokenized::Literal(Length::pct(pct))), ..Default::default() }
            })
            .overridden(styles.fill.as_ref());
        runtime_core::view(Vec::new()).with_style(style).into_element()
    };

    // --- track: the muted rail, holding the fill ---
    let track = runtime_core::view(vec![fill])
        .with_style(SliderStyles::track().values(&values).overridden(styles.track.as_ref()))
        .into_element();

    // --- thumb icon: glyph inside the thumb ---
    let thumb_kids = if has_thumb_icon {
        let mut el = icon(props.thumb_icon.get().expect("presence checked above"));
        if !props.thumb_icon.is_static() {
            // Presence is fixed for this build, so the fallback only covers a
            // live slot that turned `None`.
            let slot = props.thumb_icon.clone();
            el = el.data(move || slot.get().unwrap_or(crate::components::button::EMPTY_ICON_DATA));
        }
        let slot = {
            let values = values.clone();
            let styles = styles.clone();
            move || SliderStyles::thumb_icon().values(&values).overridden(styles.thumb_icon.as_ref())
        };
        vec![thumb_icon(el, slot)]
    } else {
        Vec::new()
    };

    // --- thumb: round handle, `left` tracks the value (center on the point) ---
    let thumb = {
        let value = value.clone();
        let style = SliderStyles::thumb()
            .values(&values)
            .inline_with(move || {
                let left = norm_pos(value.get(), min, max) * w - dia / 2.0;
                // An icon thumb's diameter is not a `size` arm, so it rides
                // the inline layer next to `left`.
                let (width, height) = if has_thumb_icon {
                    (Some(Tokenized::Literal(Length::Px(dia))), Some(Tokenized::Literal(Length::Px(dia))))
                } else {
                    (None, None)
                };
                StyleRules { left: Some(Tokenized::Literal(Length::Px(left))), width, height, ..Default::default() }
            })
            .overridden(styles.thumb.as_ref());
        runtime_core::view(thumb_kids).with_style(style).into_element()
    };

    // --- container: fixed-width relative box with the drag handler. The
    // DIM (cursor + opacity) is the `dimmed` axis read LIVE, so a reactive
    // `disabled` dims in place; the per-instance dimensions (author `width`,
    // size-derived thumb diameter) ride the inline layer. ---
    let container_style = SliderStyles::container()
        .values(&values)
        .set(props.disabled.clone().map(SliderDimmed::from))
        .inline(StyleRules {
            width: Some(Tokenized::Literal(Length::Px(w))),
            height: Some(Tokenized::Literal(Length::Px(dia))),
            ..Default::default()
        })
        .overridden(styles.container.as_ref());

    let slider = runtime_core::view(vec![track, thumb])
        .with_style(container_style)
        .on_touch(move |ev| {
            if disabled.get() {
                return TouchResponse::IGNORED;
            }
            // A non-primary press is not a drag on any platform, and it is
            // `Began`-only by contract — claiming it would both jump the
            // thumb on a right-click and swallow the press an ancestor's
            // context menu is waiting for. Recognizer-backed primitives get
            // this from `Recognizer::drive`; this handler is hand-rolled, so
            // it states the same rule itself.
            if ev.phase == TouchPhase::Began && !runtime_core::pointer_button().is_primary() {
                return TouchResponse::IGNORED;
            }
            if matches!(ev.phase, TouchPhase::Began | TouchPhase::Moved) {
                let t = runtime_core::num::clamp_f32(ev.position.x / w, 0.0, 1.0);
                let mut v = min + t * (max - min);
                if step > 0.0 {
                    v = min + ((v - min) / step).round() * step;
                }
                on_change(runtime_core::num::clamp_f32(v, min, max));
            }
            TouchResponse::CLAIMED
        })
        .into_element();

    // No icons → the bare slider. Otherwise flank the track with muted icons in
    // a row. The icons sit OUTSIDE the drag container, so the `x / width` math
    // (relative to the track) is untouched.
    let mk_icon = |data: IconData| -> Element {
        icon(data)
            .with_style(SliderStyles::side_icon().values(&values).overridden(styles.side_icon.as_ref()))
            .color(|| tokens().color.text_muted().resolve())
            .into_element()
    };
    // The leading/trailing icons drive STRUCTURE (whether the wrapping row
    // and icon nodes exist), so presence is read once at build.
    let leading_icon = props.leading_icon.get();
    let trailing_icon = props.trailing_icon.get();
    if leading_icon.is_none() && trailing_icon.is_none() {
        return slider;
    }
    let row_style = SliderStyles::row().values(&values).overridden(styles.row.as_ref());
    ui! {
        view(style = row_style) {
            if let Some(d) = leading_icon {
                mk_icon(d)
            }
            slider
            if let Some(d) = trailing_icon {
                mk_icon(d)
            }
        }
    }
}

/// Style the thumb's icon. Mirrors Checkbox's `checkmark_icon`: on a
/// premint build the slot is ATTACHED (its class carries the arm's `color`,
/// and the SVG tints via `currentColor`). On a live/native build the arm's
/// color is resolved and stamped — native icons don't inherit color — and
/// re-resolved on a tone/variant/theme change.
fn thumb_icon(el: runtime_core::GlueIcon, slot: impl Fn() -> SlotStyle<SliderStyles> + 'static) -> Element {
    if slot().into_style_application().attaches_preminted() {
        return el.with_style(slot()).into_element();
    }
    let style = slot();
    el.with_style(style)
        .color(move || {
            runtime_core::resolve_style(&slot().into_style_application())
                .color
                .as_ref()
                .map(|c| c.resolve())
                .unwrap_or_else(|| tokens().color.text().resolve())
        })
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, TStyle, P};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::FillRule;

    const DOT: IconData = IconData {
        view_box: (24, 24),
        paths: &["M12 12h.01"],
        fill_rule: FillRule::NonZero,
        filled: true,
    };

    fn view_children(el: Element) -> Vec<Element> {
        match classify(el) {
            P::View { children, .. } => children,
            _ => panic!("Slider renders a View"),
        }
    }

    /// Pull the drag container's real `on_touch` out of the built tree.
    /// `classify` collapses handler presence to a `bool`, which is enough
    /// for structure tests but can't exercise the handler itself.
    fn drag_handler(mut el: Element) -> runtime_core::TouchHandler {
        use runtime_vocabulary::prims::{PrimCell, ViewPrim};
        // A robot build wraps every component root in `Element::Owned` to
        // carry its Inspector realize hook; look through it.
        while let Element::Owned { element, .. } = el {
            el = *element;
        }
        match el {
            Element::Item { data, .. } => data
                .downcast_ref::<PrimCell<ViewPrim>>()
                .expect("Slider renders a View")
                .take()
                .on_touch
                .expect("the drag container carries on_touch"),
            _ => panic!("Slider renders a View item"),
        }
    }

    fn touch(phase: TouchPhase, x: f32) -> runtime_core::TouchEvent {
        runtime_core::TouchEvent {
            id: runtime_core::TouchId(1),
            phase,
            position: runtime_core::TouchPoint::new(x, 0.0),
            window_position: runtime_core::TouchPoint::new(x, 0.0),
            timestamp_ns: 0,
            force: None,
        }
    }

    /// A right-click on a slider is not a drag on any platform. Before the
    /// gate this handler claimed the press unconditionally: the thumb
    /// jumped to the cursor AND the press never reached the context-menu
    /// handler above it. Recognizer-backed controls get this from
    /// `Recognizer::drive`; the slider's handler is hand-rolled, so it
    /// carries the same rule and this test pins it.
    #[test]
    fn regression_slider_ignores_secondary_press() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let changes = Rc::new(std::cell::Cell::new(0u32));
            let h = {
                let changes = changes.clone();
                drag_handler(Slider(&SliderProps {
                    on_change: Rc::new(move |_| changes.set(changes.get() + 1)),
                    ..Default::default()
                }))
            };

            runtime_shared::set_pointer_button(runtime_core::PointerButton::Secondary);
            let r = h(&touch(TouchPhase::Began, 90.0));
            runtime_shared::set_pointer_button(runtime_core::PointerButton::Primary);
            assert!(
                !r.consumed && !r.claim,
                "a secondary press must bubble to an ancestor's context menu"
            );
            assert_eq!(changes.get(), 0, "and must not move the thumb");

            // The primary press right after it still drags.
            let r = h(&touch(TouchPhase::Began, 90.0));
            assert!(r.claim, "a primary press still claims the drag");
            assert_eq!(changes.get(), 1);
        });
    }

    /// A LIVE `disabled` blocks the drag in place. The press-block used to be
    /// read once at build, so `Slider(disabled = rx!(saving))` dimmed while
    /// saving but still dragged (and a slider mounted disabled never became
    /// draggable). Fails against the build-time read: the drag after the
    /// flip still reports a value.
    #[test]
    fn regression_slider_live_disabled_still_drags() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let locked = runtime_core::signal(false);
            let changes = Rc::new(std::cell::Cell::new(0u32));
            let h = {
                let changes = changes.clone();
                drag_handler(Slider(&SliderProps {
                    on_change: Rc::new(move |_| changes.set(changes.get() + 1)),
                    disabled: Reactive::from(locked),
                    ..Default::default()
                }))
            };
            assert!(h(&touch(TouchPhase::Began, 90.0)).claim, "enabled: drags");
            assert_eq!(changes.get(), 1);

            locked.set(true);
            idea_theme::testing::commit();
            let r = h(&touch(TouchPhase::Began, 40.0));
            assert!(!r.claim && !r.consumed, "disabled: the press is ignored");
            assert_eq!(changes.get(), 1, "and the thumb does not move");

            locked.set(false);
            idea_theme::testing::commit();
            assert!(h(&touch(TouchPhase::Began, 40.0)).claim, "re-enabled: drags again");
            assert_eq!(changes.get(), 2);
        });
    }

    /// The thumb (the container's second child) and its inline layer.
    fn thumb_parts(el: Element) -> (Vec<Element>, Rc<StyleRules>) {
        let thumb = view_children(el).remove(1);
        match classify(thumb) {
            P::View { children, style: Some(TStyle::AppFn(f)), .. } => {
                let inline = f().inline().cloned().expect("thumb carries an inline layer");
                (children, inline)
            }
            _ => panic!("the thumb is a View styled by a live application"),
        }
    }

    // `thumb_icon` mounts an Icon INSIDE the thumb and grows the thumb to the
    // icon-thumb diameter, centered on the value: the inline `left` offset
    // and the container height must use the same diameter or the thumb
    // drifts off the fill's end.
    #[test]
    fn thumb_icon_renders_inside_a_larger_thumb() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let el = Slider(&SliderProps {
                value: Reactive::Static(0.5),
                thumb_icon: Reactive::Static(Some(DOT)),
                ..Default::default()
            });
            let (kids, inline) = thumb_parts(el);
            assert_eq!(kids.len(), 1, "the thumb holds the icon");
            assert!(matches!(classify(kids.into_iter().next().unwrap()), P::Icon { .. }));
            let px = |v: f32| Some(Tokenized::Literal(Length::Px(v)));
            assert_eq!(inline.width, px(24.0), "Md icon thumb is 24px wide");
            assert_eq!(inline.height, px(24.0));
            assert_eq!(inline.left, px(0.5 * 184.0 - 12.0), "centered on the value");

            // No thumb icon → an empty thumb sized by the sheet's `size` arm.
            let (kids, inline) = thumb_parts(Slider(&SliderProps {
                value: Reactive::Static(0.5),
                ..Default::default()
            }));
            assert!(kids.is_empty());
            assert_eq!(inline.width, None, "plain thumb keeps the sheet's diameter");
            assert_eq!(inline.left, px(0.5 * 184.0 - 8.0));
        });
    }

    // The thumb icon takes the variant FOREGROUND (the glyph sheet's
    // appearance arm), so it reads on the thumb's tone fill. Native icons
    // don't inherit color, so on a live build the tint must be stamped.
    #[cfg(not(idealyst_premint))]
    #[test]
    fn thumb_icon_is_tinted_with_the_variant_foreground() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let (kids, _) = thumb_parts(Slider(&SliderProps {
                thumb_icon: Reactive::Static(Some(DOT)),
                ..Default::default()
            }));
            let color = match classify(kids.into_iter().next().unwrap()) {
                P::Icon { color, .. } => color.expect("the thumb icon is tinted"),
                _ => panic!("thumb child is an Icon"),
            };
            let expected = runtime_core::resolve_style(
                &SliderStyles::thumb_icon().raw("tone", "primary").set(SliderVariant::Filled).into_style_application(),
            )
            .color
            .as_ref()
            .expect("the appearance arm sets a foreground")
            .resolve();
            assert_eq!(color, expected);
        });
    }

    // Icons flank the track in an OUTER row (leading icon, slider, trailing
    // icon) so they sit outside the drag container — keeping the `x / width`
    // drag math relative to the track intact.
    #[test]
    fn icons_flank_the_track_outside_the_drag_container() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let kids = view_children(Slider(&SliderProps {
                leading_icon: Reactive::Static(Some(DOT)),
                trailing_icon: Reactive::Static(Some(DOT)),
                ..Default::default()
            }));
            assert_eq!(kids.len(), 3, "leading icon + slider + trailing icon");
            let kinds: Vec<P> = kids.into_iter().map(classify).collect();
            assert!(matches!(kinds[0], P::Icon { .. }), "leading icon");
            assert!(matches!(kinds[2], P::Icon { .. }), "trailing icon");

            // No icons → no wrapping row; the bare slider container (track + thumb).
            let plain = view_children(Slider(&SliderProps::default()));
            assert_eq!(plain.len(), 2, "track + thumb, no icon row");
    });
    }
}

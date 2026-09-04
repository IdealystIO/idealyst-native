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
    component, icon, AlignItems, Cursor, Element, FlexDirection, IconData, IdealystSchema,
    IntoElement, JustifyContent, Length, Position, Reactive, StyleApplication, StyleRules,
    StyleSheet, Tokenized, TouchPhase, TouchResponse, VariantSet,
};

use idea_theme::extensible::{installed_slider_sheets, ToneRef, VariantRef};

use crate::components::ControlSize;
use idea_theme::tokens;

/// Thumb diameter (px) per size — mirrors `SLIDER_DIMS` in idea-theme, used to
/// offset the thumb's `left` so its *center* tracks the value.
fn thumb_diameter(size: ControlSize) -> f32 {
    match size {
        ControlSize::Sm => 12.0,
        ControlSize::Md => 16.0,
        ControlSize::Lg => 20.0,
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
// container style, its press-BLOCK is read once in `on_touch`.
#[runtime_core::props]
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
    /// Surface skeleton for the fill + thumb. Default Filled.
    pub variant: VariantRef,
    /// Rail thickness + thumb size. Default Md.
    pub size: ControlSize,
    /// When `true`, blocks dragging and dims the control. Default `false`.
    pub disabled: bool,
    /// Optional icon to the LEFT of the track (e.g. a "min"/volume-low glyph).
    /// Tinted muted. Sits outside the drag area so it can't perturb the
    /// `x / width` drag math.
    pub leading_icon: Option<IconData>,
    /// Optional icon to the RIGHT of the track (e.g. a "max"/volume-high glyph).
    pub trailing_icon: Option<IconData>,
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
            variant: Reactive::Static(VariantRef::default()),
            size: Reactive::Static(ControlSize::default()),
            disabled: Reactive::Static(false),
            leading_icon: Reactive::Static(None),
            trailing_icon: Reactive::Static(None),
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
    // The press-BLOCK is read once (the `on_touch` closure captures it); the
    // DIM rides the reactive container style below.
    let disabled = props.disabled.get();
    let dia = thumb_diameter(size);

    // Style keys as live closures so a reactive tone/variant/size re-styles the
    // fill/track/thumb in place; bare props collapse to a static resolution.
    let appearance_for = {
        let tone = props.tone.clone();
        let variant = props.variant.clone();
        move || format!("{}_{}", tone.get().key(), variant.get().key())
    };
    let size_key_for = {
        let size_prop = props.size.clone();
        move || size_prop.get().as_variant_str().to_string()
    };
    let sheets = installed_slider_sheets();

    // --- fill: width tracks the value%, cached per whole percent ---
    let fill = {
        let fill_sheet = sheets.fill_sheet.clone();
        let app = appearance_for.clone();
        let value = value.clone();
        runtime_core::view(Vec::new())
            .with_style(move || {
                let pct = norm_pos(value.get(), min, max) * 100.0;
                StyleApplication::new(fill_sheet.clone())
                    .with("appearance", app())
                    // Continuous — one entry/class per whole percent as a
                    // computed layer. Inline instead.
                    .with_inline(StyleRules {
                        width: Some(Tokenized::Literal(Length::pct(pct))),
                        ..Default::default()
                    })
            })
            .into_element()
    };

    // --- track: the muted rail, holding the fill ---
    let track = {
        let track_sheet = sheets.track_sheet.clone();
        let size_key = size_key_for.clone();
        runtime_core::view(vec![fill])
            .with_style(move || {
                StyleApplication::new(track_sheet.clone()).with("size", size_key())
            })
            .into_element()
    };

    // --- thumb: round handle, `left` tracks the value (center on the point) ---
    let thumb = {
        let thumb_sheet = sheets.thumb_sheet.clone();
        let app = appearance_for.clone();
        let size_key = size_key_for.clone();
        let value = value.clone();
        runtime_core::view(Vec::new())
            .with_style(move || {
                let left = norm_pos(value.get(), min, max) * w - dia / 2.0;
                StyleApplication::new(thumb_sheet.clone())
                    .with("appearance", app())
                    .with("size", size_key())
                    // Continuous, and the worst of the set: keyed on the
                    // thumb's pixel offset, a drag across a 300px track minted
                    // ~300 cache entries and ~300 classes, none reusable.
                    .with_inline(StyleRules {
                        left: Some(Tokenized::Literal(Length::Px(left))),
                        ..Default::default()
                    })
            })
            .into_element()
    };

    // --- container: fixed-width relative box with the drag handler. Layout
    // lives on ONE shared, identified sheet; the DIM (cursor + opacity) is a
    // `dimmed` on/off AXIS read LIVE inside the style closure, so a reactive
    // `disabled` dims/uncurses in place; the per-instance dimensions (author
    // `width`, size-derived thumb diameter) ride the INLINE layer. Every arm
    // has build-time CSS, so the container premints — the former
    // `with_computed` dim layer kept every Slider on the live engine, and the
    // per-instance base sheet minted a class per (width, dia) pair. ---
    let disabled_dim = props.disabled.clone();
    let container_style = move || {
        StyleApplication::new(slider_container_sheet())
            .with("dimmed", if disabled_dim.get() { "on" } else { "off" })
            .with_inline(StyleRules {
                width: Some(Tokenized::Literal(Length::Px(w))),
                height: Some(Tokenized::Literal(Length::Px(dia))),
                ..Default::default()
            })
    };

    let slider = runtime_core::view(vec![track, thumb])
        .with_style(container_style)
        .on_touch(move |ev| {
            if disabled {
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
            .size(16.0)
            .color(|| tokens().color.text_muted().resolve())
            .into_element()
    };
    // TODO(reactive-sweep): the leading/trailing icons drive STRUCTURE (whether
    // the wrapping row + icon nodes exist), so they're read once at build; a
    // reactive icon swap would need a `when()`/`switch` around the row. The
    // common case is fixed presence.
    let leading_icon = props.leading_icon.get();
    let trailing_icon = props.trailing_icon.get();
    if leading_icon.is_none() && trailing_icon.is_none() {
        return slider;
    }
    let mut kids: Vec<Element> = Vec::with_capacity(3);
    if let Some(d) = leading_icon {
        kids.push(mk_icon(d));
    }
    kids.push(slider);
    if let Some(d) = trailing_icon {
        kids.push(mk_icon(d));
    }
    // `r#static` so the row sheet auto-premints by content — a closure
    // sheet here was a live-engine fall-through on the docs corpus.
    let row_style = Rc::new(StyleSheet::r#static(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(tokens().spacing.sm()),
        ..Default::default()
    }));
    runtime_core::view(kids).with_style(row_style).into_element()
}

thread_local! {
    static SLIDER_CONTAINER_SHEET: std::cell::RefCell<Option<Rc<StyleSheet>>> =
        const { std::cell::RefCell::new(None) };
}

/// The shared Slider container sheet: relative column that centers the
/// track, with a `dimmed` axis for the disabled look. Per-instance
/// dimensions ride the application's inline layer (see the container
/// style closure), so ONE sheet serves every Slider at every width.
fn slider_container_sheet() -> Rc<StyleSheet> {
    SLIDER_CONTAINER_SHEET.with(|s| {
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(
                StyleSheet::new(|_vs: &VariantSet| StyleRules {
                    position: Some(Position::Relative),
                    flex_direction: Some(FlexDirection::Column),
                    justify_content: Some(JustifyContent::Center),
                    align_items: Some(AlignItems::Stretch),
                    ..Default::default()
                })
                .variant("dimmed", "off", |_vs| StyleRules {
                    cursor: Some(Cursor::Pointer),
                    ..Default::default()
                })
                .variant("dimmed", "on", |_vs| StyleRules {
                    cursor: Some(Cursor::Default),
                    opacity: Some(Tokenized::Literal(SLIDER_DISABLED_OPACITY)),
                    ..Default::default()
                })
                .variant_default("dimmed", "off")
                .premint_as("idea-ui.v1.slider.container"),
            );
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

/// Opacity of a disabled Slider — matches the dim the old computed
/// layer applied.
const SLIDER_DISABLED_OPACITY: f32 = 0.45;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
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
    fn drag_handler(el: Element) -> runtime_core::TouchHandler {
        use runtime_vocabulary::prims::{PrimCell, ViewPrim};
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

//! `SegmentedControl` — a row of mutually-exclusive options.
//!
//! The "iOS segmented picker" pattern: a short, fixed row where exactly
//! one segment is selected at a time. Before this existed you hand-rolled
//! it as a `Stack(axis = Row)` of `Button`s and tracked selection
//! yourself; `SegmentedControl` packages that into one controlled
//! component.
//!
//! ```ignore
//! let view = signal("list".to_string());
//! let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| view.set(v));
//! ui! {
//!     SegmentedControl(
//!         value = view,
//!         on_change = on_change,
//!         options = vec![
//!             SegmentOption::new("list", "List"),
//!             SegmentOption::new("grid", "Grid"),
//!             SegmentOption::new("map", "Map"),
//!         ],
//!     )
//! }
//! ```
//!
//! Like [`Select`](super::select::Select) and the other selection
//! controls, it's **controlled by value**: the host owns a
//! `Signal<String>` holding the selected option's `id`, and `on_change`
//! commits the newly-picked `id`. The segment whose `id` equals the
//! current `value` paints selected; mutual exclusivity is automatic
//! because exactly one `id` can equal `value`.
//!
//! ## Adornments
//!
//! A segment can carry a `leading` and a `trailing` [`Adornment`] beside its
//! label — the same type `Field` uses. The control lays out the row and
//! renders the label, so the text is written once:
//!
//! ```ignore
//! SegmentOption::new("day", "Day shift")
//!     .leading(Adornment::element(|| ui! { view(style = ShiftDot()) }))
//! SegmentOption::new("map", "Map").leading(Adornment::Icon(icons_lucide::MAP))
//! ```
//!
//! An `Icon` adornment takes the segment's own foreground — muted at rest,
//! full text color when selected — exactly like the label. `Element` is any
//! component, rendered as built; `Button` is a tappable icon that eats its
//! own tap (the segment is not selected by it); `Group` sits several side
//! by side.
//!
//! ## Appearance
//! A bordered, tinted `track` holding one `segment` per option; the selected
//! segment is filled in (the `selected` axis flips between `on` and `off`)
//! and its `label` takes the full text color. It is deliberately unlike
//! [`Tabs`](super::tabs::Tabs): tabs navigate, a segmented control picks a
//! value, and the two must not read the same when they share a page.
//!
//! ## Restyling
//! Every styled section is a slot of [`SegmentedControlStyles`]: `track`,
//! `segment`, `inner` (an adorned segment's row), `label` and `adornment`
//! (a button adornment). Restyle any of them app-wide at boot, or one
//! control through its `styles` prop:
//!
//! ```ignore
//! SegmentedControlStyles::restyle(|s| {
//!     s.track().base(|t| StyleRules { background: Some(t.color.text()), ..Default::default() });
//!     s.segment().arm(SegmentedControlSelected::On, |t| StyleRules {
//!         background: Some(t.intent.primary.solid_bg()),
//!         ..Default::default()
//!     });
//!     s.label().arm(SegmentedControlSelected::On, |t| StyleRules {
//!         color: Some(t.intent.primary.solid_text()),
//!         ..Default::default()
//!     });
//! });
//! ```

use std::rc::Rc;

use runtime_core::{
    component, component_styles, derived, memo, pressable, recipe, ui, Color, Cursor, Element, FlexDirection,
    FontWeight, IdealystSchema, IntoElement, JustifyContent, AlignItems, Length, Memo, Reactive,
};

use crate::components::field::Adornment;
use crate::components::ControlSize;
use crate::theme::IdeaThemeRef;
use crate::Icon;

/// How far the selected segment sits inside the track's border, and the
/// gap between segments. Below the spacing scale on purpose (`xs` is 4):
/// it is a hairline-scale detail like a border width, and at `xs` the
/// selected key floated visibly loose inside the track.
pub const SEGMENT_TRACK_INSET: f32 = 2.0;

/// What a segment's height carries on each vertical side besides its own
/// padding: the track's 1px border, the track inset, and the segment's 1px
/// border.
pub const SEGMENT_CHROME_V: f32 = 1.0 + SEGMENT_TRACK_INSET + 1.0;

/// The border a bordered control (Select, Field, outlined Button) draws on
/// each side — part of the height a segmented control has to match.
const CONTROL_BORDER: f32 = 1.0;

/// A segment's vertical padding at a control size whose padding is
/// `control_pad` (the `spacing-*` token Select / Field / Button use at that
/// size): that padding plus the control's own border, minus
/// [`SEGMENT_CHROME_V`]. With it, a segmented control's outer height equals
/// a bordered control's at the same size — `2 × padding + line height + 2px
/// border` — so it lines up in a toolbar beside a Select or an outlined
/// Button.
///
/// A literal resolved from the token, not the token itself: lengths have
/// no arithmetic. It follows the theme installed before the slot is first
/// composed (the default scale, or an app's own); a spacing change made
/// while the app runs would not reach it.
fn segment_pad_v(control_pad: runtime_core::Tokenized<Length>) -> f32 {
    match control_pad.resolve() {
        Length::Px(v) => (v + CONTROL_BORDER - SEGMENT_CHROME_V).max(0.0),
        _ => 0.0,
    }
}

component_styles! {
    /// The styled sections of [`SegmentedControl`].
    pub SegmentedControl<IdeaThemeRef> {
        /// Control size: each step is the same height as a Select, Field or
        /// outlined Button at that size, with the same font.
        axis size { sm, #[default] md, lg }

        /// The track. Hugs its segments on both axes, so a control inside a
        /// stretching column stays the width of its options — unless
        /// `block`, which fills the container like Button's `block`.
        slot track {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Stretch,
                align_self: runtime_core::AlignSelf::FlexStart,
                flex_shrink: 0.0,
                padding: SEGMENT_TRACK_INSET,
                gap: SEGMENT_TRACK_INSET,
                background: t.color.surface_alt(),
                border_width: 1.0,
                border_color: t.color.border(),
                border_radius: t.radius.md(),
            }
            variant block {
                #[default]
                off(_t) {}
                on(_t) {
                    width: Length::Percent(100.0),
                    align_self: runtime_core::AlignSelf::Stretch,
                    flex_shrink: 1.0,
                }
            }
            transitions {
                background: 250ms EaseInOut,
                border_color: 250ms EaseInOut,
            }
        }

        /// One segment (a pressable).
        slot segment {
            base(t) {
                // Hold the label's natural width: segments never squash.
                flex_shrink: 0.0,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                // One step below the track's radius, so the selected fill
                // reads as nested inside the track's corners.
                border_radius: t.radius.sm(),
                // A transparent 1px border at rest, so the selected
                // segment's border and the focus ring change color without
                // changing size.
                border_width: 1.0,
                border_color: Color("transparent".into()),
                background: Color("transparent".into()),
                color: t.color.text_muted(),
                font_weight: FontWeight::Medium,
                cursor: Cursor::Pointer,
            }
            // Select's and Button's horizontal padding per size, and a
            // vertical padding that makes the whole control the same height
            // as theirs — see `segment_pad_v`.
            variant size {
                sm(t) {
                    padding_vertical: segment_pad_v(t.spacing.xs()),
                    padding_horizontal: t.spacing.sm(),
                    font_size: t.typography.body_sm_size(),
                }
                md(t) {
                    padding_vertical: segment_pad_v(t.spacing.sm()),
                    padding_horizontal: t.spacing.md(),
                    font_size: t.typography.body_size(),
                }
                lg(t) {
                    padding_vertical: segment_pad_v(t.spacing.md()),
                    padding_horizontal: t.spacing.lg(),
                    font_size: t.typography.body_lg_size(),
                }
            }
            // `block`: segments split the filled track evenly. `flex_basis:
            // 0` makes the shares equal regardless of label length; the
            // label's min-content width still floors each one.
            variant block {
                #[default]
                off(_t) {}
                on(_t) {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    flex_basis: Length::Px(0.0),
                }
            }
            variant selected {
                #[default]
                off(_t) {}
                on(t) {
                    background: t.color.surface(),
                    border_color: t.color.border(),
                    color: t.color.text(),
                }
            }
            // `state` rules win over the variant, so none of them repaint
            // the background: a hover wash would hide which segment is
            // selected.
            state hovered(t) {
                color: t.color.text(),
            }
            state pressed(t) {
                color: t.color.text(),
            }
            state focused(t) {
                color: t.color.text(),
                border_color: t.color.focus_ring(),
            }
            transitions {
                color: 150ms EaseOut,
                background: 120ms EaseOut,
                border_color: 120ms EaseOut,
            }
        }

        /// The row an adorned segment lays out: adornment, label,
        /// adornment, centred on one line.
        slot inner {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.sm(),
            }
        }

        /// A segment's label text. It carries its own color and font: only
        /// web's cascade would inherit them from the segment, native text
        /// never does — so a selected segment's text, a light/dark swap and
        /// the control size all reach the label on every backend from here.
        slot label {
            base(t) {
                color: t.color.text_muted(),
                font_weight: FontWeight::Medium,
            }
            variant size {
                sm(t) { font_size: t.typography.body_sm_size() }
                md(t) { font_size: t.typography.body_size() }
                lg(t) { font_size: t.typography.body_lg_size() }
            }
            variant selected {
                #[default]
                off(_t) {}
                on(t) { color: t.color.text() }
            }
            transitions {
                color: 150ms EaseOut,
            }
        }

        /// A `Button` adornment: icon-sized, a pointer cursor and a subtle
        /// hover/press dim.
        slot adornment {
            base(_t) {
                cursor: Cursor::Pointer,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
            }
            state hovered(_t) { opacity: 0.65 }
            state pressed(_t) { opacity: 0.4 }
        }
    }
}

/// The control-wide size scale converts into the segmented control's own
/// size axis, so `size = ControlSize::Sm` keeps lining controls up.
impl From<ControlSize> for SegmentedControlSize {
    fn from(s: ControlSize) -> Self {
        match s {
            ControlSize::Sm => Self::Sm,
            ControlSize::Md => Self::Md,
            ControlSize::Lg => Self::Lg,
        }
    }
}

impl From<ControlSize> for Reactive<SegmentedControlSize> {
    fn from(s: ControlSize) -> Self {
        Reactive::Static(s.into())
    }
}

/// Point size of an `Icon`/`Button` adornment in a segment at each control
/// size — the same scale `Field` uses for its adornments, so an icon sits
/// in proportion to the label beside it. An app-defined size takes `md`'s.
fn segment_icon_px(size: SegmentedControlSize) -> f32 {
    match size {
        SegmentedControlSize::Sm => 14.0,
        SegmentedControlSize::Lg => 18.0,
        _ => 16.0,
    }
}

/// One segment in a [`SegmentedControl`]. `id` is the value committed to
/// the bound signal when this segment is chosen; `label` is what the user
/// sees, with optional `leading` / `trailing` adornments beside it.
#[derive(Clone, IdealystSchema, runtime_core::Remote)]
pub struct SegmentOption {
    /// Stable value committed to the control's `value` signal when this
    /// segment is chosen. Compared against the current value to mark the
    /// selected segment.
    pub id: String,
    /// Segment label. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    /// Adornment before the label — an icon, a status dot, any component.
    pub leading: Adornment,
    /// Adornment after the label — a count badge, a trailing icon.
    pub trailing: Adornment,
}

impl SegmentOption {
    pub fn new(id: impl Into<String>, label: impl Into<Reactive<String>>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            leading: Adornment::None,
            trailing: Adornment::None,
        }
    }

    /// Put `adornment` before the label: `.leading(Adornment::Icon(MAP))`,
    /// or `.leading(Adornment::element(|| ui! { … }))` for any component.
    pub fn leading(mut self, adornment: Adornment) -> Self {
        self.leading = adornment;
        self
    }

    /// Put `adornment` after the label.
    pub fn trailing(mut self, adornment: Adornment) -> Self {
        self.trailing = adornment;
        self
    }
}

// Reactive-by-default: `value` is already `Reactive<String>`, `on_change` is a
// handler (`Rc`) and `options` a `Vec`, all auto-skipped; `size` and `block`
// are wrapped and routed into each segment's style, label and adornments.
// `styles` (from `styles = SegmentedControlStyles`) is the per-instance
// override for every slot.
#[runtime_core::props(styles = SegmentedControlStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct SegmentedControlProps {
    /// Controlled selected value — the `id` of the chosen [`SegmentOption`].
    /// `Reactive<String>` — a `Signal<String>` the host owns, or a model-derived
    /// `rx!(...)` that maps a typed enum/bool to the matching option `id` (so the
    /// control fits state that isn't a standalone string signal). Tapping a
    /// segment reports the new `id` via `on_change`; the segment whose `id`
    /// equals this paints selected.
    pub value: Reactive<String>,
    /// Fires with the chosen segment's `id` when the user taps a segment.
    /// Default is a no-op so an unwired control doesn't silently mutate —
    /// pass `move |id| value.set(id)` to make taps switch segments.
    pub on_change: Rc<dyn Fn(String)>,
    /// The segments, left-to-right.
    pub options: Vec<SegmentOption>,
    /// Control size — `Sm`, `Md` (default) or `Lg`, or an app-defined
    /// [`SegmentedControlSize`]. Each built-in size is the same height as a
    /// Select, Field or outlined Button at that size, with the same font,
    /// so the control lines up beside them in a toolbar. A `ControlSize`
    /// converts.
    pub size: SegmentedControlSize,
    /// When `true`, the control fills its container's width and the
    /// segments share it evenly — like Button's `block`. Default `false`:
    /// the control hugs its options.
    pub block: bool,
}

impl Default for SegmentedControlProps {
    // Manual impl: `Rc<dyn Fn(String)>` doesn't derive `Default`.
    fn default() -> Self {
        Self {
            value: Reactive::Static(String::new()),
            on_change: Rc::new(|_| {}),
            options: Vec::new(),
            size: Reactive::Static(SegmentedControlSize::default()),
            block: Reactive::Static(false),
            styles: Default::default(),
        }
    }
}

/// Renders a row of mutually-exclusive segments with reactive selected
/// highlighting. Pure UI: it draws one pressable per `SegmentOption`,
/// lights the one whose `id` matches `value`, and reports taps via
/// `on_change` — the host owns the selected value.
#[component]
pub fn SegmentedControl(props: SegmentedControlProps) -> Element {
    let styles = props.styles;
    let options = props.options;
    let value = props.value;
    let on_change = props.on_change;
    let size = props.size;
    let block = props.block.map(SegmentedControlBlock::from);
    // The component-level selections every slot sees, so a restyle arm on
    // `size` or `block` lands on whichever slot it targets.
    let values = SegmentedControlStyles::values().set(size.clone()).set(block);
    let track_style = SegmentedControlStyles::track().values(&values).overridden(styles.track.as_ref());

    ui! {
        view(style = track_style) {
            for option in options {
                segment(option, value.clone(), on_change.clone(), size.clone(), values.clone(), styles.clone())
            }
        }
    }
}

/// Build one segment pressable: the `value`-matched selected style and its
/// label.
///
/// `Pressable` isn't a ui!-level tag (the framework macro omits it so
/// idea-ui owns the styled wrapper), so the segment is built with the
/// builder fns — the same shape `Tabs::tab_button` uses.
fn segment(
    option: SegmentOption,
    value: Reactive<String>,
    on_change: Rc<dyn Fn(String)>,
    size: Reactive<SegmentedControlSize>,
    values: runtime_core::AxisValues<SegmentedControlStyles>,
    styles: SegmentedControlSlotStyles,
) -> Element {
    let id = option.id;

    // The press commits this segment's own `id`.
    let id_for_press = id.clone();
    let press = move || on_change(id_for_press.clone());

    // One derived flag per segment, shared by the segment's own style, its
    // label, and its adornments' glyphs — a memo, so a value change that leaves
    // this segment's state alone wakes none of them.
    let selected = memo(move || value.get() == id);
    let picked = derived(move || SegmentedControlSelected::from(selected.get()));

    let seg_style = SegmentedControlStyles::segment()
        .values(&values)
        .set(picked)
        .overridden(styles.segment.as_ref());
    let label_style = SegmentedControlStyles::label()
        .values(&values)
        .set(derived(move || SegmentedControlSelected::from(selected.get())))
        .overridden(styles.label.as_ref());

    let label = option.label;
    let leading = render_adornment(&option.leading, selected, size.clone(), &styles);
    let trailing = render_adornment(&option.trailing, selected, size, &styles);
    let content = if leading.is_empty() && trailing.is_empty() {
        ui! { text(style = label_style) { label } }
    } else {
        let inner_style = SegmentedControlStyles::inner().values(&values).overridden(styles.inner.as_ref());
        // `leading` / `trailing` are the adornments the CALLER handed in,
        // already built — splatted, not authored here.
        ui! {
            view(style = inner_style) {
                leading
                text(style = label_style) { label }
                trailing
            }
        }
    };
    pressable(vec![content], press).with_style(seg_style).into()
}

/// Build an adornment for a segment (empty for `None` / an empty group).
/// `Icon` and `Button` glyphs take the segment's own foreground, live on
/// `selected`, so they recolor with the label; `Element` is left as built.
fn render_adornment(
    adornment: &Adornment,
    selected: Memo<bool>,
    size: Reactive<SegmentedControlSize>,
    styles: &SegmentedControlSlotStyles,
) -> Vec<Element> {
    // The plain segment's on/off foreground, from style tokens (no sheet
    // resolve, so this also holds on a `--premint-only` build).
    let glyph_color = move || -> Option<Color> {
        let c = idea_theme::tokens().color;
        Some(if selected.get() { c.text() } else { c.text_muted() }.resolve())
    };
    let px = {
        let size = size.clone();
        move || segment_icon_px(size.get())
    };
    match adornment {
        Adornment::None => Vec::new(),
        Adornment::Element(build) => vec![build()],
        Adornment::Icon(data) => {
            vec![ui! { Icon(data = data.clone(), size = Reactive::Dynamic(Rc::new(px)), color = Reactive::Dynamic(Rc::new(glyph_color))) }]
        }
        Adornment::Button(data, on_press) => {
            let glyph = ui! { Icon(data = data.clone(), size = Reactive::Dynamic(Rc::new(px)), color = Reactive::Dynamic(Rc::new(glyph_color))) };
            let on_press = on_press.clone();
            // An icon-sized pressable inside the segment's: its recognizer
            // consumes the tap, so pressing it does not also select the
            // segment — the same "button in a clickable row" contract as
            // `TableRow`.
            vec![pressable(vec![glyph], move || on_press())
                .with_style(SegmentedControlStyles::adornment().overridden(styles.adornment.as_ref()))
                .into_element()]
        }
        Adornment::Group(items) => {
            items.iter().flat_map(|a| render_adornment(a, selected, size.clone(), styles)).collect()
        }
    }
}

recipe!(
    SegmentedControl,
    /// A controlled segmented picker. The host owns the `value` signal
    /// (the selected segment's `id`); `on_change` writes the picked id
    /// back. Build the segments with `SegmentOption::new(id, label)`.
    pub fn segmented_control_view_switch() -> ::runtime_core::Element {
        use crate::components::segmented_control::{SegmentOption, SegmentedControl};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let view = signal("list".to_string());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| view.set(v));
        ui! {
            SegmentedControl(
                value = view,
                on_change = on_change,
                options = vec![
                    SegmentOption::new("list", "List"),
                    SegmentOption::new("grid", "Grid"),
                    SegmentOption::new("map", "Map"),
                ],
            )
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, StyleApplication, StyleRules, StyleSheet, Tokenized};

    fn mount(props: SegmentedControlProps) -> (Rc<StyleRules>, Vec<Element>) {
        match classify(SegmentedControl(props)) {
            P::View { style, children, .. } => (style.expect("the track is styled").resolve(), children),
            _ => panic!("SegmentedControl renders a row View"),
        }
    }

    fn two(value: &str) -> SegmentedControlProps {
        SegmentedControlProps {
            options: vec![SegmentOption::new("a", "A"), SegmentOption::new("b", "B")],
            value: runtime_core::signal(value.to_string()).into(),
            ..Default::default()
        }
    }

    /// `(segment rules, label rules)` for one built segment.
    fn seg_parts(seg: Element) -> (Rc<StyleRules>, Rc<StyleRules>) {
        match classify(seg) {
            P::Pressable { style, mut children, .. } => {
                let label = match classify(children.remove(0)) {
                    P::Text { style, .. } => style.expect("label styled").resolve(),
                    _ => panic!("a bare segment's child is its label"),
                };
                (style.expect("segment styled").resolve(), label)
            }
            _ => panic!("a segment is a Pressable"),
        }
    }

    fn lit(hex: &str) -> Option<Tokenized<runtime_core::Color>> {
        Some(Tokenized::Literal(runtime_core::Color(hex.into())))
    }

    fn bg(r: &StyleRules) -> Option<runtime_core::Color> {
        r.background.as_ref().map(|c| c.resolve())
    }

    fn fg(r: &StyleRules) -> Option<runtime_core::Color> {
        r.color.as_ref().map(|c| c.resolve())
    }

    fn hex(h: &str) -> Option<runtime_core::Color> {
        Some(runtime_core::Color(h.into()))
    }

    fn reset() {
        runtime_core::component_styles::__reset::<SegmentedControlStyles>();
    }

    /// App-wide restyle through the slot registry: the track, the selected
    /// segment and its label repaint on EVERY control, unselected segments
    /// keep the theme, and the selected label's color is on the label's
    /// own node (native text doesn't inherit it).
    #[test]
    fn restyle_repaints_track_selected_segment_and_label_app_wide() {
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            SegmentedControlStyles::restyle(|s| {
                s.track().base(|_| StyleRules { background: lit("#111111"), ..Default::default() });
                s.segment().arm(SegmentedControlSelected::On, |_| StyleRules { background: lit("#222222"), ..Default::default() });
                s.label().arm(SegmentedControlSelected::On, |_| StyleRules { color: lit("#fafafa"), ..Default::default() });
            });
            let (track, mut segs) = mount(two("a"));
            assert_eq!(bg(&track), hex("#111111"));
            let (off, off_label) = seg_parts(segs.remove(1));
            let (on, on_label) = seg_parts(segs.remove(0));
            assert_eq!(bg(&on), hex("#222222"), "restyled selected fill");
            assert_ne!(bg(&off), hex("#222222"), "unselected keeps the theme");
            assert_eq!(fg(&on_label), hex("#fafafa"), "selected label color, on the label node");
            assert_ne!(fg(&off_label), hex("#fafafa"));
            // The restyled track still premints: the restyle is composed into
            // the slot sheet, not layered as a runtime override.
            assert!(SegmentedControlStyles::track().into_style_application().preminted_class_list().is_some());
            reset();
        });
    }

    /// The `styles` prop overrides one control's slots and wins over an
    /// app-wide restyle; a SHEET override follows the slot's selection, so
    /// it can style only the selected segment.
    #[test]
    fn styles_prop_overrides_one_control_over_the_restyle() {
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            SegmentedControlStyles::restyle(|s| {
                s.track().base(|_| StyleRules { background: lit("#111111"), ..Default::default() });
            });
            let selected_only = Rc::new(
                StyleSheet::new(|_| StyleRules::default())
                    .variant("selected", "on", |_| StyleRules { background: lit("#444444"), ..Default::default() })
                    .variant("selected", "off", |_| StyleRules::default()),
            );
            let (track, mut segs) = mount(SegmentedControlProps {
                styles: SegmentedControlSlotStyles::default()
                    .track(StyleRules { background: lit("#333333"), ..Default::default() })
                    .segment(selected_only),
                ..two("a")
            });
            assert_eq!(bg(&track), hex("#333333"), "the instance override wins over the restyle");
            let (off, _) = seg_parts(segs.remove(1));
            let (on, _) = seg_parts(segs.remove(0));
            assert_eq!(bg(&on), hex("#444444"));
            assert_ne!(bg(&off), hex("#444444"), "a sheet override follows the segment's selection");
            reset();
        });
    }

    /// An app-defined size: a value of the control's own size axis, given its
    /// look by a restyle arm on the segment and label slots.
    #[test]
    fn a_custom_size_value_draws_through_restyle_arms() {
        const XL: SegmentedControlSize = SegmentedControlSize::define("xl");
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            SegmentedControlStyles::restyle(|s| {
                s.segment().arm(XL, |_| StyleRules { padding_top: Some(Length::Px(14.0).into()), ..Default::default() });
                s.label().arm(XL, |_| StyleRules { font_size: Some(Length::Px(22.0).into()), ..Default::default() });
            });
            let (_, mut segs) = mount(SegmentedControlProps { size: Reactive::Static(XL), ..two("a") });
            let (seg, label) = seg_parts(segs.remove(0));
            assert_eq!(px(&seg.padding_top), 14.0);
            assert_eq!(px(&label.font_size), 22.0);
            reset();
        });
    }

    /// The label is its own slot: its application carries no computed or
    /// override layer, so it premints in every build (the old read-back of
    /// the segment's resolved color into a computed layer panicked under
    /// `--premint-only`).
    #[test]
    fn regression_premint_segment_label_application_premints() {
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            let (_, mut segs) = mount(two("a"));
            let label = match classify(segs.remove(0)) {
                P::Pressable { mut children, .. } => children.remove(0),
                _ => panic!("a segment is a Pressable"),
            };
            let style = match classify(label) {
                P::Text { style, .. } => style.expect("segment label carries a style"),
                _ => panic!("a segment label is a Text node"),
            };
            #[cfg(not(idealyst_premint))]
            assert!(style.application().preminted_class_list().is_some(), "the label slot premints");
            let _ = style;
        });
    }

    /// Regression: the segment label was a bare text node whose color lived
    /// only on the wrapping pressable, so on native (no CSS cascade) the
    /// selected segment never took the accent color. Each label carries its
    /// OWN color for its selected state.
    #[test]
    fn regression_segment_labels_carry_their_own_active_color() {
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            let c = idea_theme::tokens().color;
            let (_, mut segs) = mount(two("a"));
            let (_, off) = seg_parts(segs.remove(1));
            let (_, on) = seg_parts(segs.remove(0));
            assert_eq!(fg(&on), Some(c.text().resolve()));
            assert_eq!(fg(&off), Some(c.text_muted().resolve()));
        });
    }

    #[test]
    fn defaults_are_empty_and_inert() {
        with_test_world(|| {
            let p = SegmentedControlProps::default();
            assert!(p.options.is_empty());
            assert_eq!(p.value.get(), String::new());
            assert!(p.styles.track.is_none());
        });
    }

    /// Regression (CrewForge want_2ed95ee5): SegmentedControl drew with the
    /// Tabs sheets, so a value choice rendered as an underlined tab strip.
    /// It must be a bordered group whose selected segment is filled in, with
    /// no tab underline.
    #[test]
    fn regression_segmented_control_is_a_bordered_group_not_a_tab_strip() {
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            let (group, mut segs) = mount(two("a"));
            assert!(width(&group.border_left_width) > 0.0, "the group draws a border around all segments");
            assert!(group.background.is_some(), "the group is a tinted track");
            let (off, _) = seg_parts(segs.remove(1));
            let (on, _) = seg_parts(segs.remove(0));
            assert_ne!(bg(&on), bg(&off), "the selected segment is filled in");
            for seg in [&on, &off] {
                assert_eq!(width(&seg.border_bottom_width), width(&seg.border_left_width), "no tab underline");
            }
        });
    }

    /// The control's slots are its own, not the Tabs sheets: distinct premint
    /// identities, a `selected` axis (default `off`), and the hover / press /
    /// focus overlays — focus draws the themed ring.
    #[test]
    fn segmented_slots_are_their_own_not_the_tab_sheets() {
        use crate::TabsStyles;
        use runtime_core::StateBits;
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            for (ours, tabs, what) in [
                (SegmentedControlStyles::track().sheet(), TabsStyles::bar().sheet(), "track"),
                (SegmentedControlStyles::segment().sheet(), TabsStyles::tab().sheet(), "segment"),
            ] {
                assert_ne!(ours.premint_class(), tabs.premint_class(), "the {what} premints under its own class");
            }
            let seg = SegmentedControlStyles::segment().sheet();
            let axes: Vec<_> = seg.premint_author_axes().iter().map(|(a, d)| (a.as_str(), d.as_deref())).collect();
            assert_eq!(axes, vec![("block", Some("off")), ("selected", Some("off")), ("size", Some("md"))]);
            let states: Vec<StateBits> = seg.state_axes().iter().map(|(b, _)| *b).collect();
            for bit in [StateBits::HOVERED, StateBits::PRESSED, StateBits::FOCUSED] {
                assert!(states.contains(&bit));
            }
            let focused = resolve_style(&StyleApplication::new(seg).with("__state_focused", "on".to_string()));
            assert_eq!(focused.border_left_color.as_ref().and_then(|c| c.name()), Some("color-focus-ring"));
        });
    }

    /// Every themed value the track and segments paint is a style token, so a
    /// theme swap retints them. The selected segment is a raised key — the
    /// surface color with a border — on a `surface_alt` track.
    #[test]
    fn segmented_arms_are_built_from_style_tokens() {
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            let name = |c: &Option<Tokenized<runtime_core::Color>>| c.as_ref().and_then(|c| c.name());
            let group = resolve_style(&SegmentedControlStyles::track().into_style_application());
            assert_eq!(name(&group.background), Some("color-surface-alt"));
            assert_eq!(name(&group.border_left_color), Some("color-border"));
            assert!(group.border_top_left_radius.as_ref().and_then(|r| r.name()).is_some());
            assert_eq!(
                group.padding_left.as_ref().map(|p| p.resolve()),
                Some(Length::Px(SEGMENT_TRACK_INSET)),
                "the selected segment sits SEGMENT_TRACK_INSET inside the track"
            );
            let seg = |sel: SegmentedControlSelected| {
                resolve_style(&SegmentedControlStyles::segment().set(sel).into_style_application())
            };
            let (on, off) = (seg(SegmentedControlSelected::On), seg(SegmentedControlSelected::Off));
            assert_eq!(name(&on.background), Some("color-surface"));
            assert_eq!(name(&on.border_left_color), Some("color-border"));
            assert_eq!(name(&on.color), Some("color-text"));
            assert_eq!(name(&off.color), Some("color-text-muted"));
        });
    }

    /// One pressable segment per option, wrapped in a single row view.
    #[test]
    fn builds_one_segment_per_option() {
        with_test_world(|| {
            let (_, children) = mount(SegmentedControlProps {
                options: vec![SegmentOption::new("a", "A"), SegmentOption::new("b", "B"), SegmentOption::new("c", "C")],
                ..Default::default()
            });
            assert_eq!(children.len(), 3, "one segment per option");
            assert!(children.into_iter().all(|c| matches!(classify(c), P::Pressable { .. })));
        });
    }

    /// The segment's row: leading adornments, the label, trailing ones.
    fn segment_row(seg: Element) -> Vec<Element> {
        let inner = match classify(seg) {
            P::Pressable { mut children, .. } => children.remove(0),
            _ => panic!("a segment is a Pressable"),
        };
        match classify(inner) {
            P::View { children, .. } => children,
            _ => panic!("an adorned segment lays out a row view"),
        }
    }

    /// Adornments: the label is rendered ONCE by the control, between the
    /// leading and trailing adornments.
    #[test]
    fn adornments_sit_either_side_of_the_label() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let (_, mut segs) = mount(SegmentedControlProps {
                options: vec![SegmentOption::new("day", "Day shift")
                    .leading(Adornment::element(|| ui! { view() }))
                    .trailing(Adornment::Icon(crate::components::icon::EMPTY_ICON))],
                value: runtime_core::signal("day".to_string()).into(),
                ..Default::default()
            });
            let mut row = segment_row(segs.remove(0)).into_iter();
            assert!(matches!(classify(row.next().unwrap()), P::View { .. }), "leading element first");
            match classify(row.next().unwrap()) {
                P::Text { text, .. } => assert_eq!(text.as_deref(), Some("Day shift")),
                _ => panic!("the label sits between the adornments"),
            }
            assert!(matches!(classify(row.next().unwrap()), P::Icon { .. }), "trailing icon last");
            assert!(row.next().is_none());
        });
    }

    /// An `Icon` adornment recolors with its segment, like the label.
    #[test]
    fn icon_adornment_follows_the_selected_color() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let c = idea_theme::tokens().color;
            let (on, off) = (Some(c.text().resolve()), Some(c.text_muted().resolve()));
            for (picked, expect) in [("a", [on.clone(), off.clone()]), ("b", [off.clone(), on.clone()])] {
                let opt = |id: &str| SegmentOption::new(id, id).leading(Adornment::Icon(crate::components::icon::EMPTY_ICON));
                let (_, segs) = mount(SegmentedControlProps {
                    options: vec![opt("a"), opt("b")],
                    value: runtime_core::signal(picked.to_string()).into(),
                    ..Default::default()
                });
                let colors: Vec<Option<runtime_core::Color>> = segs
                    .into_iter()
                    .map(|seg| match classify(segment_row(seg).remove(0)) {
                        P::Icon { color, .. } => color,
                        _ => panic!("leading icon"),
                    })
                    .collect();
                assert_eq!(colors, expect.to_vec(), "picked {picked}");
            }
        });
    }

    /// No adornment keeps the bare label (no wrapper row).
    #[test]
    fn unadorned_segment_is_just_its_label() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let (_, mut segs) = mount(SegmentedControlProps { options: vec![SegmentOption::new("a", "A")], ..Default::default() });
            match classify(segs.remove(0)) {
                P::Pressable { mut children, .. } => assert!(matches!(classify(children.remove(0)), P::Text { .. })),
                _ => panic!("pressable"),
            }
        });
    }

    /// px of a resolved length, 0 when absent.
    fn px(l: &Option<Tokenized<Length>>) -> f32 {
        match l.as_ref().map(|t| t.resolve()) {
            Some(Length::Px(v)) => v,
            _ => 0.0,
        }
    }

    fn width(w: &Option<Tokenized<f32>>) -> f32 {
        w.as_ref().map_or(0.0, |w| w.resolve())
    }

    /// `size` makes the control the SAME HEIGHT as a Select at every size:
    /// the segmented control's vertical padding + borders (track border,
    /// track inset, segment border, segment padding) sum to Select's padding
    /// + border, and the fonts match.
    #[test]
    fn each_size_matches_a_select_of_the_same_size() {
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            let group = resolve_style(&SegmentedControlStyles::track().into_style_application());
            for (size, arm) in [(SegmentedControlSize::Sm, "sm"), (SegmentedControlSize::Md, "md"), (SegmentedControlSize::Lg, "lg")] {
                let seg = resolve_style(&SegmentedControlStyles::segment().set(size).into_style_application());
                let select = resolve_style(&crate::SelectStyles::trigger().set(crate::SelectSize::define(arm)).into_style_application());
                let seg_chrome = width(&group.border_top_width)
                    + width(&group.border_bottom_width)
                    + px(&group.padding_top)
                    + px(&group.padding_bottom)
                    + width(&seg.border_top_width)
                    + width(&seg.border_bottom_width)
                    + px(&seg.padding_top)
                    + px(&seg.padding_bottom);
                let select_chrome = width(&select.border_top_width)
                    + width(&select.border_bottom_width)
                    + px(&select.padding_top)
                    + px(&select.padding_bottom);
                assert_eq!(seg_chrome, select_chrome, "{arm}: same vertical chrome as Select");
                assert_eq!(px(&seg.font_size), px(&select.font_size), "{arm}: same font");
                assert_eq!(px(&seg.padding_left), px(&select.padding_left), "{arm}: same horizontal padding");
            }
        });
    }

    /// The label stamps the size's font (and weight) on its OWN text node:
    /// native text never inherits font from the segment.
    #[test]
    fn label_carries_the_size_font_on_its_own_node() {
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            for size in [ControlSize::Sm, ControlSize::Md, ControlSize::Lg] {
                let (_, mut segs) = mount(SegmentedControlProps { size: size.into(), ..two("a") });
                let (seg, label) = seg_parts(segs.remove(0));
                assert_eq!(px(&label.font_size), px(&seg.font_size), "{size:?} label font");
                assert_eq!(label.font_weight, seg.font_weight, "{size:?} label weight");
            }
        });
    }

    /// The control's `size` reaches the segments' style and their icon
    /// adornments (Field's 14/16/18 scale).
    #[test]
    fn size_reaches_segments_and_icon_adornments() {
        with_test_world(|| {
            reset();
            install_idea_theme(light_theme());
            for (size, icon_px) in [(ControlSize::Sm, 14.0), (ControlSize::Md, 16.0), (ControlSize::Lg, 18.0)] {
                let (_, mut segs) = mount(SegmentedControlProps {
                    options: vec![SegmentOption::new("a", "A").leading(Adornment::Icon(crate::components::icon::EMPTY_ICON))],
                    size: size.into(),
                    ..Default::default()
                });
                let (style, mut children) = match classify(segs.remove(0)) {
                    P::Pressable { style, children, .. } => (style.expect("styled"), children),
                    _ => panic!("pressable"),
                };
                let expect = resolve_style(&SegmentedControlStyles::segment().set(SegmentedControlSize::from(size)).into_style_application());
                assert_eq!(px(&style.resolve().padding_top), px(&expect.padding_top), "{size:?} segment");
                let row = match classify(children.remove(0)) {
                    P::View { children, .. } => children,
                    _ => panic!("adorned row"),
                };
                let icon_style = match classify(row.into_iter().next().unwrap()) {
                    P::Icon { style, .. } => style.expect("icon styled").resolve(),
                    _ => panic!("icon"),
                };
                assert_eq!(px(&icon_style.width), icon_px, "{size:?} icon size");
            }
        });
    }
}

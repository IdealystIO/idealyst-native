//! `Collapsible` — controlled expand/collapse with an animated body.
//! `Accordion` — data-driven group of Collapsibles with shared
//! "only one open at a time" coordination.
//!
//! ```ignore
//! // Standalone Collapsible
//! let open = signal(false);
//! ui! {
//!     Collapsible(
//!         title = "Advanced settings".into(),
//!         open = open,
//!         on_change = Rc::new(move |v| open.set(v)),
//!     ) {
//!         Stack(gap = StackGap::Md) {
//!             Field(label = Some("API key".into()), value = key, on_change = on_key)
//!             Switch(label = Some("Beta features".into()), value = beta, on_change = on_beta)
//!         }
//!     }
//! }
//!
//! // Accordion — single-open coordination
//! let active = signal(Some(0));
//! let on_change: Rc<dyn Fn(Option<usize>)> = Rc::new(move |v| active.set(v));
//! ui! {
//!     Accordion(
//!         active = active,
//!         on_change = on_change,
//!         items = vec![
//!             AccordionItem { title: "Shipping".into(), body: ui!{ /* ... */ } },
//!             AccordionItem { title: "Returns".into(),  body: ui!{ /* ... */ } },
//!         ],
//!     )
//! }
//! ```
//!
//! Both are controlled. The host owns the open-state signal so the
//! same pattern that drives Tabs / Field / Switch applies:
//! flipping the signal toggles which section is open, and
//! `on_change` fires when the user clicks a header.
//!
//! The body uses the framework's `presence` primitive — when `open`
//! flips, the body fades + slides into place (mount) or unmounts
//! after the exit animation. The page reflows accordingly; there's
//! no fake `max-height` animation hack.

use std::rc::Rc;

use runtime_core::stylesheet;
use runtime_core::{
    component, on_scope_drop, pressable, signal, text, ui, ChildList, Element,
    component_styles, AlignItems, FlexDirection, FontWeight, IdealystSchema, IntoElement,
    JustifyContent, LayoutSubscription, Length, Reactive, Ref, Signal, TextAlign, ViewHandle,
};
use idea_theme::compat::SignalModify as _;
use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use std::time::Duration;

use crate::theme::IdeaThemeRef;

// =============================================================================
// CollapsibleTransition + tunables
// =============================================================================

component_styles! {
    /// The styled sections of [`Collapsible`] (and of each [`Accordion`]
    /// item's header and body).
    pub Collapsible<IdeaThemeRef> {
        slot container {
            base(t) {
                background: t.color.surface(),
                border_top_width: 1.0,
                border_right_width: 1.0,
                border_bottom_width: 1.0,
                border_left_width: 1.0,
                border_top_color: t.color.border(),
                border_right_color: t.color.border(),
                border_bottom_color: t.color.border(),
                border_left_color: t.color.border(),
                border_top_left_radius: t.radius.lg(),
                border_top_right_radius: t.radius.lg(),
                border_bottom_left_radius: t.radius.lg(),
                border_bottom_right_radius: t.radius.lg(),
                flex_direction: FlexDirection::Column,
                overflow: runtime_core::Overflow::Hidden,
            }
            transitions {
                background: 250ms EaseInOut,
                border_top_color: 250ms EaseInOut,
            }
        }

        slot header {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                padding_vertical: t.spacing.md(),
                padding_horizontal: t.spacing.lg(),
                gap: t.spacing.sm(),
                color: t.color.text(),
                font_size: 14.0,
                font_weight: FontWeight::SemiBold,
                text_align: TextAlign::Left,
            }
            // Where the chevron sits. `end`: title left, chevron pushed right.
            // `start`: chevron then title, packed left — the tree convention.
            variant chevron {
                #[default]
                end(_t)   { justify_content: JustifyContent::SpaceBetween }
                start(_t) { justify_content: JustifyContent::FlexStart }
            }
            // `beside`: the header shares its row with a `trailing` element
            // (outside the toggle), so it takes the row's spare width.
            variant trailing {
                #[default]
                none(_t)   { }
                beside(_t) { flex_grow: 1.0, min_width: 0.0 }
            }
            state hovered(t) {
                background: t.color.surface_alt(),
            }
            // Focus mirrors hover (row wash) — replaces the native macOS ring.
            state focused(t) {
                background: t.color.surface_alt(),
            }
            transitions {
                background: 150ms EaseOut,
                color: 250ms EaseInOut,
            }
        }

        /// / A custom `header` element's box inside the toggle: takes the spare
        /// / width so a chevron at the end stays at the end, and may shrink.
        slot header_content {
            base(_t) {
                flex_grow: 1.0,
                min_width: 0.0,
            }
        }

        /// / The header row when a `trailing` element sits beside the toggle:
        /// / the toggle (growing) then the trailing element, which gets the
        /// / header's end padding since the toggle's padding no longer reaches
        /// / the row's edge.
        slot header_row {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.sm(),
                padding_right: t.spacing.lg(),
            }
        }

        slot chevron {
            base(t) {
                font_size: 13.0,
                color: t.color.text_muted(),
            }
            transitions {
                color: 250ms EaseInOut,
            }
        }

        /// Snap-mode body: state changes apply instantly. Cheap, predictable,
        /// no perceived animation. Pick this via `CollapsibleTransition::Snap`
        /// when the disclosure should feel like a single click → done.
        slot body {
            base(t) {
                flex_direction: FlexDirection::Column,
                padding_horizontal: t.spacing.lg(),
                border_top_width: 1.0,
                border_top_color: t.color.border(),
                gap: t.spacing.sm(),
                overflow: runtime_core::Overflow::Hidden,
            }
            variant open {
                #[default]
                closed(_t) {
                    max_height: Length::Px(0.0),
                    padding_top: Length::Px(0.0),
                    padding_bottom: Length::Px(0.0),
                    border_top_width: 0.0,
                }
                shown(t) {
                    max_height: Length::Px(2000.0),
                    padding_top: t.spacing.md(),
                    padding_bottom: t.spacing.md(),
                    border_top_width: 1.0,
                }
            }
            transitions {
                border_top_color: 250ms EaseInOut,
            }
        }

        /// Measured-mode body: the padded, bordered column holding the
        /// content. It sits in natural flow inside a clipping wrapper whose
        /// `max-height` an `AnimatedValue` drives (see `measured_body`), and
        /// it is the node whose height is measured — so its padding and
        /// border (restyled or not) are inside the measurement, and a closed
        /// body leaves no footprint because the wrapper clips it to zero.
        /// The `open` axis fades it in and out.
        slot body_animated {
            base(t) {
                flex_direction: FlexDirection::Column,
                padding_horizontal: t.spacing.lg(),
                padding_vertical: t.spacing.md(),
                border_top_width: 1.0,
                border_top_color: t.color.border(),
                gap: t.spacing.sm(),
            }
            variant open {
                #[default]
                closed(_t) { opacity: 0.0 }
                shown(_t) { opacity: 1.0 }
            }
            transitions {
                border_top_color: 250ms EaseInOut,
                opacity: 200ms EaseOut,
            }
        }
    }
}

/// Default duration of the open/close animation in milliseconds.
/// Used by [`CollapsibleTransition::Measured`] for the AV tween.
/// Override via `Collapsible.duration_ms`.
///
/// The body's fade (the `body_animated` slot's opacity transition) is a
/// class-level timing and does not follow a per-instance `duration_ms`.
pub const COLLAPSIBLE_DURATION_DEFAULT_MS: u32 = 240;

// The clipping wrapper around a measured body: its `max-height` is the
// animated value, and it clips the body below it. A `stylesheet!` so it
// premints from link time, crawled or not.
stylesheet! {
    CollapsibleClipSheet<()> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            overflow: runtime_core::Overflow::Hidden,
        }
    }
}

// The measured wrapper inside the clip: a plain column, no padding, so
// its measured height is its child's whole box.
stylesheet! {
    CollapsibleColumnSheet<()> {
        base(_t) { flex_direction: FlexDirection::Column }
    }
}

impl From<CollapsibleChevronSide> for CollapsibleChevron {
    fn from(side: CollapsibleChevronSide) -> Self {
        match side {
            CollapsibleChevronSide::Start => CollapsibleChevron::Start,
            CollapsibleChevronSide::End => CollapsibleChevron::End,
        }
    }
}

/// How a [`Collapsible`] (or [`Accordion`] item) animates between
/// open and closed states.
///
/// `Measured` is the default and the only smooth option — measures
/// the body's natural content height via [`ViewHandle::on_layout`]
/// (web `ResizeObserver` etc.) and tweens `AnimProp::MaxHeight` to
/// that exact value via the framework's animator. `Snap` is the
/// no-animation option for reduced-motion contexts.
///
/// Adding new transition flavors lands here as new enum variants
/// — components select stylesheets + animation strategies per
/// variant inside, no per-app CSS knowledge required.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema, runtime_core::Remote)]
pub enum CollapsibleTransition {
    /// No animation. State changes apply in one frame. Cheap and
    /// predictable; matches the `prefers-reduced-motion` user
    /// preference (which apps can wire to `Snap` themselves).
    Snap,
    /// **Measured** — the recommended default. Measures the body's
    /// natural content height via [`ViewHandle::on_layout`], then
    /// animates `AnimProp::MaxHeight` between `0 ↔ measured` via the
    /// framework's animator. No fixed cap — the open animation grows
    /// the body exactly as tall as its content. Works on every
    /// backend that supports `set_animated_f32` for `MaxHeight`
    /// (web today; iOS/Android pending native animation API).
    #[default]
    Measured,
}

/// Which end of a [`Collapsible`]'s header the disclosure chevron sits at.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema, runtime_core::Remote)]
pub enum CollapsibleChevronSide {
    /// Chevron first, then the header content — the tree / disclosure
    /// list convention.
    Start,
    /// Header content, then the chevron pushed to the end.
    #[default]
    End,
}

// =============================================================================
// Collapsible
// =============================================================================

// Reactive-by-default: `#[props]` wraps the scalar-DATA fields `transition`/
// `duration_ms` → `Reactive<…>`; `title` is already `Reactive`, `value` is a
// `Signal` source, `on_change` a handler, and `children` the children category
// — all auto-skipped. `transition`/`duration_ms` drive STRUCTURE/animation (the
// Snap-vs-Measured body branch and the tween timing inside an Effect), not a
// style sink, so they're snapshotted at build with flagged TODOs below.
/// Props for [`Collapsible`].
#[runtime_core::props(styles = CollapsibleStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct CollapsibleProps {
    /// Header text shown when both open and closed. `Reactive<String>` —
    /// a literal, a `Signal<String>`, or `rx!(...)` all work.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub title: Reactive<String>,
    /// Custom header content, in place of `title` — an icon, a name over a
    /// count. Inside the toggle: pressing it opens and closes the body.
    /// `None` renders `title`.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub header: Option<Element>,
    /// An element at the end of the header row, OUTSIDE the toggle, so its
    /// own presses don't open or close the body — an "open in new tab"
    /// `IconButton`, a menu trigger.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub trailing: Option<Element>,
    /// Which end of the header the chevron sits at. Default `End`.
    #[prop(static)]
    pub chevron_side: CollapsibleChevronSide,
    /// Controlled open state. Default `false`. The host should pass
    /// its own signal so external triggers (an "Expand all" button,
    /// a URL param) can drive it.
    pub value: Signal<bool>,
    /// Fires when the user clicks the header. Default is a no-op so
    /// an unwired Collapsible doesn't silently mutate; pass
    /// `Rc::new(move |v| value.set(v))` for the standard "click =
    /// toggle" wiring.
    pub on_change: Rc<dyn Fn(bool)>,
    /// How to animate the open/close — `Measured` (default) or
    /// `Snap`.
    pub transition: CollapsibleTransition,
    /// Duration of the open/close animation in milliseconds. Only
    /// meaningful when `transition = Measured` (Snap is instant).
    /// Default [`COLLAPSIBLE_DURATION_DEFAULT_MS`] (240). See the
    /// constant docs for the chrome-vs-AV timing caveat.
    #[schema(constraint = "milliseconds; keep near 240 to match baked chrome transitions")]
    pub duration_ms: u32,
    /// Body contents. Always mounted; visibility flows through the
    /// per-`transition` strategy (variant axis swap for `Snap`,
    /// `AnimProp::MaxHeight` tween for `Measured`).
    pub children: Vec<Element>,
}

impl Default for CollapsibleProps {
    fn default() -> Self {
        Self {
            title: Reactive::Static(String::new()),
            header: None,
            trailing: None,
            chevron_side: CollapsibleChevronSide::End,
            value: runtime_core::signal(false),
            on_change: Rc::new(|_| {}),
            transition: Reactive::Static(CollapsibleTransition::default()),
            duration_ms: Reactive::Static(COLLAPSIBLE_DURATION_DEFAULT_MS),
            children: Vec::new(),
            styles: Default::default(),
        }
    }
}

/// Renders a controlled disclosure section: a clickable header with an
/// animated chevron above a body that expands/collapses per `transition`.
#[component(children)]
pub fn Collapsible(props: CollapsibleProps) -> Element {
    let styles = props.styles;
    let container = CollapsibleStyles::container().overridden(styles.container.as_ref());
    let header = collapsible_header(
        HeaderContent::new(props.title, props.header),
        props.chevron_side,
        props.trailing,
        props.value,
        props.on_change,
        &styles,
    );
    // TODO(reactive-sweep): route `transition`/`duration_ms` into the body. Both
    // are STRUCTURAL/animation inputs — `transition` selects the Snap-vs-Measured
    // body construction (a tree-shape branch) and `duration_ms` feeds the tween
    // timing baked into the Measured effect — so neither rides a style sink.
    // Making them live needs the body rebuilt on `transition.get()` (a `switch`)
    // and the tween effect re-reading `duration_ms.get()` inside its closure.
    // For now both are snapshotted at build.
    let body = collapsible_body(
        props.value,
        props.transition.get(),
        props.duration_ms.get(),
        props.children,
        &styles,
    );
    ui! {
        view(style = container) {
            header
            body
        }
    }
}

/// What the header toggle shows besides the chevron: the `title` text, or
/// an author-built `header` element in its place.
enum HeaderContent {
    Title(Reactive<String>),
    Custom(Element),
}

impl HeaderContent {
    fn new(title: Reactive<String>, header: Option<Element>) -> Self {
        match header {
            Some(el) => HeaderContent::Custom(el),
            None => HeaderContent::Title(title),
        }
    }
}

/// The header: a toggle (content + animated chevron, on the side
/// `chevron_side` names) and, when given, a `trailing` element beside it.
/// Click toggles via `on_change` (which the host typically wires straight
/// back to the `value` signal). Hover highlights via the `:hovered` state on
/// the framework-side stylesheet.
///
/// `trailing` is a SIBLING of the toggle, not inside it: a pressable inside
/// a pressable is not a reliable press target on every backend (a button
/// within a button on web), and its press must not open the body.
fn collapsible_header(
    content: HeaderContent,
    chevron_side: CollapsibleChevronSide,
    trailing: Option<Element>,
    value: Signal<bool>,
    on_change: Rc<dyn Fn(bool)>,
    styles: &CollapsibleSlotStyles,
) -> Element {
    // The chevron flips from `›` (closed) to `⌄` (open). Only its TEXT
    // changes, so it is live text that updates in place — no subtree swap.
    let chevron_style = CollapsibleStyles::chevron().overridden(styles.chevron.as_ref());
    let chevron = ui! {
        text(style = chevron_style) {
            move || if value.get() { "\u{2304}" } else { "\u{203A}" }.to_string()
        }
    };

    // The title is a bare `text` straight in the toggle (the header sheet
    // carries its type); a custom header gets a growing box so a chevron at
    // the end stays at the end.
    let content = match content {
        HeaderContent::Title(title) => text(title).into_element(),
        HeaderContent::Custom(el) => {
            let content_style =
                CollapsibleStyles::header_content().overridden(styles.header_content.as_ref());
            ui! { view(style = content_style) { el } }
        }
    };
    // Received elements placed in order — `pressable` is a builder, not a
    // `ui!` tag.
    let row = match chevron_side {
        CollapsibleChevronSide::Start => vec![chevron, content],
        CollapsibleChevronSide::End => vec![content, chevron],
    };

    // One style for open and closed (the chevron carries the open
    // indicator), which keeps hover behaviour intact.
    let header_style = CollapsibleStyles::header()
        .set(CollapsibleChevron::from(chevron_side))
        .set(if trailing.is_some() { CollapsibleTrailing::Beside } else { CollapsibleTrailing::None })
        .overridden(styles.header.as_ref());

    let press_handler = move || {
        let next = !value.get();
        (on_change)(next);
    };

    let toggle = pressable(row, press_handler).with_style(header_style).into_element();

    match trailing {
        None => toggle,
        Some(trailing) => {
            let row_style = CollapsibleStyles::header_row().overridden(styles.header_row.as_ref());
            ui! {
                view(style = row_style) {
                    toggle
                    trailing
                }
            }
        }
    }
}

/// The collapsible body. Dispatches on `transition`:
/// - `Snap`: instantaneous show/hide via the `body` slot's `open` axis (max-height 0 ↔ shown is one
///   frame, no CSS transition declared).
/// - `Measured`: see [`measured_body`].
///
/// Why not `presence`: presence's body closure has a `Fn`-bound
/// signature, but `Vec<Element>` is single-move. Keeping the body
/// permanently mounted sidesteps the rebuild requirement and matches
/// how most disclosure widgets are implemented — the visible
/// animation flows through inline-style writes (Measured) or
/// variant axis swap (Snap) on a stable subtree.
fn collapsible_body(
    value: Signal<bool>,
    transition: CollapsibleTransition,
    duration_ms: u32,
    children: Vec<Element>,
    styles: &CollapsibleSlotStyles,
) -> Element {
    let mut kids: Vec<Element> = Vec::with_capacity(children.len());
    for c in children {
        ChildList::append_to(c, &mut kids);
    }

    match transition {
        CollapsibleTransition::Snap => {
            let style = CollapsibleStyles::body()
                .set(open_axis(value))
                .overridden(styles.body.as_ref());
            ui! { view(style = style) { kids } }
        }
        CollapsibleTransition::Measured => measured_body(value, duration_ms, kids, styles),
    }
}

/// The `open` axis value tracking `value` live.
fn open_axis(value: Signal<bool>) -> Reactive<CollapsibleOpen> {
    Reactive::Dynamic(Rc::new(move || {
        if value.get() {
            CollapsibleOpen::Shown
        } else {
            CollapsibleOpen::Closed
        }
    }))
}

/// Body for `CollapsibleTransition::Measured`. The wiring:
///
/// 1. The clipping wrapper (bound to `body_ref`) is what we animate — its
///    `max_height` is driven by `av: AnimatedValue<f32>` via
///    `AnimProp::MaxHeight`, and it clips everything below.
/// 2. Inside it, a plain measured wrapper (bound to `content_ref`) in
///    natural flow, so its layout reports its full height whatever the
///    clamp. A `LayoutSubscription` on it captures that height into
///    `natural_height` — re-firing on content changes.
/// 3. Inside that, the padded, bordered body (the `body_animated` slot)
///    with the content. The measured wrapper has no padding of its own:
///    its height is the body's whole box, padding and border included,
///    on every backend (web's `ResizeObserver` reports a content box,
///    native frames a border box — the same number for an unpadded
///    node). So a restyled body padding stays inside the target.
/// 4. An `Effect` watches `value` and `natural_height` and tweens `av`
///    to `natural_height` when open, `0.0` when closed.
fn measured_body(
    value: Signal<bool>,
    duration_ms: u32,
    kids: Vec<Element>,
    styles: &CollapsibleSlotStyles,
) -> Element {
    // Per-instance signals + handles.
    let natural_height: Signal<f32> = signal(0.0);
    let body_ref: Ref<ViewHandle> = Ref::new();
    let content_ref: Ref<ViewHandle> = Ref::new();

    // AnimatedValue drives the outer view's `max_height` via the
    // framework's per-frame writer. We don't need the result —
    // `bind` itself anchors the binding into the active scope.
    let av: AnimatedValue<f32> = AnimatedValue::new(0.0);
    av.bind(body_ref, AnimProp::MaxHeight);

    // Capture the layout subscription into a Rc<RefCell> so its
    // lifetime is the closure's, not the Signal's (LayoutSubscription
    // isn't Clone — it owns the unsubscribe closure).
    //
    // Setup is deferred to `after_animation_frame` because `content_ref`
    // isn't filled until after the framework's mount pass completes —
    // an Effect created here runs synchronously during render, before
    // the ref is populated. The next-frame callback runs after mount
    // (and after the first paint), at which point `content_ref.with`
    // returns the filled ViewHandle and we can register the
    // ResizeObserver against the real DOM element.
    let layout_sub_holder: std::rc::Rc<std::cell::RefCell<Option<LayoutSubscription>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let holder_for_setup = layout_sub_holder.clone();
    let setup_task = runtime_core::after_animation_frame(move || {
        let sub_opt = content_ref.with(|h| {
            h.on_layout(move |_w, h| {
                if (natural_height.get() - h).abs() > 0.5 {
                    natural_height.set(h);
                }
            })
        });
        if let Some(sub) = sub_opt {
            *holder_for_setup.borrow_mut() = Some(sub);
        }
    });
    // Both must outlive the function return but MUST be torn down when
    // this component's scope drops — NOT leaked. `mem::forget` here was a
    // bug: it kept the `ResizeObserver` (the `LayoutSubscription`) alive
    // forever, so after the scope was disposed (e.g. a web history-pop
    // detaching this subtree) a late layout callback still fired and read
    // `natural_height` after its `Signal<f32>` slot was freed —
    // "signal used after its scope was dropped" → abort. Anchoring to the
    // scope drops the ScheduledTask (cancels a not-yet-run setup) and the
    // subscription (unsubscribes the observer) during scope teardown,
    // before the scope's signals are freed.
    //
    // `on_scope_drop`, NOT `on_cleanup`: this is a component body, and
    // `on_cleanup` panics unless an effect is running. It happened to
    // survive when a reactive re-render swapped this subtree in (the swap
    // runs inside an effect) and aborted on a direct mount — so
    // deep-linking straight to a page containing a measured Collapsible
    // took the app down.
    on_scope_drop(move || {
        drop(setup_task);
        drop(layout_sub_holder);
    });

    // Toggle effect: kick a TweenTo on `av` whenever `value` or
    // `natural_height` flips. Reading both inside the effect closure
    // subscribes to both — value changes trigger an open/close
    // animation; height changes mid-open (content grew/shrunk)
    // re-tween to the new target.
    runtime_core::effect!({
        let open = value.get();
        // The measured node is the padded body itself, so its height is
        // the whole target — padding and border included.
        let target = if open { natural_height.get() } else { 0.0 };
        // `TweenTo` from the framework animation system — handles
        // raf loop, easing, cancellation.
        av.animate(TweenTo::new(target, Duration::from_millis(duration_ms as u64)).ease_out());
    });
    // Scope-adopted: the reactive scope owns this effect and frees it on
    // teardown (the `_toggle` handle's drop is a no-op). No `mem::forget`.

    // clip (`body_ref`) > measured wrapper (`content_ref`) > padded body —
    // see the wiring above. The body's `open` axis fades it. The view()
    // builder chains `.bind(ref)`; the `ui!` macro has no `ref =` prop.
    let body_style = CollapsibleStyles::body_animated()
        .set(open_axis(value))
        .overridden(styles.body_animated.as_ref());
    let body = runtime_core::view(kids).with_style(body_style).into_element();
    let measured = runtime_core::view(vec![body])
        .with_style(CollapsibleColumnSheet::sheet())
        .bind(content_ref)
        .into_element();
    runtime_core::view(vec![measured])
        .with_style(CollapsibleClipSheet::sheet())
        .bind(body_ref)
        .into_element()
}

// =============================================================================
// Accordion
// =============================================================================

component_styles! {
    /// The styled sections of [`Accordion`].
    pub Accordion<IdeaThemeRef> {
        /// Accordion item — same header/body shape as a Collapsible, but
        /// without the per-item border/radius (the Accordion container owns
        /// those, and items just contribute internal dividers).
        slot container {
            base(t) {
                background: t.color.surface(),
                border_top_width: 1.0,
                border_right_width: 1.0,
                border_bottom_width: 1.0,
                border_left_width: 1.0,
                border_top_color: t.color.border(),
                border_right_color: t.color.border(),
                border_bottom_color: t.color.border(),
                border_left_color: t.color.border(),
                border_top_left_radius: t.radius.lg(),
                border_top_right_radius: t.radius.lg(),
                border_bottom_left_radius: t.radius.lg(),
                border_bottom_right_radius: t.radius.lg(),
                flex_direction: FlexDirection::Column,
                overflow: runtime_core::Overflow::Hidden,
            }
            transitions {
                background: 250ms EaseInOut,
                border_top_color: 250ms EaseInOut,
            }
        }

        /// Per-item divider — top border on items 1..N so the first item has
        /// no top border and the rest separate cleanly.
        slot separator {
            base(t) {
                flex_direction: FlexDirection::Column,
                border_top_width: 1.0,
                border_top_color: t.color.border(),
            }
            transitions {
                border_top_color: 250ms EaseInOut,
            }
        }
    }
}

/// Expansion policy for an [`Accordion`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, IdealystSchema, runtime_core::Remote)]
pub enum AccordionExpand {
    /// Only one item open at a time. Opening item `i` closes any
    /// previously-open one; clicking the already-open item closes
    /// it (all collapsed).
    #[default]
    Single,
    /// Any subset of items can be open. Each click independently
    /// toggles that one item's state without touching the others.
    Multi,
}

/// One item in an [`Accordion`]. Constructed inline at the call site:
/// `AccordionItem { title: "Shipping".into(), body: ui!{ ... } }`.
#[derive(IdealystSchema, runtime_core::Remote)]
pub struct AccordionItem {
    /// Header text for this item. `Reactive<String>` — static or live.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub title: Reactive<String>,
    /// The item's collapsible body content.
    pub body: Element,
}

// Reactive-by-default: `#[props]` wraps the scalar-DATA fields `expand`/
// `transition`/`duration_ms` → `Reactive<…>`; `items` (children category),
// `open` (Signal source), and `on_change` (Option<handler>) are auto-skipped.
// All three wrapped props drive STRUCTURE — they're consumed in the per-item
// build loop (expansion policy, body strategy, tween timing) — so they're
// snapshotted at build with a flagged TODO in the body.
/// Props for [`Accordion`].
#[runtime_core::props(styles = AccordionStyles)]
#[derive(IdealystSchema)]
pub struct AccordionProps {
    /// Items rendered in order.
    pub items: Vec<AccordionItem>,
    /// Per-item open state, parallel to `items` (`open.get()[i]` is
    /// `true` ⇔ item `i` is expanded). Default: empty vec (the
    /// Accordion auto-fills with `false`s to match `items.len()` on
    /// the first interaction).
    ///
    /// The same signal shape covers both [`AccordionExpand::Single`]
    /// and [`AccordionExpand::Multi`]; the difference is how the
    /// component mutates it on click. Both modes keep the signal
    /// fully observable from the host.
    pub open: Signal<Vec<bool>>,
    /// Expansion policy — single (only one open) or multi (any
    /// subset). Default: [`AccordionExpand::Single`].
    pub expand: AccordionExpand,
    /// How each item animates between open and closed. Default:
    /// [`CollapsibleTransition::Measured`].
    pub transition: CollapsibleTransition,
    /// Duration of each item's open/close animation in milliseconds.
    /// Forwarded to every item's underlying Collapsible. Default
    /// [`COLLAPSIBLE_DURATION_DEFAULT_MS`] (240). See
    /// [`CollapsibleProps::duration_ms`].
    #[schema(constraint = "milliseconds; keep near 240 to match baked chrome transitions")]
    pub duration_ms: u32,
    /// Fires after the Accordion mutates `open` in response to a
    /// click. Receives the index that was clicked and that item's
    /// new state. Optional — the Accordion already wrote the change
    /// to `open` by the time this fires; the callback is for
    /// observation (analytics, persisting to local storage, …).
    pub on_change: Option<Rc<dyn Fn(usize, bool)>>,
}

impl Default for AccordionProps {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            open: runtime_core::signal(Vec::new()),
            expand: Reactive::Static(AccordionExpand::default()),
            transition: Reactive::Static(CollapsibleTransition::default()),
            duration_ms: Reactive::Static(COLLAPSIBLE_DURATION_DEFAULT_MS),
            on_change: None,
            styles: Default::default(),
        }
    }
}

/// Renders a vertical stack of [`Collapsible`] sections from `items`,
/// coordinating their open state per the `expand` policy (single- or
/// multi-open) and reporting clicks via `on_change`.
#[component]
pub fn Accordion(props: AccordionProps) -> Element {
    let container_style = AccordionStyles::container().overridden(props.styles.container.as_ref());
    // Items draw the Collapsible header and body slots (restyled app-wide
    // through `CollapsibleStyles`).
    let item_styles = CollapsibleSlotStyles::default();
    let open_state = props.open;
    // TODO(reactive-sweep): route `expand`/`transition`/`duration_ms` into the
    // per-item build. All three are STRUCTURAL — consumed in the item loop below
    // to pick the expansion policy, the body strategy, and the tween timing — so
    // a live source would need the item subtree rebuilt (a `switch`), not a style
    // sink. For now each is snapshotted at build.
    let expand = props.expand.get();
    let transition = props.transition.get();
    let duration_ms = props.duration_ms.get();
    let on_change = props.on_change;
    let n = props.items.len();

    // Ensure `open` has the right length. Bare `signal(vec![false; N])`
    // is the canonical caller convention; this top-up handles the
    // default-empty case gracefully without panicking on out-of-bounds.
    let current_len = open_state.get().len();
    if current_len < n {
        open_state.modify(|v| v.resize(n, false));
    }

    let mut item_views: Vec<Element> = Vec::with_capacity(props.items.len());
    for (idx, item) in props.items.into_iter().enumerate() {
        // Per-item open signal — derived from the host's open-vec
        // signal so the Collapsible body sees a simple `Signal<bool>`.
        // We use a fresh `Signal<bool>` initialized from the current
        // open-vec entry, kept in sync via a one-way effect that
        // reads the vec and writes the per-item signal.
        let item_open: Signal<bool> =
            signal(open_state.get().get(idx).copied().unwrap_or(false));
        // Scope-adopted: the Accordion's reactive scope owns this sync
        // effect and frees it on teardown (the handle drop is a no-op).
        // No `mem::forget` (a leak outside framework core).
        runtime_core::effect!({
            let now = open_state.get().get(idx).copied().unwrap_or(false);
            if item_open.get() != now {
                item_open.set(now);
            }
        });

        // Per-item on_change: mutate the host's open-vec according to
        // `expand` mode, then fire the observation callback.
        let on_change_for_item = on_change.clone();
        let item_on_change: Rc<dyn Fn(bool)> = Rc::new(move |next_open: bool| {
            open_state.modify(|v| match expand {
                AccordionExpand::Single => {
                    // Clear everything else; set only this item.
                    for entry in v.iter_mut() {
                        *entry = false;
                    }
                    if let Some(slot) = v.get_mut(idx) {
                        *slot = next_open;
                    }
                }
                AccordionExpand::Multi => {
                    if let Some(slot) = v.get_mut(idx) {
                        *slot = next_open;
                    }
                }
            });
            if let Some(cb) = &on_change_for_item {
                (cb)(idx, next_open);
            }
        });

        // Build the collapsible header + body in a wrapper that
        // contributes the inter-item divider when not first.
        let header = collapsible_header(
            HeaderContent::Title(item.title),
            CollapsibleChevronSide::End,
            None,
            item_open,
            item_on_change,
            &item_styles,
        );
        let body =
            collapsible_body(item_open, transition, duration_ms, vec![item.body], &item_styles);

        let item_block = if idx == 0 {
            // First item: no top divider; the container's own border
            // takes care of the top edge.
            ui! { view { header body } }
        } else {
            let sep = AccordionStyles::separator().overridden(props.styles.separator.as_ref());
            ui! { view(style = sep) { header body } }
        };
        item_views.push(item_block);
    }

    ui! {
        view(style = container_style) { item_views }
    }
}

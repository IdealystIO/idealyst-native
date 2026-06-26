//! `Collapsible` — controlled expand/collapse with an animated body.
//! `Accordion` — data-driven group of Collapsibles with shared
//! "only one open at a time" coordination.
//!
//! ```ignore
//! // Standalone Collapsible
//! let open = signal!(false);
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
//! let active = signal!(Some(0));
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

use runtime_core::{
    component, derived, on_cleanup, pressable, signal, switch, text, ui, ChildList, Element,
    IdealystSchema, IntoElement, LayoutSubscription, Reactive, Ref, Signal, StyleApplication,
    VariantEnum, ViewHandle,
};
use runtime_core::animation::{AnimProp, AnimatedValue, TweenTo};
use std::time::Duration;

use crate::stylesheets::{
    AccordionContainer, AccordionItemSeparator, CollapsibleBody, CollapsibleBodyAnimated,
    CollapsibleBodyAnimatedOpen, CollapsibleBodyOpen, CollapsibleChevron, CollapsibleContainer,
    CollapsibleHeader,
};

// =============================================================================
// CollapsibleTransition + tunables
// =============================================================================

/// Default duration of the open/close animation in milliseconds.
/// Used by [`CollapsibleTransition::Measured`] for the AV tween.
/// Override via `Collapsible.duration_ms`.
///
/// Note: chrome transitions (padding, opacity, border-color) on the
/// underlying stylesheet are baked into CSS at the framework's
/// compile time — they don't currently track this constant per-
/// instance. If you set `duration_ms` far from 240, the chrome will
/// finish ahead of (or after) the height animation. The "right" fix
/// is per-instance transition-timing overrides on
/// `StyleApplication`; until that lands, keep `duration_ms` close to
/// the default 240 to avoid visible mismatch.
pub const COLLAPSIBLE_DURATION_DEFAULT_MS: u32 = 240;

/// Total vertical chrome (padding + border) on a Measured body when
/// shown. Used to translate the inner content's measured height into
/// the outer's target max-height — the framework forces
/// `box-sizing: border-box`, so the outer's max-height has to cover
/// content + chrome to avoid clipping.
///
/// Mirrors the `CollapsibleBodyAnimated` `shown` variant:
/// `padding_top: spacing-md (12) + padding_bottom: spacing-md (12) +
/// border_top_width: 1 = 25`. If the stylesheet's chrome changes,
/// bump this in lockstep.
const MEASURED_CHROME_PX: f32 = 25.0;

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
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema)]
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
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct CollapsibleProps {
    /// Header text shown when both open and closed. `Reactive<String>` —
    /// a literal, a `Signal<String>`, or `rx!(...)` all work.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub title: Reactive<String>,
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
            value: Signal::new(false),
            on_change: Rc::new(|_| {}),
            transition: Reactive::Static(CollapsibleTransition::default()),
            duration_ms: Reactive::Static(COLLAPSIBLE_DURATION_DEFAULT_MS),
            children: Vec::new(),
        }
    }
}

/// Renders a controlled disclosure section: a clickable header with an
/// animated chevron above a body that expands/collapses per `transition`.
#[component(children)]
pub fn Collapsible(props: CollapsibleProps) -> Element {
    let container = CollapsibleContainer();
    let header = collapsible_header(props.title, props.value, props.on_change);
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
    );
    ui! {
        view(style = container) {
            header
            body
        }
    }
}

/// The clickable header — title on the left, animated chevron on the
/// right. Click toggles via `on_change` (which the host typically
/// wires straight back to the `value` signal). Hover highlights via
/// the `:hovered` state on the framework-side stylesheet.
fn collapsible_header(
    title: Reactive<String>,
    value: Signal<bool>,
    on_change: Rc<dyn Fn(bool)>,
) -> Element {
    // Reactive style for the chevron glyph — re-derives when `value`
    // changes so the indicator flips from `›` (closed) to `⌄` (open).
    let chevron = switch(
        move || value.get(),
        |&open| {
            let style = move || StyleApplication::new(CollapsibleChevron::sheet());
            let glyph = if open { "\u{2304}" } else { "\u{203A}" }.to_string();
            text(glyph).with_style(style).into_element()
        },
    );

    // Header style is shared between open/closed states (the chevron
    // carries the open indicator). One reactive style closure keeps
    // hover behavior intact.
    let header_style = || StyleApplication::new(CollapsibleHeader::sheet());

    let title_text = text(title).into_element();
    let row = vec![title_text, chevron];

    let press_handler = move || {
        let next = !value.get();
        (on_change)(next);
    };

    pressable(row, press_handler)
        .with_style(header_style)
        .into_element()
}

/// The collapsible body. Dispatches on `transition`:
/// - `Snap`: instantaneous show/hide via the `CollapsibleBody`
///   stylesheet's `open` variant axis (max-height 0 ↔ shown is one
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
) -> Element {
    let mut kids: Vec<Element> = Vec::with_capacity(children.len());
    for c in children {
        ChildList::append_to(c, &mut kids);
    }

    match transition {
        CollapsibleTransition::Snap => {
            let style = CollapsibleBody().open(derived(move || {
                if value.get() {
                    CollapsibleBodyOpen::Shown
                } else {
                    CollapsibleBodyOpen::Closed
                }
            }));
            ui! { view(style = style) { kids } }
        }
        CollapsibleTransition::Measured => measured_body(value, duration_ms, kids),
    }
}

/// Body for `CollapsibleTransition::Measured`. The wiring:
///
/// 1. Outer `view` (bound to `body_ref`) is what we animate — its
///    `max_height` is driven by `av: AnimatedValue<f32>` via
///    `AnimProp::MaxHeight`.
/// 2. Inner `view` (bound to `content_ref`) holds the actual content.
///    A `LayoutSubscription` on this inner view captures the natural
///    content height into `natural_height: Signal<f32>` — re-fires on
///    content changes so the target always tracks reality.
/// 3. An `Effect` watches both `value` and `natural_height` and
///    triggers a `TweenTo` on `av` whenever they change: target =
///    `natural_height + MEASURED_CHROME_PX` when open, `0.0` when
///    closed. The chrome offset compensates for `box-sizing:
///    border-box` (framework universal) — without it, max-height
///    would clip the bottom of content by padding + border height.
///
/// Why two views: the outer one carries the `overflow: hidden` + the
/// animated max-height. The inner one is in the natural-flow layout
/// so `ResizeObserver` (or platform equivalent) sees its full height
/// regardless of the outer's max-height clamp.
fn measured_body(value: Signal<bool>, duration_ms: u32, kids: Vec<Element>) -> Element {
    // Per-instance signals + handles.
    let natural_height: Signal<f32> = signal!(0.0);
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
    // scope via `on_cleanup` drops the ScheduledTask (cancels a not-yet-run
    // setup) and the subscription (unsubscribes the observer) during scope
    // teardown, before the scope's signals are freed.
    on_cleanup(move || {
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
        // Add chrome offset to the measured content height — the
        // outer's `box-sizing: border-box` (framework universal)
        // means max-height covers padding + border + content. The
        // ResizeObserver on the inner reports only content height,
        // so we'd clip the bottom of content by the chrome amount
        // without this addition.
        let target = if open {
            let nh = natural_height.get();
            if nh > 0.0 { nh + MEASURED_CHROME_PX } else { 0.0 }
        } else {
            0.0
        };
        // `TweenTo` from the framework animation system — handles
        // raf loop, easing, cancellation. The chrome transitions
        // (padding, opacity, border) on `CollapsibleBodyAnimated`
        // are baked at 200–240ms; keep `duration_ms` near 240 to
        // avoid the chrome finishing visibly out of sync with the
        // height.
        av.animate(TweenTo::new(target, Duration::from_millis(duration_ms as u64)).ease_out());
    });
    // Scope-adopted: the reactive scope owns this effect and frees it on
    // teardown (the `_toggle` handle's drop is a no-op). No `mem::forget`.

    // Outer view — `body_ref` binds for AnimatedValue (drives
    // max-height per frame). The variant axis switches between
    // `closed` and `shown` based on `value`, so padding / opacity /
    // border-top CSS-transition between the variants' values via
    // the declared transitions on `CollapsibleBodyAnimated`.
    //
    // Division of labor:
    //   - max-height: driven by AV (animator-precise to natural height)
    //   - padding-top/bottom, opacity, border-top: CSS transitions
    //     on variant swap (handled by the browser)
    //
    // Both run concurrently over ~240ms — the body's chrome
    // (padding, border) collapses with the height, so closed items
    // leave zero visible footprint.
    let outer_style = move || {
        let variant = if value.get() {
            CollapsibleBodyAnimatedOpen::Shown
        } else {
            CollapsibleBodyAnimatedOpen::Closed
        };
        StyleApplication::new(CollapsibleBodyAnimated::sheet())
            .with("open", variant.as_variant_str().to_string())
    };

    // Inner view is in natural flow — its layout reports the
    // intrinsic content height regardless of the outer's clamp.
    // Use the framework's view() builder to chain `.bind(ref)` —
    // the `ui!` macro doesn't expose a `ref =` prop.
    let inner = runtime_core::view(kids).bind(content_ref).into_element();
    runtime_core::view(vec![inner])
        .with_style(outer_style)
        .bind(body_ref)
        .into_element()
}

// =============================================================================
// Accordion
// =============================================================================

/// Expansion policy for an [`Accordion`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, IdealystSchema)]
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
#[derive(IdealystSchema)]
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
#[runtime_core::props]
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
            open: Signal::new(Vec::new()),
            expand: Reactive::Static(AccordionExpand::default()),
            transition: Reactive::Static(CollapsibleTransition::default()),
            duration_ms: Reactive::Static(COLLAPSIBLE_DURATION_DEFAULT_MS),
            on_change: None,
        }
    }
}

/// Renders a vertical stack of [`Collapsible`] sections from `items`,
/// coordinating their open state per the `expand` policy (single- or
/// multi-open) and reporting clicks via `on_change`.
#[component]
pub fn Accordion(props: AccordionProps) -> Element {
    let container_style = AccordionContainer();
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

    // Ensure `open` has the right length. Bare `signal!(vec![false; N])`
    // is the canonical caller convention; this top-up handles the
    // default-empty case gracefully without panicking on out-of-bounds.
    let current_len = open_state.get().len();
    if current_len < n {
        open_state.update(|v| v.resize(n, false));
    }

    let mut item_views: Vec<Element> = Vec::with_capacity(props.items.len());
    for (idx, item) in props.items.into_iter().enumerate() {
        // Per-item open signal — derived from the host's open-vec
        // signal so the Collapsible body sees a simple `Signal<bool>`.
        // We use a fresh `Signal<bool>` initialized from the current
        // open-vec entry, kept in sync via a one-way effect that
        // reads the vec and writes the per-item signal.
        let item_open: Signal<bool> =
            signal!(open_state.get().get(idx).copied().unwrap_or(false));
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
            open_state.update(|v| match expand {
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
        let header = collapsible_header(item.title, item_open, item_on_change);
        let body = collapsible_body(item_open, transition, duration_ms, vec![item.body]);

        let item_block = if idx == 0 {
            // First item: no top divider; the container's own border
            // takes care of the top edge.
            ui! { view { header body } }
        } else {
            let sep = AccordionItemSeparator();
            ui! { view(style = sep) { header body } }
        };
        item_views.push(item_block);
    }

    ui! {
        view(style = container_style) { item_views }
    }
}

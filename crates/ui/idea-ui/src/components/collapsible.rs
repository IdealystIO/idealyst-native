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
    component, derived, pressable, signal, switch, text, ui, ChildList, Element, IntoElement,
    Length, Reactive, Signal, StyleApplication, Tokenized, VariantEnum,
};

use crate::stylesheets::{
    AccordionContainer, AccordionItemSeparator, CollapsibleBody, CollapsibleBodyOpen,
    CollapsibleBodySmooth, CollapsibleBodySmoothOpen, CollapsibleChevron, CollapsibleContainer,
    CollapsibleHeader,
};

// =============================================================================
// CollapsibleTransition + tunables
// =============================================================================

/// Default cap for the Smooth transition's `max-height` animation in
/// pixels. CSS can't transition `height: auto`, so we animate
/// `max-height` to a fixed value — the visible portion of the
/// transition is `content-height / max-height` of the duration.
/// Authors with taller content set their own via `Collapsible.max_height`.
pub const SMOOTH_MAX_HEIGHT_DEFAULT_PX: f32 = 400.0;

/// How a [`Collapsible`] (or [`Accordion`] item) animates between
/// open and closed states.
///
/// `Smooth` is the default. Pick `Snap` when motion is wrong for the
/// surface (a dense data tree, an oncall dashboard, a reduced-motion
/// preference). Adding new transition flavors here is the supported
/// extensibility seam — components select stylesheets per variant
/// inside, no per-app CSS knowledge required.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum CollapsibleTransition {
    /// Animated `max-height` + `opacity` + `padding` over ~240ms. The
    /// content grows smoothly into place. Content taller than the
    /// stylesheet's `max-height` cap (2000px) grows smoothly to the
    /// cap then snaps the remainder.
    #[default]
    Smooth,
    /// No transition. State changes apply in one frame. Cheap and
    /// predictable; matches the `prefers-reduced-motion` user
    /// preference (which apps can wire to `Snap` themselves).
    Snap,
}

// =============================================================================
// Collapsible
// =============================================================================

/// Props for [`Collapsible`].
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct CollapsibleProps {
    /// Header text shown when both open and closed. `Reactive<String>` —
    /// a literal, a `Signal<String>`, or `rx!(...)` all work.
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
    /// How to animate the open/close — `Smooth` (default) or `Snap`.
    pub transition: CollapsibleTransition,
    /// Smooth-transition max-height cap in pixels. Only meaningful
    /// when `transition = Smooth`. Default
    /// [`SMOOTH_MAX_HEIGHT_DEFAULT_PX`] (400). Tune up for taller
    /// content — the visible portion of the open animation is
    /// `content-height / max_height` of the duration, so a cap close
    /// to actual content height feels smoothest. Content taller than
    /// the cap clips during the transition then stretches at the
    /// end.
    pub max_height: f32,
    /// Body contents. Always mounted; visibility flows through the
    /// stylesheet variant axis selected by `transition`.
    pub children: Vec<Element>,
}

impl Default for CollapsibleProps {
    fn default() -> Self {
        Self {
            title: Reactive::Static(String::new()),
            value: Signal::new(false),
            on_change: Rc::new(|_| {}),
            transition: CollapsibleTransition::default(),
            max_height: SMOOTH_MAX_HEIGHT_DEFAULT_PX,
            children: Vec::new(),
        }
    }
}

#[component(children)]
pub fn Collapsible(props: CollapsibleProps) -> Element {
    let container = CollapsibleContainer();
    let header = collapsible_header(props.title, props.value, props.on_change);
    let body = collapsible_body(props.value, props.transition, props.max_height, props.children);
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

/// The collapsible body — always mounted; visibility is driven by the
/// stylesheet's `open` variant axis (`closed` vs `shown`). The browser
/// transitions opacity + padding when the variant flips. The
/// `max_height` snap is instant (the framework's transition vocabulary
/// doesn't cover `height: auto`); padding + opacity carry the visible
/// animation.
///
/// Why not `presence`: presence's body closure has a `Fn`-bound
/// signature, but `Vec<Element>` is single-move. Keeping the DOM
/// permanently mounted sidesteps the rebuild requirement and matches
/// how most disclosure widgets are implemented on the web — the
/// browser already handles the per-property transition.
///
/// The `max_height: 2000px` cap in the `shown` variant covers typical
/// section content; very tall bodies would clip past that. Bumping it
/// is a one-line edit on the stylesheet.
fn collapsible_body(
    value: Signal<bool>,
    transition: CollapsibleTransition,
    max_height: f32,
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
        CollapsibleTransition::Smooth => {
            // Closure form so we can layer a per-instance
            // `max_height` override on top of the variant — the
            // stylesheet's literal value is just a fallback when
            // authors don't tune.
            let style = move || {
                let variant = if value.get() {
                    CollapsibleBodySmoothOpen::Shown
                } else {
                    CollapsibleBodySmoothOpen::Closed
                };
                let mut app = StyleApplication::new(CollapsibleBodySmooth::sheet())
                    .with("open", variant.as_variant_str().to_string());
                if value.get() {
                    // Only override on the open state — closed keeps
                    // max_height: 0 from the variant.
                    app.overrides.max_height =
                        Some(Tokenized::Literal(Length::Px(max_height)));
                }
                app
            };
            ui! { view(style = style) { kids } }
        }
    }
}

// =============================================================================
// Accordion
// =============================================================================

/// Expansion policy for an [`Accordion`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
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
pub struct AccordionItem {
    pub title: Reactive<String>,
    pub body: Element,
}

/// Props for [`Accordion`].
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
    /// [`CollapsibleTransition::Smooth`].
    pub transition: CollapsibleTransition,
    /// Smooth-transition max-height cap (px). Forwarded to each
    /// item's underlying Collapsible. Default
    /// [`SMOOTH_MAX_HEIGHT_DEFAULT_PX`]. See [`CollapsibleProps::max_height`].
    pub max_height: f32,
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
            expand: AccordionExpand::default(),
            transition: CollapsibleTransition::default(),
            max_height: SMOOTH_MAX_HEIGHT_DEFAULT_PX,
            on_change: None,
        }
    }
}

#[component]
pub fn Accordion(props: AccordionProps) -> Element {
    let container_style = AccordionContainer();
    let open_state = props.open;
    let expand = props.expand;
    let transition = props.transition;
    let max_height = props.max_height;
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
        let e_sync = runtime_core::Effect::new(move || {
            let now = open_state.get().get(idx).copied().unwrap_or(false);
            if item_open.get() != now {
                item_open.set(now);
            }
        });
        // Anchor the sync effect to the Accordion's scope.
        std::mem::forget(e_sync);

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
        let body = collapsible_body(item_open, transition, max_height, vec![item.body]);

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

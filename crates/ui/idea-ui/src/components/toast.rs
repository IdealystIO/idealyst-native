//! `Toast` — transient, auto-dismissing notifications.
//!
//! Toasts are pushed imperatively from anywhere (event handlers,
//! async completions) and rendered by a single [`ToastHost`] mounted
//! once near the app root:
//!
//! ```ignore
//! // Mount the host once (top of your app tree):
//! ui! { ToastHost(placement = ToastPlacement::Top) }
//!
//! // One-liners:
//! idea_ui::push_toast("Saved!", tone::Success);
//! idea_ui::push_toast_with("Upload failed", tone::Danger, variant::Filled);
//!
//! // Configured, via the builder — body line, a trailing action, and an
//! // opt-out/custom close:
//! idea_ui::Toast::new("Upload failed")
//!     .tone(tone::Danger)
//!     .variant(variant::Filled)
//!     .body("Server returned 503.")
//!     .action(|| ui! { Button(label = "Retry", on_click = retry) })
//!     .push();
//!
//! // Fully custom content (the closure receives the toast id):
//! idea_ui::push_toast_node(|id| ui! {
//!     Alert(title = "Synced", close = AlertClose::Button(Rc::new(move || dismiss_toast(id))))
//! });
//! ```
//!
//! Each toast fades + slides in (via the framework's `presence`
//! primitive), shows for [`TOAST_SHOW_MS`], then animates out and
//! removes itself.
//!
//! A standard toast renders an [`Alert`](crate::Alert) surface, so toasts
//! carry the same `tone` × `variant` styling and theme tokens as Alert
//! (override globally via `install_alert_sheet(...)`), inherit Alert's
//! native text-color fix — a `Filled` toast's text would otherwise vanish
//! on iOS/Android, which don't cascade color from the container fill — and
//! get a × close affordance (on by default) wired to [`dismiss_toast`].
//!
//! The queue is a process-global (thread-local) signal created with
//! `unscope` so it outlives any one render scope; see
//! [[project_global_cache_signals_unscope]].

use std::cell::Cell;
use std::rc::Rc;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::ViewportPlacement;
use runtime_core::{
    after_ms_detached, component, presence, ui, unscope, AlignItems, Easing, FlexDirection,
    IdealystSchema, IntoElement, JustifyContent, Length, PresenceAnim, PresenceState, Element,
    Signal, StyleRules, StyleSheet, Tokenized, VariantSet,
};

use idea_theme::extensible::{ToneRef, VariantRef};

// A standard toast renders an `Alert` (see module docs). `ui!` references
// the `Alert` type alias `#[component]` generates; `AlertClose` configures
// its close affordance.
use crate::components::alert::{Alert, AlertClose};
use crate::stylesheets::ToastStack;

/// How long a toast stays fully visible before it begins exiting.
pub const TOAST_SHOW_MS: i32 = 3200;
/// Duration of the enter/exit fade-slide.
pub const TOAST_ANIM_MS: u32 = 200;
/// Vertical slide distance (px) of the enter/exit animation.
const TOAST_SLIDE_PX: f32 = 8.0;
/// Default gap (px) between the toast stack and the viewport edge(s) it
/// hugs. Override per-host via [`ToastHostProps::edge_gap`].
pub const TOAST_EDGE_GAP: f32 = 16.0;

// =============================================================================
// Global queue
// =============================================================================

/// One queued toast. Constructed by the `push_toast*` family / the
/// [`Toast`] builder; consumed by [`ToastHost`]'s reactive list.
#[derive(Clone)]
pub struct ToastEntry {
    /// Process-unique id. Pass to [`dismiss_toast`] to close early.
    pub id: u64,
    /// Content builder, re-run by the card's `presence`. Built by the
    /// [`Toast`] builder (the standard Alert surface) or supplied via
    /// [`push_toast_node`]. Private: a toast is constructed through the
    /// builder / push functions, never field-by-field.
    render: Rc<dyn Fn() -> Element>,
    /// Flips to `true` when the toast begins its exit animation. The
    /// card's `presence` reads it by looking the entry up in the queue.
    ///
    /// This is a plain `bool` carried inside the queue's `Signal<Vec<_>>`
    /// — NOT a per-entry `Signal` — so it costs no arena slot and is
    /// reclaimed with the entry when [`remove_toast`] drops it. (A
    /// per-toast `unscope`d signal would leak one arena slot per toast
    /// shown, since there is no public `Signal::dispose` reachable here.)
    pub leaving: bool,
}

impl Default for ToastEntry {
    fn default() -> Self {
        Self {
            id: 0,
            render: Rc::new(|| runtime_core::view(Vec::new()).into_element()),
            leaving: false,
        }
    }
}

thread_local! {
    static QUEUE: std::cell::RefCell<Option<Signal<Vec<ToastEntry>>>> =
        const { std::cell::RefCell::new(None) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// The process-global toast queue, lazily created off any render scope.
fn queue() -> Signal<Vec<ToastEntry>> {
    QUEUE.with(|q| {
        if q.borrow().is_none() {
            let sig = unscope(|| Signal::new(Vec::new()));
            *q.borrow_mut() = Some(sig);
        }
        *q.borrow().as_ref().unwrap()
    })
}

fn next_id() -> u64 {
    NEXT_ID.with(|n| {
        let v = n.get();
        n.set(v.wrapping_add(1));
        v
    })
}

// =============================================================================
// Builder + push API
// =============================================================================

/// The toast's close affordance. Mirrors [`AlertClose`] but defaults to
/// `Auto` (a toast shows a × unless you opt out), and `Custom` carries
/// only the *look* — the toast wires the dismiss for you.
enum ToastClose {
    /// Standard × wired to dismiss this toast (the default).
    Auto,
    /// No close affordance.
    Off,
    /// Caller-supplied close *look*, wrapped so a press dismisses.
    Custom(Rc<dyn Fn() -> Element>),
}

/// Fluent builder for a configured toast. Start with [`Toast::new`], chain
/// options, then [`push`](Toast::push). The `push_toast`/`push_toast_with`
/// one-liners delegate here.
///
/// `action` and `close_with` take a thunk (`|| ui! { … }`) rather than a
/// built element: the card may rebuild its surface and `Element` isn't
/// `Clone`, so the slot has to be re-buildable.
pub struct Toast {
    message: String,
    body: Option<String>,
    tone: ToneRef,
    variant: VariantRef,
    action: Option<Rc<dyn Fn() -> Element>>,
    close: ToastClose,
}

impl Toast {
    /// Start a toast with `message` as its title. Defaults: neutral tone,
    /// the default variant, no body, no action, close × on.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            body: None,
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            action: None,
            close: ToastClose::Auto,
        }
    }

    /// Semantic palette (Success/Danger/…).
    pub fn tone(mut self, tone: impl Into<ToneRef>) -> Self {
        self.tone = tone.into();
        self
    }

    /// Surface treatment (Filled/Soft/Outline/…).
    pub fn variant(mut self, variant: impl Into<VariantRef>) -> Self {
        self.variant = variant.into();
        self
    }

    /// A second detail line beneath the title.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Trailing action slot — e.g. an "Undo"/"Retry" `Button`. A thunk
    /// (see the type docs).
    pub fn action(mut self, action: impl Fn() -> Element + 'static) -> Self {
        self.action = Some(Rc::new(action));
        self
    }

    /// Toggle the close ×. `true` (the default) shows it; `false` removes
    /// it.
    pub fn closable(mut self, closable: bool) -> Self {
        self.close = if closable { ToastClose::Auto } else { ToastClose::Off };
        self
    }

    /// Replace the default × with your own close *look*; a press still
    /// dismisses the toast. A thunk (see the type docs).
    pub fn close_with(mut self, close: impl Fn() -> Element + 'static) -> Self {
        self.close = ToastClose::Custom(Rc::new(close));
        self
    }

    /// Enqueue the toast. Returns its id (pass to [`dismiss_toast`] to
    /// close it early).
    pub fn push(self) -> u64 {
        enqueue(move |id| ToastEntry { id, render: self.into_render(id), leaving: false })
    }

    /// Build the re-runnable content closure for this toast, given the id
    /// the queue assigned it. Shared by [`push`](Self::push) and tests.
    fn into_render(self, id: u64) -> Rc<dyn Fn() -> Element> {
        let Toast { message, body, tone, variant, action, close } = self;
        Rc::new(move || {
            let action_node = action.as_ref().map(|build| build());
            let close = match &close {
                ToastClose::Off => AlertClose::None,
                ToastClose::Auto => AlertClose::Button(Rc::new(move || dismiss_toast(id))),
                ToastClose::Custom(look) => {
                    // The author supplies the look; we wrap it so a press
                    // dismisses this toast.
                    let look = look();
                    AlertClose::Custom(
                        runtime_core::pressable(vec![look], move || dismiss_toast(id))
                            .into_element(),
                    )
                }
            };
            ui! {
                Alert(
                    title = message.clone(),
                    body = body.clone(),
                    tone = tone.clone(),
                    variant = variant.clone(),
                    action = action_node,
                    close = close,
                )
            }
        })
    }
}

/// Push a standard toast (default variant, close × on). Returns the
/// toast's id (pass to [`dismiss_toast`] to close it early).
pub fn push_toast(message: impl Into<String>, tone: impl Into<ToneRef>) -> u64 {
    Toast::new(message).tone(tone).push()
}

/// Push a standard toast with an explicit variant.
pub fn push_toast_with(
    message: impl Into<String>,
    tone: impl Into<ToneRef>,
    variant: impl Into<VariantRef>,
) -> u64 {
    Toast::new(message).tone(tone).variant(variant).push()
}

/// Push a toast whose content is built by `render` — full control over the
/// surface. `render` receives the toast's id so it can wire its own
/// dismiss/close/actions (e.g. `dismiss_toast(id)`), and is re-run by the
/// card on each render (so it may read signals). Returns the id.
///
/// [`Alert`](crate::Alert) is the recommended surface; the [`Toast`]
/// builder covers the common Alert-shaped cases without the id plumbing.
pub fn push_toast_node(render: impl Fn(u64) -> Element + 'static) -> u64 {
    let render = Rc::new(render);
    enqueue(move |id| ToastEntry {
        id,
        render: Rc::new(move || render(id)),
        leaving: false,
    })
}

/// Allocate an id, enqueue the built entry, and schedule its auto-dismiss
/// lifecycle.
///
/// The two timers begin the exit after the show window and remove the
/// entry after the exit animation completes. They're imperative,
/// off-scope timers (push is called from anywhere, not inside a component
/// body), so they're detached: the runtime owns them, they fire once, then
/// sweep away.
fn enqueue(build: impl FnOnce(u64) -> ToastEntry) -> u64 {
    let id = next_id();
    queue().update(|v| v.push(build(id)));
    after_ms_detached(TOAST_SHOW_MS, move || begin_leaving(id));
    after_ms_detached(TOAST_SHOW_MS + TOAST_ANIM_MS as i32, move || remove_toast(id));
    id
}

/// Begin dismissing a toast immediately (e.g. on a close click). The
/// card animates out, then removes itself.
pub fn dismiss_toast(id: u64) {
    if queue().get().iter().any(|e| e.id == id) {
        begin_leaving(id);
        after_ms_detached(TOAST_ANIM_MS as i32, move || remove_toast(id));
    }
}

/// Flip an entry's `leaving` flag through the queue signal so every
/// `ToastCard` reading the queue re-evaluates its `present()`.
fn begin_leaving(id: u64) {
    queue().update(|v| {
        if let Some(e) = v.iter_mut().find(|e| e.id == id) {
            e.leaving = true;
        }
    });
}

fn remove_toast(id: u64) {
    queue().update(|v| v.retain(|e| e.id != id));
}

// =============================================================================
// ToastCard
// =============================================================================

#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ToastCardProps {
    /// The queued toast this card renders. Supplied by [`ToastHost`]'s
    /// reactive list, not authored directly.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub entry: ToastEntry,
}

impl Default for ToastCardProps {
    fn default() -> Self {
        Self { entry: ToastEntry::default() }
    }
}

/// One toast surface. Renders the entry's content builder (the standard
/// [`Alert`](crate::Alert) surface, or caller-supplied content) and
/// fades/slides itself in and out via `presence`, driven by the entry's
/// `leaving` flag.
#[component]
pub fn ToastCard(props: &ToastCardProps) -> Element {
    let entry = props.entry.clone();
    let id = entry.id;
    let render = entry.render.clone();

    // The card content, re-run by `presence`.
    let surface = move || render();

    presence(surface)
        // Reactively read this entry's `leaving` flag from the queue. If
        // the entry is already gone, it's leaving (present = false).
        .present(move || {
            queue()
                .get()
                .iter()
                .find(|e| e.id == id)
                .map_or(false, |e| !e.leaving)
        })
        .enter(PresenceAnim::new(
            PresenceState::default().opacity(0.0).translate_y(-TOAST_SLIDE_PX),
            TOAST_ANIM_MS,
            Easing::EaseOut,
        ))
        .exit(PresenceAnim::new(
            PresenceState::default().opacity(0.0).translate_y(-TOAST_SLIDE_PX),
            TOAST_ANIM_MS,
            Easing::EaseIn,
        ))
        .into_element()
}

// =============================================================================
// ToastHost
// =============================================================================

/// Where the toast stack anchors on the viewport — any of the nine
/// regions of a 3×3 grid (the three vertical bands × the three
/// horizontal bands). Default [`BottomLeft`](ToastPlacement::BottomLeft).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema)]
pub enum ToastPlacement {
    /// Top edge, hugging the leading (left) side.
    TopLeft,
    /// Top edge, horizontally centered.
    TopCenter,
    /// Top edge, hugging the trailing (right) side.
    TopRight,
    /// Vertically centered, hugging the leading (left) side.
    MiddleLeft,
    /// Dead center of the viewport.
    MiddleCenter,
    /// Vertically centered, hugging the trailing (right) side.
    MiddleRight,
    /// Bottom edge, hugging the leading (left) side. The default —
    /// the least intrusive spot for transient notifications.
    #[default]
    BottomLeft,
    /// Bottom edge, horizontally centered.
    BottomCenter,
    /// Bottom edge, hugging the trailing (right) side.
    BottomRight,
}

impl ToastPlacement {
    /// Which viewport strip the portal anchors to. The horizontal band
    /// (left/center/right) is resolved by the positioner's flex inside the
    /// strip, not the portal — `ViewportPlacement` has no corners, so the
    /// strip is full-width (top/bottom) or full-height (middle) and the
    /// stack is pushed within it. See [`Self::positioner_rules`].
    fn viewport(self) -> ViewportPlacement {
        use ToastPlacement::*;
        match self {
            TopLeft | TopCenter | TopRight => ViewportPlacement::Top,
            BottomLeft | BottomCenter | BottomRight => ViewportPlacement::Bottom,
            MiddleLeft => ViewportPlacement::Left,
            MiddleRight => ViewportPlacement::Right,
            MiddleCenter => ViewportPlacement::Center,
        }
    }

    /// Style for the positioner view that fills the portal strip and pushes
    /// the (fixed-width) toast stack into the requested corner, leaving
    /// `gap` px of breathing room from the hugged edge(s).
    ///
    /// Top/Bottom strips are full-width, so the positioner is a `Row` and
    /// `justify_content` picks the horizontal band. Middle (Left/Right)
    /// strips are full-height, so the positioner is a `Column` whose
    /// `justify_content: Center` vertically centers the stack while
    /// `align_items` picks the side. Uniform `padding: gap` yields the
    /// edge gap on every hugged side (far-side padding is harmless — the
    /// stack is already pushed away from it).
    fn positioner_rules(self, gap: f32) -> StyleRules {
        use ToastPlacement::*;
        let full = || Some(Tokenized::Literal(Length::pct(100.0)));
        let mut rules = match self {
            // Full-width top/bottom strips: a Row, horizontal band via justify.
            TopLeft | BottomLeft => StyleRules {
                flex_direction: Some(FlexDirection::Row),
                justify_content: Some(JustifyContent::FlexStart),
                width: full(),
                ..Default::default()
            },
            TopCenter | BottomCenter => StyleRules {
                flex_direction: Some(FlexDirection::Row),
                justify_content: Some(JustifyContent::Center),
                width: full(),
                ..Default::default()
            },
            TopRight | BottomRight => StyleRules {
                flex_direction: Some(FlexDirection::Row),
                justify_content: Some(JustifyContent::FlexEnd),
                width: full(),
                ..Default::default()
            },
            // Full-height side strips: a Column, vertically centered, side via align.
            MiddleLeft => StyleRules {
                flex_direction: Some(FlexDirection::Column),
                justify_content: Some(JustifyContent::Center),
                align_items: Some(AlignItems::FlexStart),
                height: full(),
                ..Default::default()
            },
            MiddleRight => StyleRules {
                flex_direction: Some(FlexDirection::Column),
                justify_content: Some(JustifyContent::Center),
                align_items: Some(AlignItems::FlexEnd),
                height: full(),
                ..Default::default()
            },
            // Center: the portal already centers the (content-sized) stack.
            MiddleCenter => StyleRules::default(),
        };
        // Uniform edge gap on all sides — the stack is pushed into a corner,
        // so the far-side padding is harmless and the hugged edges get `gap`.
        let pad = Some(Tokenized::Literal(Length::Px(gap)));
        rules.padding_top = pad.clone();
        rules.padding_right = pad.clone();
        rules.padding_bottom = pad.clone();
        rules.padding_left = pad;
        rules
    }
}

#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ToastHostProps {
    /// Which of the nine viewport regions the toast stack anchors to.
    /// Default [`BottomLeft`](ToastPlacement::BottomLeft).
    pub placement: ToastPlacement,
    /// Gap (px) between the stack and the viewport edge(s) it hugs.
    /// Default [`TOAST_EDGE_GAP`].
    pub edge_gap: f32,
}

impl Default for ToastHostProps {
    fn default() -> Self {
        Self { placement: ToastPlacement::default(), edge_gap: TOAST_EDGE_GAP }
    }
}

/// Renders the process-global toast queue. Mount once near the app
/// root; the `push_toast*` family (from anywhere) enqueues entries that
/// appear here as a non-modal, touch-passthrough overlay anchored per
/// `placement`.
#[component]
pub fn ToastHost(props: &ToastHostProps) -> Element {
    let q = queue();
    let placement = props.placement;
    let gap = props.edge_gap;

    // The fixed-width stack of cards.
    let stack = ui! {
        view(style = ToastStack()) {
            for entry in q, key = entry.id {
                ToastCard(entry = entry)
            }
        }
    };

    // A positioner fills the portal strip and pushes the stack into the
    // requested corner with `gap` px from the hugged edge(s). Built once —
    // placement/gap are static props, so no reactive style closure needed.
    let positioner_sheet: Rc<StyleSheet> =
        Rc::new(StyleSheet::new(move |_vs: &VariantSet| placement.positioner_rules(gap)));
    let positioner = runtime_core::view(vec![stack]).with_style(positioner_sheet).into_element();

    runtime_core::overlay(vec![positioner])
        .placement(placement.viewport())
        .backdrop(BackdropMode::None)
        .trap_focus(false)
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::extensible::{installed_alert_sheet, tone, variant};
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{arena_stats, resolve_style, Color, StyleApplication, StyleSource};

    /// Walk the rendered tree and return the first `Text` node's resolved
    /// color (DFS, into Views and Pressables — enough for an Alert's
    /// title/body/close shape).
    fn first_text_color(el: &Element) -> Option<Color> {
        match el {
            Element::Text { style, .. } => {
                let app = match style.as_ref()? {
                    StyleSource::Static(a) => a.clone(),
                    _ => return None,
                };
                resolve_style(&app).color.clone().map(|c| c.resolve())
            }
            Element::View { children, .. } => children.iter().find_map(first_text_color),
            Element::Pressable { children, .. } => children.iter().find_map(first_text_color),
            _ => None,
        }
    }

    fn alert_children(el: Element) -> Vec<Element> {
        match el {
            Element::View { children, .. } => children,
            _ => panic!("a standard toast renders an Alert View"),
        }
    }

    /// Regression: a standard (Filled) toast must not leak an arena signal
    /// slot. Previously each toast stored a per-entry `unscope`d
    /// `Signal<bool>` that was never disposed (no public `Signal::dispose`
    /// exists), so every toast shown permanently consumed one slot. The
    /// `leaving` flag now rides as a plain `bool` inside the queue's
    /// `Signal<Vec<_>>`.
    #[test]
    fn pushing_toasts_does_not_leak_signal_slots() {
        // Materialize the one global queue signal so it's part of the
        // baseline (it persists for the process — that's expected).
        let _ = queue();
        let baseline = arena_stats().signals_in_use;

        // With no scheduler installed (unit test), `after_ms` runs its
        // synchronous fallback, so each push fully cycles inline
        // (push → begin_leaving → remove). 64 toasts come and go.
        for i in 0..64 {
            push_toast_with(format!("toast {i}"), ToneRef::default(), VariantRef::default());
        }

        assert_eq!(
            arena_stats().signals_in_use,
            baseline,
            "toasts must not leak signal slots"
        );
    }

    /// Regression: a standard (Filled) toast must render its text in the
    /// intent foreground, not the default label color. The toast surface
    /// used to be a hand-rolled `view + text(AlertTitle)` that set no
    /// `color` — fine on web (the container fill cascades), but on
    /// iOS/Android (no text-color inheritance) the title rendered in the
    /// default dark color and vanished on the solid fill. Routing the
    /// standard path through `Alert` carries Alert's per-node color
    /// stamping, so the title text node carries the intent foreground.
    #[test]
    fn regression_filled_toast_text_carries_intent_color() {
        install_idea_theme(light_theme());

        let expected = resolve_style(
            &StyleApplication::new(installed_alert_sheet())
                .with("appearance", "primary_filled".to_string()),
        )
        .color
        .clone()
        .expect("the filled container resolves a foreground")
        .resolve();

        let surface =
            Toast::new("Saved").tone(tone::Primary).variant(variant::Filled).into_render(1)();

        let title_color =
            first_text_color(&surface).expect("the toast's title carries its own color");
        assert_eq!(title_color, expected, "toast title is the intent text color");
        assert_eq!(expected.0.to_ascii_lowercase(), "#ffffff");
    }

    /// The builder shows a close × by default; `closable(false)` removes it
    /// (the Alert then renders just its content column).
    #[test]
    fn builder_closable_toggles_the_close() {
        install_idea_theme(light_theme());

        let with_close = alert_children(Toast::new("hi").into_render(1)());
        assert_eq!(with_close.len(), 2, "content + default close ×");

        let no_close = alert_children(Toast::new("hi").closable(false).into_render(2)());
        assert_eq!(no_close.len(), 1, "closable(false) → content only");
    }

    /// An action slot renders between the content and the (default) close.
    #[test]
    fn builder_action_renders_between_content_and_close() {
        install_idea_theme(light_theme());

        let surface = Toast::new("hi")
            .action(|| runtime_core::text("Undo".to_string()).into_element())
            .into_render(7)();
        let children = alert_children(surface);
        assert_eq!(children.len(), 3, "content + action + close");
        match &children[1] {
            Element::Text { .. } => {}
            _ => panic!("action slot renders the provided element"),
        }
    }
}

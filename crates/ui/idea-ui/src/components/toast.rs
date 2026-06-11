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
//! // Push from anywhere:
//! idea_ui::push_toast("Saved!", tone::Success);
//! idea_ui::push_toast_with("Upload failed", tone::Danger, variant::Filled);
//! ```
//!
//! Each toast fades + slides in (via the framework's `presence`
//! primitive), shows for [`TOAST_SHOW_MS`], then animates out and
//! removes itself. The surface reuses the installed Alert stylesheet,
//! so toasts carry the same `tone` × `variant` styling and theme
//! tokens as Alert — override globally via `install_alert_sheet(...)`.
//!
//! The queue is a process-global (thread-local) signal created with
//! `unscope` so it outlives any one render scope; see
//! [[project_global_cache_signals_unscope]].

use std::cell::Cell;

use runtime_core::primitives::overlay::BackdropMode;
use runtime_core::primitives::portal::ViewportPlacement;
use runtime_core::{
    after_ms_detached, component, presence, ui, unscope, Easing, IdealystSchema, IntoElement,
    PresenceAnim, PresenceState, Element, Signal, StyleApplication,
};

use idea_theme::extensible::{installed_alert_sheet, ToneRef, VariantRef};

use crate::stylesheets::{AlertTitle, ToastStack};

/// How long a toast stays fully visible before it begins exiting.
pub const TOAST_SHOW_MS: i32 = 3200;
/// Duration of the enter/exit fade-slide.
pub const TOAST_ANIM_MS: u32 = 200;
/// Vertical slide distance (px) of the enter/exit animation.
const TOAST_SLIDE_PX: f32 = 8.0;

// =============================================================================
// Global queue
// =============================================================================

/// One queued toast. Constructed by [`push_toast`]; consumed by
/// [`ToastHost`]'s reactive list.
#[derive(Clone, IdealystSchema)]
pub struct ToastEntry {
    /// Process-unique id. Pass to [`dismiss_toast`] to close early.
    pub id: u64,
    /// The message line shown in the toast card.
    pub message: String,
    /// Semantic palette (Success/Danger/…) for the surface styling.
    pub tone: ToneRef,
    /// Surface skeleton (Filled/Soft/…) for the surface styling.
    pub variant: VariantRef,
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
            message: String::new(),
            tone: ToneRef::default(),
            variant: VariantRef::default(),
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

/// Push a toast with the default Filled variant. Returns the toast's
/// id (pass to [`dismiss_toast`] to close it early).
pub fn push_toast(message: impl Into<String>, tone: impl Into<ToneRef>) -> u64 {
    push_toast_with(message, tone, VariantRef::default())
}

/// Push a toast with an explicit variant.
pub fn push_toast_with(
    message: impl Into<String>,
    tone: impl Into<ToneRef>,
    variant: impl Into<VariantRef>,
) -> u64 {
    let id = next_id();
    let entry = ToastEntry {
        id,
        message: message.into(),
        tone: tone.into(),
        variant: variant.into(),
        leaving: false,
    };
    queue().update(|v| v.push(entry));

    // Begin exit after the show window, then remove after the exit
    // animation completes. These are imperative, off-scope timers (push
    // is called from anywhere, not inside a component body), so they're
    // detached: the runtime owns them, they fire once, then sweep away.
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

/// One toast surface. Renders the entry's message on the Alert-sheet
/// surface and fades/slides itself in and out via `presence`, driven by
/// the entry's `leaving` signal.
#[component]
pub fn ToastCard(props: &ToastCardProps) -> Element {
    let entry = props.entry.clone();
    let id = entry.id;
    let appearance = format!("{}_{}", entry.tone.key(), entry.variant.key());
    let message = entry.message;

    let surface = move || {
        let app = appearance.clone();
        let msg = message.clone();
        ui! {
            view(style = move || {
                StyleApplication::new(installed_alert_sheet()).with("appearance", app.clone())
            }) {
                text(style = AlertTitle()) { msg.clone() }
            }
        }
    };

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

/// Where the toast stack anchors on the viewport.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema)]
pub enum ToastPlacement {
    /// Anchor the stack to the top of the viewport.
    #[default]
    Top,
    /// Anchor the stack to the bottom of the viewport.
    Bottom,
}

impl ToastPlacement {
    fn viewport(self) -> ViewportPlacement {
        match self {
            ToastPlacement::Top => ViewportPlacement::Top,
            ToastPlacement::Bottom => ViewportPlacement::Bottom,
        }
    }
}

#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ToastHostProps {
    /// Viewport edge the toast stack anchors to. Default Top.
    pub placement: ToastPlacement,
}

impl Default for ToastHostProps {
    fn default() -> Self {
        Self { placement: ToastPlacement::default() }
    }
}

/// Renders the process-global toast queue. Mount once near the app
/// root; [`push_toast`] (from anywhere) enqueues entries that appear
/// here as a non-modal, touch-passthrough overlay anchored per
/// `placement`.
#[component]
pub fn ToastHost(props: &ToastHostProps) -> Element {
    let q = queue();
    let list = ui! {
        view(style = ToastStack()) {
            for entry in q, key = entry.id {
                ToastCard(entry = entry)
            }
        }
    };

    runtime_core::overlay(vec![list])
        .placement(props.placement.viewport())
        .backdrop(BackdropMode::None)
        .trap_focus(false)
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::arena_stats;

    /// Regression: a toast must not leak an arena signal slot. Previously
    /// each toast stored a per-entry `unscope`d `Signal<bool>` that was
    /// never disposed (no public `Signal::dispose` exists), so every toast
    /// shown permanently consumed one slot. The `leaving` flag now rides as
    /// a plain `bool` inside the queue's `Signal<Vec<_>>`.
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
}

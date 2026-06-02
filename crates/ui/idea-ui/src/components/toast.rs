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
    after_ms, component, presence, ui, unscope, Easing, IdealystSchema, IntoElement, PresenceAnim,
    PresenceState, Element, Signal, StyleApplication,
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
    /// card's `presence` reads it via `present()`.
    pub leaving: Signal<bool>,
}

impl Default for ToastEntry {
    fn default() -> Self {
        Self {
            id: 0,
            message: String::new(),
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            leaving: Signal::default(),
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
    let leaving = unscope(|| Signal::new(false));
    let entry = ToastEntry {
        id,
        message: message.into(),
        tone: tone.into(),
        variant: variant.into(),
        leaving,
    };
    queue().update(|v| v.push(entry));

    // Begin exit after the show window, then remove after the exit
    // animation completes. `mem::forget` keeps the one-shot tasks alive
    // past this fn — dropping a ScheduledTask cancels it.
    let start_exit = after_ms(TOAST_SHOW_MS, move || leaving.set(true));
    std::mem::forget(start_exit);
    let remove = after_ms(TOAST_SHOW_MS + TOAST_ANIM_MS as i32, move || remove_toast(id));
    std::mem::forget(remove);
    id
}

/// Begin dismissing a toast immediately (e.g. on a close click). The
/// card animates out, then removes itself.
pub fn dismiss_toast(id: u64) {
    // `leaving` inside the cloned entries is the same `Signal` (Copy),
    // so flipping it here drives the real card's exit.
    let mut found = false;
    if let Some(e) = queue().get().iter().find(|e| e.id == id) {
        e.leaving.set(true);
        found = true;
    }
    if found {
        let remove = after_ms(TOAST_ANIM_MS as i32, move || remove_toast(id));
        std::mem::forget(remove);
    }
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
    let leaving = entry.leaving;
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
        .present(move || !leaving.get())
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

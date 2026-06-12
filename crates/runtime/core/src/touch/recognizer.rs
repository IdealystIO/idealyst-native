//! The [`Recognizer`] finite-state-machine abstraction shared by every
//! stock recognizer (`tap`, `long_press`, `pan`, `pinch`) and by any
//! third-party recognizer.
//!
//! ## Why this exists
//!
//! Each stock recognizer is a state machine over the raw [`TouchEvent`]
//! stream. Before this trait they were four independent closures with
//! private state enums — impossible to compose, prioritise, or coordinate.
//! `Recognizer` lifts the shared shape into one surface so the gesture
//! arbiter (see the `gesture` SDK) can drive N of them against a single
//! `on_touch` slot and resolve conflicts between them.
//!
//! The state vocabulary mirrors UIKit's `UIGestureRecognizerState`
//! deliberately, so the mental model transfers: `Possible → Began →
//! Changed* → Recognized | Failed | Cancelled`.
//!
//! ## Callbacks live inside the recognizer
//!
//! A recognizer owns its user callback (captured at construction, exactly
//! like the original `pan`/`pinch` factories). [`Recognizer::update`]
//! returns only the [`GestureState`] and the desired [`TouchResponse`];
//! the arbiter never sees the recognizer's typed output payload
//! (`PanEvent`, `PinchEvent`, …). This keeps the trait object-safe
//! (`Box<dyn Recognizer>`) with no associated type, and lets a recognizer
//! emit whatever shape it likes.
//!
//! ## Gating (require-to-fail)
//!
//! A recognizer must consult [`RecognizerCtx::may_recognize`] *before*
//! transitioning to [`GestureState::Began`] or [`GestureState::Recognized`]
//! (and before firing the corresponding callback). When it is `false` a
//! require-to-fail prerequisite has not yet failed, so the recognizer must
//! stay [`GestureState::Possible`] and re-evaluate on the next event. The
//! arbiter drives recognizers in dependency order within a single event, so
//! a prerequisite that fails on the same event (e.g. a pan that lifts
//! without crossing slop) has already reached [`GestureState::Failed`] by
//! the time its dependent (e.g. a tap) is driven — no event replay needed.
//!
//! Standalone use (the stock factory wrappers) passes
//! [`RecognizerCtx::UNGATED`], so `may_recognize` is always `true` and the
//! gate is a no-op — behaviour is identical to the pre-trait closures.

use std::rc::Rc;

use crate::touch::{TouchEvent, TouchResponse};

/// Installed by the arbiter so a recognizer that can recognize *off* the
/// touch stream (the long-press timer is the only stock case) asks the
/// driver to re-run arbitration instead of firing unilaterally. Calling
/// it signals "I have a pending state change — re-poll me". Standalone
/// recognizers (no arbiter) have no notifier set and fire directly.
pub type AsyncNotifier = Rc<dyn Fn()>;

/// Lifecycle state of a single recognizer for one interaction. Mirrors
/// UIKit's `UIGestureRecognizerState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureState {
    /// Default / resting. Watching the stream; may still begin or fail.
    Possible,
    /// A continuous gesture committed and started this event (pan/pinch
    /// first active frame). The recognizer now owns the interaction.
    Began,
    /// A continuous gesture updated (pan/pinch subsequent frames).
    Changed,
    /// Terminal-success: a discrete gesture fired (tap/long-press), or a
    /// continuous gesture finished cleanly (finger lifted while active).
    Recognized,
    /// Terminal-failure: the recognizer will not fire for this
    /// interaction (slop/timeout exceeded, never reached its threshold).
    /// Frees the touch for competitors and unblocks require-to-fail
    /// dependents.
    Failed,
    /// Terminal-interrupt: the gesture had already begun and was cut off
    /// by the platform, a parent claim, or node detach. Distinct from
    /// [`GestureState::Failed`] because side effects already happened and
    /// the recognizer's `Cancelled` callback must surface.
    Cancelled,
}

impl GestureState {
    /// `Recognized | Failed | Cancelled` — the interaction is over for
    /// this recognizer until it is [`Recognizer::reset`].
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Recognized | Self::Failed | Self::Cancelled)
    }

    /// `Began | Changed` — the recognizer currently owns the interaction
    /// and a cancel must surface its `Cancelled` callback.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Began | Self::Changed)
    }
}

/// Whether a recognizer fires once (discrete) or streams updates
/// (continuous). Drives arbiter defaults — a discrete winner can fire on
/// touch-up without cancelling a not-yet-begun continuous peer, and a
/// continuous winner claims the native touch by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecognizerKind {
    /// Fires once: `Possible → Recognized` (tap, long-press).
    Discrete,
    /// Streams: `Possible → Began → Changed* → Recognized` (pan, pinch).
    Continuous,
}

/// Per-event coordination passed by the driver into [`Recognizer::update`].
#[derive(Clone, Copy, Debug)]
pub struct RecognizerCtx {
    /// `false` while a require-to-fail prerequisite is still unresolved.
    /// A recognizer MUST NOT transition to [`GestureState::Began`] /
    /// [`GestureState::Recognized`] (nor fire its callback) while this is
    /// `false`; it stays [`GestureState::Possible`] and re-checks next
    /// event. See the module docs.
    pub may_recognize: bool,
}

impl RecognizerCtx {
    /// No gating — the recognizer may recognize freely. Used by the
    /// standalone factory wrappers and as the arbiter's default for any
    /// recognizer with no unresolved prerequisites.
    pub const UNGATED: Self = Self { may_recognize: true };
}

impl Default for RecognizerCtx {
    fn default() -> Self {
        Self::UNGATED
    }
}

/// What a recognizer reports after consuming one [`TouchEvent`].
#[derive(Clone, Copy, Debug)]
pub struct RecognizerUpdate {
    /// The recognizer's new FSM state — the arbiter's input for conflict
    /// resolution.
    pub state: GestureState,
    /// The [`TouchResponse`] this recognizer wants for the event (whether
    /// it owns/consumes the touch, whether it claims to preempt native
    /// scrollers). The standalone wrapper returns this verbatim; the
    /// arbiter aggregates it across recognizers and may override it to
    /// `IGNORED` for a recognizer that lost arbitration.
    pub response: TouchResponse,
}

impl RecognizerUpdate {
    pub fn new(state: GestureState, response: TouchResponse) -> Self {
        Self { state, response }
    }
}

/// A finite-state gesture recognizer driven by the raw touch stream.
///
/// Implementors fire their own user callbacks internally. The driver
/// (standalone wrapper or arbiter) consumes only the returned
/// [`RecognizerUpdate`]. See the module docs for the gating contract and
/// the rationale for keeping callbacks inside the recognizer.
pub trait Recognizer {
    /// Stable, human-readable name for diagnostics and require-to-fail
    /// wiring. Should be unique within a recognizer family (e.g. `"tap"`,
    /// `"pan"`).
    fn name(&self) -> &'static str;

    /// Discrete vs. continuous. Defaults to [`RecognizerKind::Continuous`].
    fn kind(&self) -> RecognizerKind {
        RecognizerKind::Continuous
    }

    /// Current state without advancing the machine.
    fn state(&self) -> GestureState;

    /// Feed one raw touch event and advance the machine. Fires the
    /// recognizer's user callback(s) for any transition it makes this
    /// event (subject to `ctx.may_recognize`, see module docs).
    fn update(&mut self, ev: &TouchEvent, ctx: &RecognizerCtx) -> RecognizerUpdate;

    /// Return to [`GestureState::Possible`] for a fresh interaction.
    ///
    /// `cancelled = true` means the arbiter is tearing this recognizer
    /// down because a competitor won (or the stream was interrupted): a
    /// recognizer that was [`GestureState::is_active`] must surface its
    /// `Cancelled` callback. `cancelled = false` is an ordinary reset
    /// between interactions and fires nothing.
    fn reset(&mut self, cancelled: bool);

    /// Install the arbiter's re-arbitration hook. A recognizer that can
    /// recognize off the touch stream (long-press timer) stores this and
    /// calls it from its timer instead of firing; the arbiter responds by
    /// calling [`Recognizer::poll_async`]. Default no-op: recognizers that
    /// only ever recognize on a touch event ignore it. Standalone wrappers
    /// never install one, so off-stream recognizers fire directly.
    fn set_async_notifier(&mut self, _notifier: AsyncNotifier) {}

    /// Drive an off-stream state change the recognizer signalled via its
    /// [`AsyncNotifier`]. Fires the pending callback subject to
    /// `ctx.may_recognize`, exactly like [`Recognizer::update`] gates an
    /// on-stream recognition, and returns the resulting state. Returns
    /// `None` if there was nothing pending. Default: nothing pending.
    fn poll_async(&mut self, _ctx: &RecognizerCtx) -> Option<RecognizerUpdate> {
        None
    }
}

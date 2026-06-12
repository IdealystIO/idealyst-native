//! Gesture arbitration — drive several [`Recognizer`]s against one view's
//! `on_touch` slot and resolve the conflicts a single slot can't.
//!
//! ```ignore
//! use gesture::GestureGroup;
//! use runtime_core::{Tap, Pan, TapRecognizer, PanRecognizer};
//!
//! let mut g = GestureGroup::new();
//! let tap = g.add(Tap::new(TapRecognizer::new(), || select_item()));
//! let pan = g.add(Pan::new(PanRecognizer::new(), |s| drag(s)));
//! // A clean press should tap, but a drag should pan instead — so the tap
//! // waits to see the pan fail before it fires.
//! g.require_to_fail(tap, pan);
//! view(/* … */).on_touch(g.handler());
//! ```
//!
//! See `docs/gesture-arbiter-plan.md` for the model. The short version:
//!
//! - **Priority** is add order — earlier [`GestureGroup::add`] wins ties.
//! - **[`GestureGroup::require_to_fail`]** holds a dependent in
//!   [`GestureState::Possible`] until its prerequisite reaches
//!   [`GestureState::Failed`]. Recognizers are driven in dependency order
//!   within each event, so a prerequisite that fails on the same event
//!   (e.g. a pan that lifts without crossing slop) has already failed by
//!   the time its dependent (a tap) is driven — no event replay.
//! - **[`GestureGroup::allow_simultaneous`]** lets two recognizers be
//!   active at once (pan + pinch). Otherwise the first to begin/recognize
//!   wins exclusivity and every other live recognizer is cancelled.
//!
//! The key invariant that makes exclusivity cheap: a recognizer fires user
//! side effects only when it leaves [`GestureState::Possible`], and the
//! arbiter resolves exclusivity at exactly that transition — so a cancelled
//! loser was still `Possible` and has emitted nothing. (See the module-level
//! design doc for the full argument.)

use std::collections::HashSet;
use std::rc::Rc;

use runtime_core::{
    GestureState, Recognizer, RecognizerCtx, TouchEvent, TouchHandler, TouchPhase, TouchResponse,
};

/// Opaque handle to a recognizer added to a [`GestureGroup`]. Pass it to
/// [`GestureGroup::require_to_fail`] / [`GestureGroup::allow_simultaneous`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RecognizerRef(usize);

/// Builder + owner for a coordinated set of recognizers sharing one
/// `on_touch` slot. Add recognizers, wire their relationships, then call
/// [`GestureGroup::handler`] to produce the installable [`TouchHandler`].
pub struct GestureGroup {
    recognizers: Vec<Box<dyn Recognizer>>,
    /// `requires[d]` = prerequisite indices that must fail before `d` may
    /// recognize.
    requires: Vec<Vec<usize>>,
    /// `simul[i]` = peers allowed to be active alongside `i` (symmetric).
    simul: Vec<HashSet<usize>>,
}

impl Default for GestureGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl GestureGroup {
    pub fn new() -> Self {
        Self {
            recognizers: Vec::new(),
            requires: Vec::new(),
            simul: Vec::new(),
        }
    }

    /// Add a recognizer. Earlier adds have higher priority when two
    /// recognizers want to begin on the same event.
    pub fn add(&mut self, rec: impl Recognizer + 'static) -> RecognizerRef {
        let i = self.recognizers.len();
        self.recognizers.push(Box::new(rec));
        self.requires.push(Vec::new());
        self.simul.push(HashSet::new());
        RecognizerRef(i)
    }

    /// `dependent` stays [`GestureState::Possible`] until `prerequisite`
    /// reaches [`GestureState::Failed`] (or is cancelled). The UIKit
    /// `require(toFail:)` edge.
    pub fn require_to_fail(&mut self, dependent: RecognizerRef, prerequisite: RecognizerRef) {
        if dependent.0 != prerequisite.0
            && !self.requires[dependent.0].contains(&prerequisite.0)
        {
            self.requires[dependent.0].push(prerequisite.0);
        }
    }

    /// Allow `a` and `b` to be active simultaneously (UIKit
    /// `shouldRecognizeSimultaneously = true`). Symmetric.
    pub fn allow_simultaneous(&mut self, a: RecognizerRef, b: RecognizerRef) {
        if a.0 != b.0 {
            self.simul[a.0].insert(b.0);
            self.simul[b.0].insert(a.0);
        }
    }

    /// Consume the group and produce the [`TouchHandler`] for a view's
    /// `on_touch` slot.
    pub fn handler(self) -> TouchHandler {
        let order = topo_order(&self.requires);
        let slots: Vec<Slot> = self
            .recognizers
            .into_iter()
            .enumerate()
            .map(|(i, rec)| Slot {
                rec,
                requires: self.requires[i].clone(),
                simul: self.simul[i].clone(),
                done: false,
                failed: false,
            })
            .collect();

        let inner = Rc::new(std::cell::RefCell::new(Inner {
            slots,
            order,
            active_touches: HashSet::new(),
        }));

        // Install each recognizer's async notifier so an off-stream
        // recognition (long-press timer) re-runs arbitration instead of
        // firing unilaterally. Weak so the notifier → inner → recognizer →
        // notifier cycle can't leak.
        {
            let weak = Rc::downgrade(&inner);
            let mut g = inner.borrow_mut();
            let n = g.slots.len();
            for i in 0..n {
                let w = weak.clone();
                g.slots[i].rec.set_async_notifier(Rc::new(move || {
                    if let Some(s) = w.upgrade() {
                        // Guard against reentrancy: a notifier that fires
                        // synchronously while we already hold the borrow is
                        // simply skipped (the in-flight pass will see the
                        // pending state on its own re-poll).
                        if let Ok(mut g) = s.try_borrow_mut() {
                            g.rearbitrate_async();
                        }
                    }
                }));
            }
        }

        let h = inner;
        Rc::new(move |ev: &TouchEvent| -> TouchResponse {
            // A reentrant touch event (shouldn't happen — backends deliver
            // serially) falls through as ignored rather than panicking.
            match h.try_borrow_mut() {
                Ok(mut g) => g.drive_event(ev),
                Err(_) => TouchResponse::IGNORED,
            }
        })
    }
}

struct Slot {
    rec: Box<dyn Recognizer>,
    requires: Vec<usize>,
    simul: HashSet<usize>,
    /// Reached a terminal state this interaction — skip until reset.
    done: bool,
    /// Reached a *non-recognizing* terminal (Failed/Cancelled) this
    /// interaction — unblocks require-to-fail dependents.
    failed: bool,
}

struct Inner {
    slots: Vec<Slot>,
    /// Dependency order (prerequisites before dependents) for driving.
    order: Vec<usize>,
    /// Live finger ids — used to detect the last lift and reset the group.
    active_touches: HashSet<runtime_core::TouchId>,
}

impl Inner {
    fn drive_event(&mut self, ev: &TouchEvent) -> TouchResponse {
        if ev.phase == TouchPhase::Began {
            self.active_touches.insert(ev.id);
        }

        let mut winners: Vec<usize> = Vec::new();
        let mut consumed = false;
        let mut claim = false;

        for k in 0..self.order.len() {
            let idx = self.order[k];
            if self.slots[idx].done {
                continue;
            }
            let may = self.may_recognize(idx);
            let ctx = RecognizerCtx {
                may_recognize: may,
            };
            let upd = self.slots[idx].rec.update(ev, &ctx);
            consumed |= upd.response.consumed;
            claim |= upd.response.claim;
            self.record_outcome(idx, upd.state, &mut winners);
        }

        self.resolve_exclusivity(&winners);

        if matches!(ev.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            self.active_touches.remove(&ev.id);
            if self.active_touches.is_empty() {
                self.reset_all();
            }
        }

        TouchResponse { consumed, claim }
    }

    /// Re-poll recognizers that signalled an off-stream state change
    /// (long-press timer). No touch event, so no aggregate response — the
    /// only effect is firing the recognizer's callback and applying
    /// exclusivity if it recognized.
    fn rearbitrate_async(&mut self) {
        let mut winners: Vec<usize> = Vec::new();
        for k in 0..self.order.len() {
            let idx = self.order[k];
            if self.slots[idx].done {
                continue;
            }
            let may = self.may_recognize(idx);
            let ctx = RecognizerCtx {
                may_recognize: may,
            };
            if let Some(upd) = self.slots[idx].rec.poll_async(&ctx) {
                self.record_outcome(idx, upd.state, &mut winners);
            }
        }
        self.resolve_exclusivity(&winners);
    }

    /// `true` if every require-to-fail prerequisite of `idx` has failed.
    fn may_recognize(&self, idx: usize) -> bool {
        self.slots[idx]
            .requires
            .iter()
            .all(|&p| self.slots[p].failed)
    }

    /// Fold one recognizer's returned state into the slot's terminal
    /// bookkeeping and the winner list.
    fn record_outcome(&mut self, idx: usize, state: GestureState, winners: &mut Vec<usize>) {
        if state.is_terminal() {
            self.slots[idx].done = true;
            // Recognized = it claimed the gesture; Failed/Cancelled = it
            // won't, which unblocks dependents.
            self.slots[idx].failed = state != GestureState::Recognized;
        }
        if matches!(state, GestureState::Began | GestureState::Recognized) {
            winners.push(idx);
        }
    }

    /// When at least one recognizer began/recognized this pass, the
    /// highest-priority (lowest index) one wins exclusivity; every other
    /// live recognizer not allowed to run alongside it is cancelled.
    fn resolve_exclusivity(&mut self, winners: &[usize]) {
        let Some(&w) = winners.iter().min_by_key(|&&i| i) else {
            return;
        };
        for i in 0..self.slots.len() {
            if i == w || self.slots[i].done || self.slots[w].simul.contains(&i) {
                continue;
            }
            let was_active = self.slots[i].rec.state().is_active();
            self.slots[i].rec.reset(was_active);
            self.slots[i].done = true;
            // A cancelled competitor will never recognize this
            // interaction → treat as failed so its dependents unblock.
            self.slots[i].failed = true;
        }
    }

    /// Return every recognizer to `Possible` for the next interaction.
    fn reset_all(&mut self) {
        for s in &mut self.slots {
            s.rec.reset(false);
            s.done = false;
            s.failed = false;
        }
    }
}

/// Kahn topological sort of the require-to-fail DAG so prerequisites are
/// driven before dependents. Falls back to index order if a cycle is
/// detected (a require-to-fail cycle is a user error and would otherwise
/// deadlock the gate; index order at least keeps the group running).
fn topo_order(requires: &[Vec<usize>]) -> Vec<usize> {
    let n = requires.len();
    // Edge prerequisite -> dependent. indegree[d] = number of prerequisites.
    let mut indegree = vec![0usize; n];
    for (d, deps) in requires.iter().enumerate() {
        indegree[d] = deps.len();
    }
    let mut queue: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    let mut head = 0;
    while head < queue.len() {
        let node = queue[head];
        head += 1;
        order.push(node);
        // Any dependent that lists `node` as a prerequisite loses an edge.
        for (d, deps) in requires.iter().enumerate() {
            if deps.contains(&node) {
                indegree[d] -= 1;
                if indegree[d] == 0 {
                    queue.push(d);
                }
            }
        }
    }
    if order.len() == n {
        order
    } else {
        (0..n).collect()
    }
}

#[cfg(test)]
mod tests;

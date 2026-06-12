# Gesture arbiter & recognizer FSM

Status: **implemented.** Companion to [`native-touch-plan.md`](native-touch-plan.md),
which established the raw `TouchEvent` pipeline. This doc designs the layer the
v1 plan deferred: a reusable recognizer state machine plus an arbitration /
priority framework, inspired by `UIGestureRecognizer` + `UIGestureRecognizerDelegate`.

Where it lives:

- **`Recognizer` trait + `GestureState` / `RecognizerKind` / `RecognizerCtx` /
  `RecognizerUpdate`** — [`crates/runtime/core/src/touch/recognizer.rs`](../crates/runtime/core/src/touch/recognizer.rs).
- **The four stock recognizers** (`Tap` / `LongPress` / `Pan` / `Pinch`),
  each now an `impl Recognizer` with a thin factory wrapper preserving the old
  `tap()/…` `TouchHandler` API — [`recognizers.rs`](../crates/runtime/core/src/touch/recognizers.rs).
- **`GestureGroup` arbiter** — [`crates/sdk/gesture/`](../crates/sdk/gesture/).

## Motivation

What exists today (`crates/runtime/core/src/touch/`):

- Raw per-finger `TouchEvent` stream, one `on_touch` slot per view
  (`TouchHandler = Rc<dyn Fn(&TouchEvent) -> TouchResponse>`).
- Four recognizers (`tap`, `long_press`, `pan`, `pinch`) — each a **private**
  hand-rolled FSM inside a closure, each capturing its own callback.

What's missing — and what this doc specifies:

1. **No reusable recognizer abstraction.** The FSM pattern is duplicated four
   times with no shared `trait`, no public state enum. You can't author your own
   recognizer that the system understands as a recognizer.
2. **No arbitration / priority.** Conflict resolution is limited to spatial
   responder-chain bubbling + the all-or-nothing native `claim`. There is no
   `require(toFail:)`, no simultaneous-recognition opt-in, no priority order.
3. **Hard structural limit: one recognizer per view.** Two recognizers can't
   co-exist on a node, let alone negotiate.

The v1 recognizers note this directly (`recognizers.rs:10-16`): *"future
iterations will introduce a requireToFail-style coordination layer."* This is
that layer.

## Design tenets

- **Core gets the primitive; the SDK gets the composition.** Per CLAUDE.md §3,
  the bare `Recognizer` trait + state enum live in `runtime_core::touch`
  (so the four stock recognizers can implement them and stay in core). The
  *arbiter* — the composable multi-recognizer coordinator — lives in a new
  `crates/sdk/gesture`.
- **No new backend surface.** The arbiter installs exactly one `TouchHandler`
  into the existing `on_touch` slot and fans events to its recognizers in
  Rust. The `claim` protocol (`TouchResponse.claim`) is reused unchanged for
  preempting native scrollers. Zero per-backend code — same uniformity bar as
  pan/zoom.
- **Mirror UIKit semantics, not its API shape.** States and the failure-graph
  match `UIGestureRecognizer` so the mental model transfers; the surface is
  idiomatic Rust (trait + builder, not subclassing + KVO).

## Part 1 — The `Recognizer` trait (core)

A recognizer is a finite state machine fed the raw touch stream. The state enum
mirrors `UIGestureRecognizer.State`:

```rust
// runtime_core::touch

/// Lifecycle state of a single recognizer for one interaction.
/// Mirrors UIKit's UIGestureRecognizerState.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GestureState {
    /// Default. Watching the stream; could still begin or fail.
    Possible,
    /// Continuous gesture committed and started (pan/pinch first frame).
    Began,
    /// Continuous gesture updated (pan/pinch subsequent frames).
    Changed,
    /// Discrete gesture fired (tap), OR continuous gesture finished cleanly.
    /// Terminal-success.
    Recognized,
    /// Will not fire for this interaction. Terminal-failure. Frees the
    /// touch for competitors / require-to-fail dependents.
    Failed,
    /// Interrupted after beginning (system, parent claim, node detach).
    /// Terminal — distinct from Failed: side effects already happened.
    Cancelled,
}

impl GestureState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Recognized | Self::Failed | Self::Cancelled)
    }
    /// Began or Changed — the recognizer currently owns the interaction.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Began | Self::Changed)
    }
}

/// A finite-state gesture recognizer driven by the raw touch stream.
///
/// Implementors fire their own user callbacks internally (captured at
/// construction, exactly like today's `pan`/`pinch` factories). The
/// arbiter consumes only the returned `GestureState` to coordinate
/// priority — it never sees the recognizer's output payload. That keeps
/// the trait object-safe with no associated type and lets each recognizer
/// emit whatever shape it wants (unit, PanSample, PinchSample, …).
pub trait Recognizer {
    /// Stable name for diagnostics and require-to-fail wiring.
    fn name(&self) -> &'static str;

    /// Feed one raw event; return the new state. Called only while the
    /// arbiter considers this recognizer eligible (see gating below).
    fn update(&mut self, ev: &TouchEvent) -> GestureState;

    /// Current state without advancing.
    fn state(&self) -> GestureState;

    /// Forced back to `Possible` for a fresh interaction, OR cancelled by
    /// the arbiter because a competitor won. `cancelled = true` means a
    /// gesture that had already begun must surface its `Cancelled` callback.
    fn reset(&mut self, cancelled: bool);

    /// Whether the gesture is continuous (pan/pinch — Began→Changed*→Ended)
    /// or discrete (tap — Possible→Recognized). Drives arbiter defaults:
    /// a discrete winner can fire on touch-up without cancelling a
    /// not-yet-begun continuous peer.
    fn kind(&self) -> RecognizerKind { RecognizerKind::Continuous }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecognizerKind { Discrete, Continuous }
```

**Why callbacks stay inside the recognizer (not routed through the arbiter):**
keeps the trait object-safe (`Box<dyn Recognizer>`) with no associated output
type, and preserves the existing factory ergonomics. The arbiter's job is
*purely* gating — decide who's allowed to advance — not data transport.

The four stock recognizers become `impl Recognizer`. Their current closures
already hold exactly this state; we lift the private `enum TapState` /
`PanState` / … into structs and move the `match (phase, state)` body into
`update`. The existing `tap()/pan()/…` factory functions stay as thin wrappers
that box the struct, so today's call sites and tests don't break.

## Part 2 — The arbiter (`crates/sdk/gesture`)

```rust
/// Owns the single `on_touch` slot for a view and coordinates N recognizers.
pub struct GestureGroup {
    recognizers: Vec<Slot>,     // priority order = vec order
    rules: ArbitrationRules,
}

struct Slot {
    rec: Box<dyn Recognizer>,
    requires_failure_of: Vec<usize>,  // indices that must Fail before this may Begin
    simultaneous_with: SimSet,        // peers allowed to run alongside this one
}

impl GestureGroup {
    pub fn new() -> Self { /* … */ }

    /// Add a recognizer. Earlier adds have higher priority.
    pub fn add(&mut self, rec: impl Recognizer + 'static) -> RecognizerRef { /* … */ }

    /// `dependent` stays Possible until `prerequisite` reaches Failed.
    /// The UIKit `require(toFail:)` edge.
    pub fn require_to_fail(&mut self, dependent: RecognizerRef, prerequisite: RecognizerRef);

    /// Both may be active at once (UIKit shouldRecognizeSimultaneously = true).
    pub fn allow_simultaneous(&mut self, a: RecognizerRef, b: RecognizerRef);

    /// Produce the installable handler for `.on_touch(...)`.
    pub fn handler(self) -> TouchHandler { /* … */ }
}
```

### Arbitration algorithm (per `TouchEvent`)

The handler returned by `handler()` runs this on every event:

1. **Drive every live recognizer, in dependency order.** Skip only recognizers
   already terminal for this interaction. Crucially, a require-to-fail
   *dependent* still sees `update` — it must, because a discrete recognizer like
   tap only recognizes on `Ended` and can't be "held blind" and replayed later.
   What the dependency creates is a **gate**, not a mute: the driver passes
   `RecognizerCtx { may_recognize }`, which is `false` while any prerequisite is
   still unresolved. A recognizer consults that flag at its begin/recognize
   transition and stays `Possible` if it's `false`. Recognizers are driven in
   topological order of the require-to-fail DAG (prerequisites first), so a
   prerequisite that fails *on this same event* has already reached `Failed` by
   the time its dependent is driven — and the gate is open for it. No event
   replay, no buffering (this is the resolution of the original Open Question 1).
2. *(folded into 1)*
3. **Resolve a winner at the begin/recognize edge.** If one or more recognizers
   transitioned to `Began` or `Recognized` this tick:
   - The highest-priority such recognizer **wins exclusivity**.
   - Every other non-terminal recognizer that is **not** in the winner's
     `simultaneous_with` set is `reset(cancelled = was_active)`. Because losers
     were still `Possible` (hadn't begun), cancelling them is side-effect-free —
     this is the property that makes the simple model correct (see *Why
     exclusivity is cheap* below).
   - Simultaneous peers are left running.
4. **Native claim.** The moment the winner is `Began`/`Recognized` and is
   `Continuous` (or any recognizer that opts in), the aggregate `TouchResponse`
   carries `claim: true`, invoking the existing backend capture protocol to
   preempt parent scrollers. Discrete winners default to `consume`-only.
5. **Aggregate response.** Return `CLAIMED` if anyone claimed, else `CONSUMED`
   if anyone is active/owns the touch, else `IGNORED` so the event bubbles to an
   ancestor `GestureGroup` (composition across the responder chain still works).
6. **Reset on stream end.** On `Ended`/`Cancelled` for the last live finger,
   `reset(false)` every recognizer back to `Possible` for the next interaction.

### Why exclusivity is cheap (the key invariant)

A recognizer fires user-visible side effects only when it transitions out of
`Possible` (to `Began`/`Recognized`). The arbiter resolves exclusivity *at that
exact transition*. So at the instant recognizer X wins, every loser is still in
`Possible` and has emitted nothing — cancelling it is free, no rollback. The
only way two recognizers both fire is if they're explicitly
`allow_simultaneous`, or temporally disjoint (tap fires on up, pan on move). The
`require_to_fail` graph exists precisely for the cases where a recognizer would
otherwise begin *before* a competitor has had the chance to — it delays the
eligibility in step 1 until the prerequisite fails.

This is the same correctness argument UIKit relies on, restated for our
single-threaded, synchronous dispatch.

## Part 3 — Worked examples

### A. Tap vs. pan on the same view (the canonical conflict)

```rust
let mut g = GestureGroup::new();
let tap = g.add(Tap::new(|| select_item()));
let pan = g.add(Pan::new(|s| drag(s)));      // continuous
g.require_to_fail(tap, pan);                  // tap waits to see it's not a drag
view(/* … */).on_touch(g.handler())
```

- Finger down: pan is `Possible` (under 8px slop). tap is *held* — its
  `require_to_fail(pan)` edge means tap isn't eligible until pan `Failed`.
- Finger lifts immediately (<8px, <750ms): pan sees `Ended` with no begin →
  `Failed`. Now tap becomes eligible, sees the buffered up-edge → `Recognized`,
  fires `select_item()`. ✅
- Finger moves >8px: pan → `Began`, wins exclusivity, claims the touch, cancels
  tap. Drag runs; tap never fires. ✅

Both branches resolve **within the triggering event**: because pan (the
prerequisite) is driven before tap (the dependent), tap sees an already-`Failed`
pan and an open gate on the very same `Ended` that failed pan. Covered by
`require_to_fail_tap_fires_after_pan_fails` and
`require_to_fail_pan_wins_and_cancels_tap_on_drag` in the SDK tests.

### B. Pan + pinch simultaneously (photo viewer)

```rust
let mut g = GestureGroup::new();
let pan = g.add(Pan::new(/* one finger drag */));
let pinch = g.add(Pinch::new(/* two finger zoom */));
g.allow_simultaneous(pan, pinch);
```

- Pinch ignores the first finger (returns `Possible`), pan tracks it. Second
  finger lands: pinch → `Began`. Because pan ∈ pinch.`simultaneous_with`, pan is
  **not** cancelled — both run, exactly like `UIScrollView`'s zoom+pan. ✅

### C. Horizontal pan inside a vertical ScrollView

Unchanged from today, but now composable: the horizontal `Pan` recognizer
claims (`claim: true`) once it passes slop on the x-axis, cancelling the native
vertical scroller via the existing protocol. A `GestureGroup` on the inner view
with just that one recognizer behaves identically to today's bare `pan()` — the
arbiter is a no-op overhead of one `Vec` of length 1.

## Part 4 — Crate & module layout

```
crates/runtime/core/src/touch/
  mod.rs              # + GestureState, RecognizerKind (new public types)
  recognizer.rs       # + Recognizer trait (new)
  recognizers.rs      # the 4 stock FSMs, refactored to impl Recognizer;
                      #   existing tap()/pan()/… factories kept as wrappers

crates/sdk/gesture/   # NEW
  src/lib.rs          # GestureGroup, ArbitrationRules, RecognizerRef,
                      #   require_to_fail / allow_simultaneous, handler()
  src/lib.rs tests    # arbitration matrix: priority, require-to-fail,
                      #   simultaneous, cancel-on-loss
```

`sdk/gesture` depends only on `runtime-core` (no backend deps), matching
`sdk/pan` and `sdk/zoom`. The existing pan/zoom SDKs can later grow a
`GestureGroup`-based constructor so they compose, but their current standalone
APIs stay.

## Part 5 — Testing

Per CLAUDE.md §1 & §8, the arbiter is core-adjacent coordination logic and must
land with a full unit matrix (the recognizers already have ~850 lines of FSM
tests to mirror):

- **Trait conformance:** each stock recognizer drives `Possible → … → terminal`
  correctly through `update`, and `reset(cancelled)` surfaces `Cancelled` only
  when it had begun.
- **Priority:** higher-priority recognizer wins a simultaneous begin.
- **require_to_fail:** dependent held in `Possible` until prerequisite `Failed`;
  fires on the same interaction after release (example A).
- **simultaneous:** both stay active (example B); non-listed peers get cancelled.
- **Native claim aggregation:** `CLAIMED` emitted iff a claiming recognizer is
  active; bubbling (`IGNORED`) when no recognizer owns the touch.
- **Regression:** a `GestureGroup` with a single recognizer is byte-for-byte
  behaviorally identical to the bare factory (guards the no-op overhead path).

## Open questions — resolutions (all settled in the shipped build)

1. **Terminal-event replay within a tick (example A).** *Resolved without
   replay.* Dependents are not held blind; they see every event and gate only at
   the begin/recognize transition via `RecognizerCtx::may_recognize`. Driving in
   topological order means a prerequisite that fails on an event has already
   reached `Failed` before its dependent is driven on that same event. No replay,
   no buffering, no iteration cap needed. (The first draft proposed a re-run
   loop; the simpler ordering invariant made it unnecessary.)
2. **Cross-`GestureGroup` priority up the responder chain.** *Out of scope v1, as
   leaned.* `require_to_fail` / `allow_simultaneous` are intra-group only.
   Between nested groups, the existing responder-chain bubble + `claim` protocol
   governs (child-wins): an inner group that claims preempts an ancestor group.
3. **Timer-driven failure (long-press) and arbitration.** *Implemented as
   leaned.* The `Recognizer` trait gained `set_async_notifier` + `poll_async`.
   `LongPress`'s timer no longer fires unilaterally — it enters a `PendingFire`
   state and calls the notifier; the arbiter (`Inner::rearbitrate_async`)
   re-polls every live recognizer with a fresh `may_recognize` and applies the
   same winner/cancel logic. Standalone `long_press()` installs a notifier that
   polls immediately and ungated, so its behaviour is unchanged. Covered by
   `long_press_fires_through_arbiter_timer` and
   `long_press_cancelled_by_competing_pan_before_timer`.
4. **`GestureState`/`Recognizer` in core or SDK?** *Core, as leaned.* The trait +
   state/ctx/update types live in `runtime_core::touch::recognizer`; only the
   composable `GestureGroup` arbiter is in `crates/sdk/gesture`.

## Not yet built (clear next steps, not blockers)

- **`swipe()` and `rotate()` stock recognizers.** Both implement the same
  `Recognizer` trait; swipe is a discrete decision off pan velocity at `Ended`,
  rotate is a two-finger continuous angle delta (peer of `Pinch`).
- **A `GestureGroup`-based constructor on the `pan`/`zoom` SDKs** so they compose
  in a group instead of owning the slot standalone. Their current standalone
  APIs stay.
- **Ergonomic sugar** for the common pairs (e.g. a `tap_or_pan(...)` helper that
  wires the `require_to_fail` edge), once real call sites tell us which pairings
  recur.
```

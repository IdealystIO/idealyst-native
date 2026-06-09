# Change-detection (dedup) on `Signal::set`

**Status:** IMPLEMENTED — `crates/runtime/core/src/reactive.rs`
(`Signal::set_if_changed` / `Signal::update_if_changed`, batch-window
net dedup, `DirtyWindow`). Tests: `reactive::tests::set_if_changed_*` /
`update_if_changed_*`.
**Area:** `crates/runtime/core/src/reactive.rs`. The
`crates/runtime/reactive/arena/src/lib.rs` prototype is unintegrated
(nothing depends on it) and was intentionally left untouched.

## What shipped (beyond the original proposal)

Option 1 (opt-in `set_if_changed` / `update_if_changed`, `where T:
PartialEq`) landed as written, **plus** batch-window *net* dedup, which
is strictly stronger:

- Outside a batch, `set_if_changed` compares the new value against the
  current one in place and fans out only on a real change.
- **Inside a [`batch`], the comparison is against the *window-initial*
  value, not each intermediate.** A signal set `A → B → A` within one
  batch nets to no change and never wakes its subscribers — a per-write
  compare would have notified twice (both steps are real changes), then
  re-run effects that observe the final `A`. This closes the
  no-op-rerender / dropped-interaction class outright.

### How the net dedup works

`batch()`'s pending state changed from an eagerly-collected
`Vec<EffectId>` to a `DirtyWindow` that tracks dirtied **`SignalId`s**
(in first-dirty order) — deferring `collect_subscribers` to the flush so
the net comparison has somewhere to live. Per dirty signal:

- A plain `set`/`update` marks the entry `force` (always-notify
  primitive; also *taints* the window so a co-resident `set_if_changed`
  notifies regardless — the window-initial value was already overwritten
  uncaptured).
- The **first** `set_if_changed` of the window captures the
  window-initial value *by move* (free — it's overwritten anyway) inside
  a `FnOnce(&dyn Any) -> bool` closure. That closure is the type-erased
  bridge: it carries the typed original and, at flush, downcasts the
  signal's live `SignalInner<T>` and compares — so **no `PartialEq`
  bound leaks onto `Signal<T>`**; it lives only on `set_if_changed`,
  which built the closure. (Chosen over a `fn`-pointer + separately
  boxed original, and over a hash/fingerprint — the latter's collision
  risk would *drop* a real notification, a silent correctness bug.)

At flush, signals whose net value is unchanged contribute neither
subscriber wakeups nor JS-notifier fires. JS notifiers now fire once per
net-changed signal at the flush (previously eager per-write, even in a
batch).

> **Batching is now automatic** — see [automatic-batching.md](automatic-batching.md).
> Every reactive turn (event handler, async completion, timer/animation
> frame, reducer dispatch) runs inside a `cycle` window, so the
> *batch-window* net dedup above applies to ordinary app code without an
> explicit `batch(..)`. The two are separate levers: the cycle window
> coalesces fan-out (universal, every `T`); `set_if_changed` additionally
> elides net-zero writes (opt-in, `T: PartialEq`).

---

## Original proposal (retained for rationale)

## Problem

`Signal::set` notifies **every** subscriber unconditionally — it never checks
whether the new value actually differs from the current one:

```rust
// reactive.rs:1196
pub fn set(&self, value: T) {
    assert_not_in_memo_compute();
    if with_signal_mut::<T, _>(self.id, self.gen, |inner| { inner.value = value; }).is_none() {
        return; // only bails on a stale/recycled slot
    }
    let to_run = collect_subscribers(self.id);
    notify_or_queue(&to_run);   // ← fires ALL subscribers, even if value is unchanged
    notify_js_subscriber(self.id);
}
```

So `sig.set(current_value)` still re-runs every effect / `when` / render that
reads `sig`. Today only `memo`/`derived` dedup (they compare their computed
output via `PartialEq` and skip downstream notification when it's unchanged —
reactive.rs:693, 836). Raw `Signal` is a "dumb cell": always notifies.

### Why it matters
- Needless effect re-runs and re-renders whenever app code sets a signal to a
  value it already holds (common: re-applying derived state, syncing props,
  "set on every event" handlers).
- **Compounds with `when(closure, …)`**, which rebuilds its branch on *any*
  tracked-signal change (it does not dedup on the resulting bool). A no-op
  `set` of a signal read inside a `when` closure → unnecessary remount of the
  branch → can drop in-flight pointer interactions on the rebuilt child
  (observed: video play/pause + mute buttons becoming unclickable during
  playback). See the whiteboard-pro video work for a concrete case.

## Goal

When `set`/`update` results in a value **equal** to the current one, skip the
subscriber fan-out (`collect_subscribers` + `notify_or_queue` +
`notify_js_subscriber`).

## Constraint

`Signal<T>` is generic over **any** `T` — there is no `PartialEq` bound today,
and signals legitimately hold non-comparable values (closures, `Rc<dyn …>`,
style builders, etc.). So we **cannot** add a blanket `PartialEq` dedup to the
existing `set` without breaking those uses, and Rust stable has no
specialization to branch on "is `T: PartialEq`".

## Options

1. **Opt-in deduping setter (recommended).** Add, in a `where T: PartialEq`
   impl block, `set_if_changed(&self, value)` (and `update_if_changed`). It
   compares against the current value and only writes + notifies on a real
   change. `set` stays the always-notify primitive. Zero breakage; callers opt
   in on hot/no-op-prone signals.
2. **Auto-dedup `set` by default** — needs specialization (nightly) to keep
   working for non-`PartialEq` `T`. Not viable on stable; also a silent
   semantics change for any code that relies on always-notify.
3. **A `DedupSignal<T: PartialEq>` wrapper type.** More surface area than (1)
   for the same benefit.

## Recommendation — Option 1

Mirror `set`, but compare-then-write inside the single `with_signal_mut` guard
and return whether it changed; only fan out on change:

```rust
impl<T: PartialEq + 'static> Signal<T> {
    /// Like [`set`], but a no-op (no subscriber notification) when `value`
    /// equals the current value. Use on signals that are frequently re-set to
    /// the same value, to avoid needless re-renders.
    pub fn set_if_changed(&self, value: T) {
        assert_not_in_memo_compute();
        let changed = with_signal_mut::<T, _>(self.id, self.gen, |inner| {
            if inner.value == value { false } else { inner.value = value; true }
        });
        if changed == Some(true) {
            notify_or_queue(&collect_subscribers(self.id));
            notify_js_subscriber(self.id);
        }
    }
}
```

(Compare **in place** — no `T: Clone` needed. `update_if_changed` is the same
shape with an `f(&mut clone)`-then-compare, which *does* need `Clone`; offer it
only if a caller wants it.)

Apply the same addition to the arena `Signal` (`reactive/arena/src/lib.rs:277`)
if that path is live.

## Edge cases / risks

- **Borrow safety:** do the compare and the write inside one `with_signal_mut`
  closure (as above) so there's no read-then-write lock gap.
- **Floats:** `PartialEq` on `f32`/`f64` is exact; `NaN != NaN` so a `NaN` set
  always notifies — acceptable.
- **Don't change `set`.** Keep it always-notify: monotonic version counters
  (`version.set(v+1)`) change anyway and rely on the cheap path; some code may
  intentionally force-notify.
- **Reach:** `memo`/`derived` already dedup downstream, so `set_if_changed`
  mainly helps *direct* signal readers and `when(closure)` predicates.

## Companion fix

Independently, make `when(closure, then, else)` **memoize on the computed
bool** (only swap branches when it flips), matching the `Signal<bool>` / `memo`
lowering. That removes the "rebuild interactive child on every tracked-signal
change" footgun even when a deduping setter isn't used. Together the two changes
eliminate this class of needless-rerender / dropped-interaction bug app-wide.

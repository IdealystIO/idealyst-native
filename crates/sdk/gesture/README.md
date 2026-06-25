# `gesture`

Gesture arbitration: drive several recognizers against one view's single
`on_touch` slot and resolve the conflicts that slot can't. This is the
`UIGestureRecognizer` + `UIGestureRecognizerDelegate` coordination layer in
pure Rust, on top of the raw `TouchEvent` stream the framework already unifies
across mouse, touch, and pen. The recognizers themselves (tap / long-press /
pan / pinch / rotate, and any third-party FSM) live in `runtime_core`; this
crate only arbitrates between them — no backend code, no per-platform branching.

A view has exactly one `on_touch` slot, so without arbitration a tap and a pan
on the same element fight over it. `GestureGroup` owns the slot, fans each
event out to every recognizer in Rust, and applies three rules to decide who
wins.

## What you get

- `GestureGroup` — builder + owner for a coordinated set of recognizers.
  - `GestureGroup::new()` / `Default`
  - `add(rec) -> RecognizerRef` — add a recognizer; **earlier adds have higher
    priority** when two want to begin on the same event.
  - `require_to_fail(dependent, prerequisite)` — keep `dependent` in
    `Possible` until `prerequisite` reaches `Failed` (the UIKit
    `require(toFail:)` edge). E.g. a tap waits to see a pan fail before firing.
  - `allow_simultaneous(a, b)` — let both be active at once (pan + pinch, or
    pinch + rotate). Symmetric. Without it, the first recognizer to begin wins
    exclusivity and every other live recognizer is cancelled.
  - `handler() -> TouchHandler` — consume the group, produce the installable
    handler for the view's `on_touch` slot.
- `RecognizerRef` — opaque handle returned by `add`, passed to
  `require_to_fail` / `allow_simultaneous`.

Off-stream recognitions (a long-press timer) are handled too: a recognizer that
recognizes off the touch stream re-runs arbitration instead of firing
unilaterally, so the group can still gate or cancel it.

## Usage

```rust
use gesture::GestureGroup;
use runtime_core::{
    view, Pan, PanRecognizer, Tap, TapRecognizer,
};

fn item() -> runtime_core::Element {
    let mut g = GestureGroup::new();

    // Tap is added first → higher priority. Pan is the prerequisite.
    let tap = g.add(Tap::new(TapRecognizer::new(), || select_item()));
    let pan = g.add(Pan::new(PanRecognizer::new(), |e| drag(e)));

    // A clean press should tap, but a drag should pan instead — so the tap
    // waits to see the pan fail before it fires.
    g.require_to_fail(tap, pan);

    view(vec![/* … */]).on_touch(g.handler()).into()
}
```

Simultaneous recognition (two-finger manipulation):

```rust
use gesture::GestureGroup;
use runtime_core::{Pinch, PinchRecognizer, Rotate, RotateRecognizer};

let mut g = GestureGroup::new();
let pinch  = g.add(Pinch::new(PinchRecognizer::new(), |e| zoom(e)));
let rotate = g.add(Rotate::new(RotateRecognizer::new(), |e| spin(e)));
g.allow_simultaneous(pinch, rotate); // spread *and* twist at the same time
let handler = g.handler();
```

The recognizer constructors (`Tap::new`, `Pan::new`, `Pinch::new`,
`Rotate::new`, `LongPress::new`, …) come from `runtime_core`; `add` accepts any
`impl Recognizer`, including third-party FSMs and `dnd::DragRecognizer`.

## How arbitration resolves

- **Priority** is add order — the earliest-added recognizer wins a tie when
  several want to begin on the same event.
- **`require_to_fail`** drives recognizers in dependency order within each
  event, so a prerequisite that fails on the same event (a pan that lifts
  without crossing slop) has already failed by the time its dependent (a tap)
  is driven — no event replay needed.
- **`allow_simultaneous`** exempts a pair from exclusivity; otherwise the
  winner cancels every other live recognizer.

The invariant that makes exclusivity cheap: a recognizer fires user side
effects only when it leaves `Possible`, and the arbiter resolves exclusivity
at exactly that transition — so a cancelled loser was still `Possible` and has
emitted nothing.

A require-to-fail cycle is a user error; the group falls back to index order
rather than deadlocking.

## Permissions

None.

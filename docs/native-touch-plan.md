# Native touch & gesture system

A raw-touch event pipeline owned by the framework. All gesture recognition
(tap, long-press, pan, swipe, pinch, custom) runs in Rust against the
event stream — no `UIGestureRecognizer` / `GestureDetector` integration.

## Why all-raw

We considered exposing native gesture recognizer bindings for the common
cases (tap, long-press, pan) and Rust recognizers for novel ones. Rejected
because:

- Native recognizers coordinate via `requireToFail` /
  `shouldRecognizeSimultaneously` (iOS) and the `GestureDetector` state
  machine (Android). A Rust recognizer can't participate in either graph.
  "Tap is native, swipe is Rust" means priority/cancellation is decided
  by whichever side's state machine reacts first — differs per OS, per
  OS version, per device.
- React Native lived this for ~5 years. Gesture Handler / Reanimated
  ultimately took over the touch pipeline on both platforms. Worth
  learning from.
- Tap and long-press in Rust are ~20 lines each. The savings from native
  recognizers don't pay for the cross-layer coordination tax.

## The boundary

**Platform-owned widgets keep their gestures sealed.** UIScrollView's
pan-to-scroll, UITextView's text selection, WebView's everything — those
recognizers stay inside the opaque native subtree. We don't open them up
and we don't compete with them at the recognizer level.

**Everything user-authored is raw touch.** `View`, `Pressable`, `Button`,
custom surfaces — all driven by the touch pipeline. The conflict surface
collapses to one well-defined boundary: a Rust gesture inside a native
scrollable container. Handled by a claim protocol (below), not by
fighting recognizers.

## Event shape

```rust
pub struct TouchEvent {
    pub id: TouchId,           // stable Began → Ended/Cancelled
    pub phase: TouchPhase,     // Began | Moved | Ended | Cancelled
    pub position: Vec2,        // view-local
    pub window_position: Vec2, // for cross-view tracking
    pub timestamp_ns: u64,     // platform monotonic
    pub force: Option<f32>,    // 0.0..1.0 if supported
}

pub struct TouchResponse {
    pub consumed: bool, // false → bubble to next subscribed ancestor
    pub claim:    bool, // true  → preempt parent scrollers/recognizers
}
```

Multi-touch dispatch is **per-touch, not batched**. The handler maintains
its own `TouchId → state` map. UIKit's `Set<UITouch>` and Android's
`getPointerCount()` get normalized into one call per touch per phase
change in the backend layer.

## Element surface

Add an `on_touch` slot to `Element::View` (and other primitives that
can carry interactivity — `Pressable`, `ScrollView`, etc.):

```rust
on_touch: Option<Rc<dyn Fn(&TouchEvent) -> TouchResponse>>,
```

**Existing widget primitives stay on their native handlers.** `Button`,
`Pressable`, and similar primitives keep their existing native event
paths (`UIButton` target/action, `<button>` `onclick`,
`android.widget.Button.setOnClickListener`). The raw touch system is
*additive* — recognizers (`tap`, `long_press`) are author-facing
building blocks for novel gestures on plain Views, not a replacement
for the built-in widget actions. Rebuilding the widgets would create
regression surface (slop tuning, focus-ring, accessibility double-tap)
for no gain.

Framework auto-flips `StateBits::PRESSED` while ≥1 touch is active on a
subscribed view; clears on Ended/Cancelled. No author code needed.

## Backend trait

```rust
fn install_touch_handler(
    &mut self,
    node: &Self::Node,
    h: Rc<dyn Fn(&TouchEvent) -> TouchResponse>,
);

fn remove_touch_handler(&mut self, node: &Self::Node);

/// Called by core when a handler returns `TouchResponse { claim: true }`.
/// Backend decides locally how to suppress competing native consumers
/// (scroll containers, system gestures). Core has no knowledge of those
/// mechanisms.
fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId);
```

Per-platform delivery:

| Platform | Capture | Notes |
|----------|---------|-------|
| iOS      | `UIView` subclass, override `touchesBegan:withEvent:` etc. | `multipleTouchEnabled = YES`. DO NOT add `UIGestureRecognizer`s — they fight raw delivery via `delaysTouches*`. |
| Android  | `View.setOnTouchListener` returning `MotionEvent` | Iterate `getPointerCount()`, dispatch per changed pointer index. |
| wgpu     | Unify winit `Touch` + `CursorMoved` + `MouseInput` into a single `PointerEvent` stream. Hit-test in renderer (already happens for clicks). | Stable `TouchId` per mouse button or per OS touch id. |
| web      | Pointer Events API (`pointerdown/move/up/cancel`), `setPointerCapture` on Began. | `touch-action: none` on subscribed nodes. |

## Claim protocol — the hard part

When a row inside a vertical `ScrollView` wants to recognize a horizontal
pan, the scroll view will start scrolling at the same moment. Native
recognizers solve this with `requireToFail`. With raw touch we need an
explicit protocol expressed entirely at the Backend trait surface — core
defines the contract, each backend implements it in native terms.

**Core's view (no platform knowledge):**

1. Handler returns `TouchResponse { claim: true }` at the moment it
   decides "this gesture is mine" (e.g. after 8px horizontal movement).
2. Core forwards the decision to the backend via a trait method, e.g.
   `Backend::claim_touch(node, touch_id)`. That's the whole contract
   core knows about.
3. Subsequent events for the same `TouchId` continue flowing to the
   claiming handler regardless of native interference.

**Backend's view (per platform, implementation-private):**

Each backend's `claim_touch` impl decides locally how to suppress
competing native consumers. The framework doesn't enumerate or know
about those mechanisms — they're behind the trait method. Per-platform
notes for implementers are in the table above; they don't appear in
core code.

Until claim, native scroll containers (which are opaque to us) may
observe the same touch and steal it. When that happens, the platform
delivers `Cancelled` to our handler — which is one reason `Cancelled`
is first-class and distinct from `Ended`.

This protocol shape is the v1 design goal, not a v2 cleanup. The API
isn't pleasant unless this works.

## Open questions

- **Bubbling semantics.** Does an unconsumed Began event bubble immediately,
  or does the framework wait until all phases are observed before deciding
  the chain? UIKit's responder chain commits the responder at Began.
  Android's `dispatchTouchEvent` commits per-event. Suggest: commit at
  Began (topmost subscribed view at hit point), keep all subsequent events
  for the same `TouchId` flowing to it. If it returns `consumed: false`
  on Began, retry one ancestor up — same touch can't change owner mid-flight.
- **State bits on bubble.** If a handler returns unconsumed on Began,
  does `PRESSED` flip on the actual receiving ancestor only, or all
  candidates? Suggest: only the final receiver.
- **Force / pressure on platforms without it.** Default `None` — handlers
  should branch on `Option<f32>` not assume.

## Build order

1. **Core types** in runtime-core: `TouchEvent`, `TouchPhase`,
   `TouchId`, `TouchResponse`, `Vec2`. Add `on_touch` slot to View.
2. **Backend trait** methods (default no-op).
3. **wgpu first** — fastest iteration, dispatcher already hit-tests for
   clicks. Implement bubble + claim entirely in the renderer.
4. **Rust tap/long-press recognizers** as standalone modules in
   `runtime_core::touch::recognizers`. Author-facing building
   blocks; existing widget primitives (`Pressable`, `Button`) keep
   their native event paths — see above.
5. **Backend implementations** — all four (web, Android, iOS, wgpu)
   are equal-priority. wgpu is done; the rest are independent of
   each other and can be picked up in any order.
6. **Motion-value layer** on top — still useful for skipping the
   style-recompute path during pans, even with Rust in the per-frame
   loop. Separate plan doc when we get there.

## Zoom: pinch + the desktop wheel/magnify channel (landed)

Zoom is the first gesture that needs **two** input families, and it shows how
they stay decoupled:

- **Pinch** is a single-view, two-finger recognizer — `runtime_core::pinch`,
  alongside `pan`/`tap`/`long_press`. It rides the existing per-touch stream
  (two concurrent `TouchId`s in one handler), so it needs **no backend
  plumbing** and works on every backend that delivers touches. It tracks the
  first two fingers, activates past a distance slop, and emits `PinchEvent`
  with a cumulative `scale` (relative to the two-finger-down distance) + the
  focal midpoint + smoothed scale-velocity. A lone finger returns `IGNORED` so
  a tap/pan on the same chain still sees it; once active it `CLAIM`s.

- **Trackpad pinch + scroll-wheel** are *not* touches, so they get a separate
  channel: `Backend::install_wheel_handler` + an `on_wheel` slot on View,
  delivering `runtime_core::WheelEvent` (`WheelKind::Zoom | Scroll`). The
  desktop backends source it — **web** (`wheel`; `ctrlKey` ⇒ Zoom, the rest
  Scroll) and **macOS** (`magnify:` ⇒ Zoom, `scrollWheel:` ⇒ Scroll). Each
  backend normalizes its native zoom signal into `WheelEvent.scale` (an
  incremental multiplier) so app code carries no per-platform constant. iOS /
  Android keep the trait default no-op — no trackpad/wheel there; pinch covers
  them.

The two converge in the `zoom` SDK (`crates/sdk/zoom`), which drives one
`AnimatedValue<f32>` scale from both a `pinch_handler()` (`on_touch`) and a
`wheel_handler()` (`on_wheel`) — the scale peer of the `pan` SDK.

## Out of scope (for v1)

- Composing two *different* recognizers on one view (e.g. pan + pinch on the
  same node) — `on_touch` holds a single handler, so a node is pan **or**
  pinch today. Zoom sidesteps this by pairing pinch (touch) with the wheel
  channel (separate slot) rather than with pan. A general composition layer
  can come later.
- Rotate / two-finger pan recognizers (the per-touch model supports them; just
  not built yet).
- Multi-finger gestures requiring simultaneous recognition across views
  (e.g. one finger on a slider, another on a pan) — supported by the
  per-touch model but the recognizer composition story can wait.
- 3D-touch / hover (pre-touch) events.
- Apple Pencil-specific events (azimuth, tilt). Plumbing-only; data
  goes in `TouchEvent.force` and a future `TouchEvent.tool` field.

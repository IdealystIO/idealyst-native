# iOS + Android touch backend implementations

Status: runtime-core, wgpu, and web are landed; iOS and Android are
the two missing platform impls of `Backend::install_touch_handler` and
`Backend::claim_touch`. This doc describes how to land them.

The platforms are independent — pick either first, the other follows
the same shape. Each is roughly one engineering day.

See [native-touch-plan.md](native-touch-plan.md) for the cross-platform
design (event vocabulary, responder chain, claim protocol). This doc
only covers per-platform mechanics.

## What core asks of the backend

Two methods on the [`Backend`] trait, both default no-op so partial
backends still render:

```rust
fn install_touch_handler(
    &mut self,
    node: &Self::Node,
    handler: TouchHandler,
);

fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId);
```

`install_touch_handler` is called once per `Element::View {
on_touch: Some(_), .. }` after node creation. The backend's job:

1. Wire `handler` to whatever native touch delivery the platform
   uses.
2. On each native touch event, translate it into a `TouchEvent` and
   invoke the handler.
3. Track the `TouchResponse`:
   - `consumed: true` → keep the touch local; don't bubble.
   - `consumed: false` → bubble to the next subscribed ancestor.
   - `claim: true` → invoke `claim_touch(node, id)` to suppress
     competing native consumers (parent scroll views, system
     gestures).

`claim_touch` is dispatched by the framework (or by the backend's own
inline path on web) when a handler returns `claim: true`. The
backend's impl is platform-specific — it knows how to silence its
native scroll containers.

---

## iOS

### Event capture

The framework's View primitive already maps to a UIView subclass
(`IDLView` or equivalent). Grow that subclass with the four touch
overrides:

```objc
- (void)touchesBegan:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event;
- (void)touchesMoved:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event;
- (void)touchesEnded:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event;
- (void)touchesCancelled:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event;
```

Set `multipleTouchEnabled = YES` on subscribed instances so all
fingers reach us (default is single-touch).

**Do not add `UIGestureRecognizer`s.** They preempt raw touch delivery
via `delaysTouchesBegan` / `delaysTouchesEnded`. We're explicitly
opting out of that pipeline — see [native-touch-plan.md](native-touch-plan.md).

### Event translation

For each `UITouch` in the changed set:

| `TouchEvent` field | Source |
|---|---|
| `id` | hash of `UITouch *` object pointer (stable through gesture) |
| `phase` | from the override method called |
| `position` | `[touch locationInView: self]` |
| `window_position` | `[touch locationInView: nil]` |
| `timestamp_ns` | `UITouch.timestamp * 1_000_000_000` (NSTimeInterval is seconds) |
| `force` | `touch.force / touch.maximumPossibleForce`, or `None` if `maximumPossibleForce == 0.0` |

### Handler storage

Standard pattern from the existing iOS click bridge:

```objc
// On install:
TouchHandler *boxed = Box::into_raw(...);  // Rust side leaks the box
objc_setAssociatedObject(view, &kTouchHandlerKey, boxed,
                         OBJC_ASSOCIATION_RETAIN_NONATOMIC);

// On invoke:
TouchHandler *handler = objc_getAssociatedObject(view, &kTouchHandlerKey);
TouchResponse r = invoke_rust(handler, &event);

// On dealloc:
// Drop the Rust box via a `-[IDLView dealloc]` extension that
// retrieves the handler and calls `Box::from_raw(handler)` to
// run the destructor.
```

Lifecycle: tied to the UIView. When the view deallocs, the handler
drops automatically. No manual cleanup from the Rust side.

### Responder chain

UIKit's natural responder chain matches our bubble semantics:

- Handler returns `consumed: true` → IDLView does NOT call
  `[super touchesBegan:withEvent:]`. The touch stays here; UIKit
  routes subsequent events for the same `UITouch` to this view.
- Handler returns `consumed: false` → IDLView calls `[super
  touchesBegan:withEvent:]`. UIKit bubbles to the next responder
  (parent view); that view's IDLView impl runs the same check.

Move/End/Cancel route automatically — UIKit tracks each touch's
owning view from Began.

### Claim protocol (`claim_touch`)

When a handler returns `claim: true`:

```rust
fn claim_touch(&mut self, node: &Self::Node, _touch_id: TouchId) {
    // Walk up the view hierarchy from `node` looking for any
    // UIScrollView (UICollectionView, UITableView are subclasses).
    // Toggle their pan recognizer enabled state to cancel any
    // in-flight scroll.
    let view = node.as_view();
    let mut ancestor = view.superview();
    while let Some(v) = ancestor {
        if let Some(sv) = v.as_scroll_view() {
            sv.pan_gesture_recognizer().set_enabled(false);
            sv.pan_gesture_recognizer().set_enabled(true);
        }
        ancestor = v.superview();
    }
}
```

This is the canonical "cancel in-flight pan" trick — toggling
`enabled` forces the recognizer to fail (Cancelled state) and
re-arm for the next gesture. The framework's view of this is the
abstract `claim_touch` call; iOS's impl encapsulates the ancestor
walk.

### Multi-touch

Each `UITouch` is a stable object through its lifecycle. Hash its
pointer for `TouchId`. Multiple concurrent touches → multiple
parallel `TouchEvent` streams sharing a handler.

### Open questions

- **Nested scroll views.** Paged scroll views, scroll-inside-scroll
  layouts may need additional handling. Real-device tests with our
  `ScrollView` primitive will surface edge cases.
- **`UITextView` / `UIScrollView` containers we ship.** Those keep
  their native gestures internally per the sealed-platform-widget
  boundary in [native-touch-plan.md](native-touch-plan.md). They're
  opaque to our claim protocol because we don't subclass them.
- **Pencil / hover events.** `UIEvent.type == UIEventTypeHover`
  arrives via separate hover delegates. v1 doesn't surface hover;
  `force` is the only pencil-related field on `TouchEvent`.

---

## Android

### Event capture

> **Implementation note (superseded sketch below).** The shipped
> backend does **not** override `dispatchTouchEvent`. It installs a
> `View.OnTouchListener` (`RustTouchListener`) per node — see
> `crates/backend/android/mobile/src/imp/primitives/touch.rs`. An
> `OnTouchListener` fires on the *target* (deepest) view first and only
> propagates to ancestors when the child declines, which gives the
> **deepest-view-first → bubble-to-ancestor** responder model the other
> backends use (and that `runtime_core::touch` documents). A
> `dispatchTouchEvent` override would instead make a parent intercept
> *before* its children — the opposite order — so it was rejected. The
> original sketch is kept for historical context:

The framework's View primitive already maps to a `FrameLayout`
subclass (or similar). The original sketch grew that subclass with
`dispatchTouchEvent`:

```kotlin
override fun dispatchTouchEvent(event: MotionEvent): Boolean {
    val anyConsumed = handleTouchInternal(event)
    return if (anyConsumed) true else super.dispatchTouchEvent(event)
}
```

That intercepts *before* children (parent-first), which contradicts
the responder model the framework settled on — hence the
`OnTouchListener` approach above instead. (iOS likewise delivers to the
deepest hit-tested view first and bubbles via the responder chain; it
is not "parent sees touch first.")

### Event translation

`MotionEvent.getActionMasked()`:

| Action | Phase | Pointer to dispatch |
|---|---|---|
| `ACTION_DOWN` | `Began` | `getActionIndex()` (always 0 for first finger) |
| `ACTION_POINTER_DOWN` | `Began` | `getActionIndex()` |
| `ACTION_MOVE` | `Moved` | every pointer (iterate `getPointerCount()`) |
| `ACTION_UP` | `Ended` | `getActionIndex()` |
| `ACTION_POINTER_UP` | `Ended` | `getActionIndex()` |
| `ACTION_CANCEL` | `Cancelled` | every active pointer |

For each pointer to dispatch:

| `TouchEvent` field | Source |
|---|---|
| `id` | `event.getPointerId(index) as u64` |
| `phase` | mapped from action |
| `position` | `(event.getX(index), event.getY(index))` |
| `window_position` | `(event.getRawX(index), event.getRawY(index))` (API 29+; older: window offset add) |
| `timestamp_ns` | `event.eventTime * 1_000_000` |
| `force` | `event.getPressure(index)`, filter sentinel `0.0` and `1.0` |

### JNI surface

One native method:

```kotlin
external fun nativeInvokeTouch(
    handlerPtr: Long,
    id: Long, phase: Int,
    x: Float, y: Float,
    winX: Float, winY: Float,
    timestampNs: Long,
    force: Float, hasForce: Boolean,
): Int  // packed: bit 0 = consumed, bit 1 = claim
```

Rust side derefs `handlerPtr` (which was `Box::into_raw(boxed
TouchHandler)`), builds a `TouchEvent`, invokes, returns the
packed response.

For a Moved with N concurrent touches, we make N JNI calls. At
typical 120hz × 3 fingers that's 360 crossings/sec — acceptable.
Batching could be done later if profiling shows a hot spot.

### Handler storage

Pattern matches the existing Android `ClickCallback`:

```kotlin
// In install (called from Rust via existing JNI):
val ptr: Long = nativeBoxTouchHandler(handler)
view.setTag(R.id.touch_handler_ptr, ptr)

// In dispatchTouchEvent:
val ptr = view.getTag(R.id.touch_handler_ptr) as? Long ?: return false

// In onViewDetachedFromWindow:
val ptr = view.getTag(R.id.touch_handler_ptr) as? Long
if (ptr != null) nativeDropTouchHandler(ptr)
```

Rust side:

```rust
#[no_mangle]
pub extern "C" fn nativeBoxTouchHandler(handler: ...) -> jlong {
    Box::into_raw(Box::new(handler)) as jlong
}

#[no_mangle]
pub extern "C" fn nativeDropTouchHandler(ptr: jlong) {
    let _ = unsafe { Box::from_raw(ptr as *mut TouchHandler) };
}
```

### Responder chain

`dispatchTouchEvent` returning `false` cascades to the parent view's
`dispatchTouchEvent` automatically. Our handler returning
`consumed: false` propagates to `super.dispatchTouchEvent(event)`,
which bubbles up the view hierarchy.

### Claim protocol (`claim_touch`)

```rust
fn claim_touch(&mut self, node: &Self::Node, _touch_id: TouchId) {
    let parent = node.parent();
    if let Some(p) = parent {
        p.request_disallow_intercept_touch_event(true);
    }
}
```

`requestDisallowInterceptTouchEvent(true)` propagates up the parent
chain — every ancestor with `onInterceptTouchEvent` (notably
`ScrollView`, `RecyclerView`, `NestedScrollView`) honors the flag
for the remainder of this gesture and lets touches pass through. The
flag resets at the next `ACTION_DOWN`.

### Multi-touch

`MotionEvent.getPointerId(index)` returns an OS-assigned stable id
per finger through the gesture. Cast to `u64` for `TouchId`. Same
shape as iOS — multiple concurrent streams sharing one handler.

### Open questions

- **`onInterceptTouchEvent` vs. `dispatchTouchEvent`.** I'd start with
  `dispatchTouchEvent` (catches everything before children). If we
  find subscribed Views inside our own framework's `ScrollView`
  primitive want children to win, switch to overriding both with
  `super.onInterceptTouchEvent` returning false unless we've claimed.
  Defer until we see the issue.
- **API 29 `getRawX(index)` availability.** Older devices need to
  add `view.locationOnScreen` to `getX/getY`. Trivial fallback.
- **`requestDisallowInterceptTouchEvent` and `NestedScrollView`.**
  Custom scroll containers sometimes ignore the flag. Verify with
  our framework's `ScrollView` primitive in particular.

---

## Build order

Either platform first. Roughly one engineering day each.

1. **Subclass the native View type** (or extend the existing subclass).
   Both platforms already have such a class for the View primitive.
2. **Implement `install_touch_handler`** — store handler, override the
   touch entry points, translate events, invoke handler, read
   response.
3. **Implement `claim_touch`** — ancestor walk for scroll cancel
   (iOS) / `requestDisallowInterceptTouchEvent` (Android).
4. **Smoke test** by adding `view().on_touch(...)` and
   `view().on_touch(tap(TapRecognizer::new(), || ...))` to the
   existing demo apps. Verify with two-finger gestures too.
5. **Tests** — walker→backend wiring is trivially testable (mock
   storage + assert install was called); the dispatcher itself is
   harder to unit-test because of platform setup. The recognizer
   tests in runtime-core already cover the gesture state machines
   on top.

## Definition of done

- `view().on_touch(...)` mounts a working raw-touch handler on both
  platforms, with parity to the wgpu and web behavior.
- `tap()` and `long_press()` recognizers fire correctly with the
  expected slop / threshold values.
- A pan recognizer inside a `ScrollView` can claim the touch (via
  `TouchResponse { claim: true }`) and the parent scroll stops
  scrolling.
- Multi-touch produces parallel event streams with stable
  `TouchId`s.
- `Cancelled` is delivered when the OS interrupts the gesture
  (incoming call, alert, view detach) and when a sibling claim
  preempts.

## Out of scope (for v1)

- Hover events (pre-touch pointer hover with Pencil / mouse).
- Multi-finger gestures requiring cross-view recognizer composition
  (e.g. one finger on a slider, another on a parent pan).
- 3D-touch / pressure-sensitive gestures beyond the `force` field.
- Apple Pencil tilt / azimuth — `TouchEvent` doesn't carry them yet.

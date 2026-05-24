# Proposal: interaction events on `View`

The framework's interaction surface today is:

- `Button` — single-callback "tap activates a verb."
- `StateBits` — flip-a-bit signals for `HOVERED`/`PRESSED`/`FOCUSED`/
  `DISABLED`, driven by native event listeners, consumed by styling.
- Controlled inputs — `TextInput`, `Toggle`, `Slider` for their
  specific shapes.

There's no path for interactive content that isn't one of those —
draggable cards, swipeable rows, long-press menus, hover previews,
double-tap actions, custom sliders, drawing surfaces. Today the
author has to drop down to `Graphics` (which gives a GPU surface, not
input events) or write platform-specific code in the host crate.

This proposal opens that gap: **interaction events on `View`.** The
framework hands authors raw event streams (pointer down/move/up/cancel,
hover enter/move/exit) with coordinates and stable pointer ids;
gestures (tap, long-press, drag, pinch) are Rust components on top.
No platform-specific gesture recognizers in the framework.

"Interaction events" is the umbrella. Pointer events are the first
family this proposal covers; keyboard and wheel events fit the same
shape and can land later as separate event families on the same
builder pattern (`.on_key`, `.on_wheel`).

> Status: proposal. Not implemented.

---

## Surface

Builder methods on `View`. No new primitive — `View` is already the
generic interaction-capable container, and almost every interactive
thing is structurally a `View` plus a behavior.

```rust
ui! {
    View(style = card_style())
        .on_pointer(move |evt| {
            match evt.phase {
                PointerPhase::Down  => start.set(Some((evt.x, evt.y))),
                PointerPhase::Move  => offset.set((evt.x - start_x, evt.y - start_y)),
                PointerPhase::Up    => /* commit */,
                PointerPhase::Cancel => /* roll back */,
            }
        })
    {
        Text { "Drag me" }
    }
}
```

The callback fires for every pointer event over the view. Authors
who want named gestures build them as components on top
(see "Composing gestures" below).

### `PointerEvent`

```rust
pub struct PointerEvent {
    /// X coordinate in the view's local space (top-left origin).
    pub x: f32,
    /// Y coordinate in the view's local space (top-left origin).
    pub y: f32,
    /// Stable id per pointer for the lifetime of a press/drag.
    /// On a mouse: always 0 (or whatever the platform reports).
    /// On multi-touch: each finger gets a distinct id, stable
    /// from Down through Up/Cancel. Useful for multi-touch
    /// gestures (pinch, two-finger drag).
    pub pointer_id: u32,
    /// Which phase of the gesture this event represents.
    pub phase: PointerPhase,
    /// What kind of pointer fired this — mouse, touch, pen.
    /// Allows components to render different affordances per
    /// kind (hover effects on mouse only, etc.).
    pub pointer_type: PointerType,
}

pub enum PointerPhase {
    /// Pointer just engaged the view (mouse-down, finger-down).
    Down,
    /// Pointer moved while engaged. Fires repeatedly.
    Move,
    /// Pointer disengaged cleanly (mouse-up, finger-lifted).
    Up,
    /// Gesture was interrupted by the platform — system gesture
    /// (iOS back-swipe), window deactivation, multi-touch
    /// conflict resolution. Authors should treat this the same
    /// as Up but roll back instead of commit.
    Cancel,
}

pub enum PointerType {
    Mouse,
    Touch,
    Pen,
}
```

**Coordinates are local to the view.** A pointer at (0, 0) is at the
view's top-left corner; (width, height) is bottom-right. This is
what authors want for "is this inside my visible bounds" / "how far
from the edge" checks. The framework does the per-event subtraction
from the global coordinate.

**No modifier keys.** Keyboard modifiers (`shift`, `cmd`/`ctrl`,
`alt`) belong with keyboard tracking, which is a separate concern.
A future keyboard module exposes them via its own signal API; gesture
code that needs "cmd-click" reads both streams.

**No pressure / tilt / button.** Could be added later if a real need
emerges. The minimal payload covers single- and multi-touch gestures
plus mouse drags — the overwhelming common case.

### `Hover` for mouse-only platforms

Mouse hover (no button pressed) is a real interaction surface on
web/desktop and doesn't fit cleanly into the press-tracking
`pointer` stream. Adding a parallel low-volume callback:

```rust
View(...).on_hover(move |evt| {
    match evt.phase {
        HoverPhase::Enter => /* hover started */,
        HoverPhase::Move  => /* hover moved within view */,
        HoverPhase::Exit  => /* hover ended */,
    }
})

pub struct HoverEvent {
    pub x: f32,
    pub y: f32,
    pub phase: HoverPhase,
}

pub enum HoverPhase { Enter, Move, Exit }
```

Touch-only platforms (mobile) never fire hover. Web/desktop fire it
when the pointer enters/moves/exits without a button held.

This is separate from `StateBits::HOVERED` because the latter is a
*boolean* signal for styling (is the mouse over this thing?) while
`on_hover` is an *event stream* with coordinates. Components that
need both — a tile that gets a hover background AND tracks the
cursor for a spotlight effect — use both.

---

## Changes to `Primitive::View`

The variant grows two optional callback slots:

```rust
pub enum Primitive {
    View {
        children: Vec<Primitive>,
        style: Option<StyleSource>,
        ref_fill: Option<RefFill>,
        on_pointer: Option<Rc<dyn Fn(PointerEvent)>>,
        on_hover:   Option<Rc<dyn Fn(HoverEvent)>>,
    },
    // …
}
```

`Option` so a `View` without listeners stays as cheap as it is today
— the backend skips attaching anything. The cost is `None` checks on
every `View` build, which compile to nothing in release.

`Rc<dyn Fn(...)>` rather than `Box<dyn Fn(...)>` because backends
need to clone the callback into their event-listener closures
(wasm-bindgen on web, JNI trampolines on Android). The walker hands
the backend an `Rc` and the backend installs as many wrappers as it
needs.

`Bound<ViewHandle>` grows the builders:

```rust
impl Bound<ViewHandle> {
    pub fn on_pointer<F>(self, f: F) -> Self
    where F: Fn(PointerEvent) + 'static;

    pub fn on_hover<F>(self, f: F) -> Self
    where F: Fn(HoverEvent) + 'static;
}
```

`ui!` / `jsx!` already pass arbitrary props through; `.on_pointer =
closure` and `.on_hover = closure` desugar to the corresponding
builder calls. No macro changes needed beyond recognizing the prop
names.

---

## `Backend` trait additions

One new method per event kind:

```rust
pub trait Backend {
    // …existing…

    /// Wire the view's pointer event stream. The framework calls this
    /// once per View whose `on_pointer` is `Some`. Backends install
    /// their native listeners (pointermove/down/up on web, touch
    /// listeners + onHoverEvent on Android, UIGestureRecognizer on
    /// iOS) and invoke `callback` with each event translated into the
    /// framework's `PointerEvent`.
    ///
    /// Coordinate translation: the backend MUST translate native
    /// coordinates into the view's local space (top-left origin)
    /// before invoking the callback. The framework's PointerEvent
    /// contract is "local coordinates." Backends do whatever
    /// platform-specific math they need (web: `getBoundingClientRect`
    /// subtract; iOS: `convert:toView:`; Android: `getLocationOnScreen`).
    ///
    /// Pointer id translation: the backend must produce a stable id
    /// per gesture (same id for Down → Move… → Up/Cancel). Platforms
    /// already provide this; backends just pass it through.
    ///
    /// Default impl is a no-op for backends that don't yet implement
    /// pointer events (views with `on_pointer` listeners simply never
    /// fire on those platforms — same posture as `attach_states` and
    /// imperative-ops defaults).
    #[allow(unused_variables)]
    fn attach_pointer(
        &mut self,
        node: &Self::Node,
        callback: Rc<dyn Fn(PointerEvent)>,
    ) {
        // default: no-op
    }

    /// Same shape, for hover events. Touch-only platforms leave the
    /// default no-op; mouse-capable platforms implement.
    #[allow(unused_variables)]
    fn attach_hover(
        &mut self,
        node: &Self::Node,
        callback: Rc<dyn Fn(HoverEvent)>,
    ) {
        // default: no-op
    }
}
```

Two methods, both defaulted. The same minimal-surface posture every
other optional feature uses.

### Why not bundle into `create_view`?

A few reasons:

1. **Most `View`s don't have listeners.** Bundling means every
   `create_view` ignores two `None` parameters. Two separate
   `attach_*` calls (only invoked when needed) is cleaner.
2. **Symmetry with `attach_states`.** That precedent — separate
   `create` + `attach_*` calls per concern — already shapes the
   trait.
3. **Cancellation cost.** A view that loses its listener (the parent
   rebuilds without the prop) needs a teardown path. With separate
   `attach_*`, that's natural — wrap the listener in a scope-owned
   `Effect` and the teardown lives where the framework already
   handles teardown.

---

## Walker integration

Inside the `Primitive::View` arm of `build`:

```rust
Primitive::View { children, style, ref_fill, on_pointer, on_hover } => {
    let n = build_view(backend, children);
    if let Some(s) = style { attach_style(backend, &n, s); }
    if let Some(RefFill::View(fill)) = ref_fill {
        fill(backend.borrow().make_view_handle(&n));
    }
    if let Some(cb) = on_pointer {
        // Wrap in a scope-owned Effect so the listener auto-detaches
        // when the surrounding scope drops (when/switch flip, owner
        // teardown). The Effect body just retains the callback Rc;
        // the actual listener install happens once below.
        backend.borrow_mut().attach_pointer(&n, cb);
    }
    if let Some(cb) = on_hover {
        backend.borrow_mut().attach_hover(&n, cb);
    }
    n
}
```

### Lifecycle: detaching listeners on scope drop

Like `release_virtualizer` / `release_graphics`, native listeners
need to be torn down before the captured `Rc<dyn Fn(PointerEvent)>`
gets freed — otherwise a queued event firing during teardown reaches
a callback whose captured signals are already gone, panicking.

The cleanest shape, matching the existing release-hook pattern:

```rust
fn release_pointer(&mut self, node: &Self::Node) { /* default no-op */ }
fn release_hover(&mut self, node: &Self::Node)   { /* default no-op */ }
```

And the walker installs a cleanup Effect:

```rust
if let Some(cb) = on_pointer {
    backend.borrow_mut().attach_pointer(&n, cb);
    let cleanup = PointerCleanup { backend: backend.clone(), node: n.clone() };
    let _e = Effect::new(move || { let _ = &cleanup.node; });
    // PointerCleanup's Drop calls backend.release_pointer(&node).
}
```

Same mechanic the framework uses everywhere else cleanup needs to
ride along with a scope. The effects-first / signals-second drop
order in `Scope::drop` ensures the listener detaches while signals
are still live.

---

## Per-backend implementation

### Web

```rust
fn attach_pointer(&mut self, node: &Node, callback: Rc<dyn Fn(PointerEvent)>) {
    let el: &web_sys::Element = node.dyn_ref().unwrap();
    let id = self.next_pointer_listener_id();

    let translator = |evt: web_sys::PointerEvent, phase, el: &web_sys::Element| {
        let rect = el.get_bounding_client_rect();
        PointerEvent {
            x: evt.client_x() as f32 - rect.left() as f32,
            y: evt.client_y() as f32 - rect.top() as f32,
            pointer_id: evt.pointer_id() as u32,
            phase,
            pointer_type: match evt.pointer_type().as_str() {
                "mouse" => PointerType::Mouse,
                "touch" => PointerType::Touch,
                "pen"   => PointerType::Pen,
                _       => PointerType::Mouse,
            },
        }
    };

    // pointerdown / pointermove / pointerup / pointercancel — the
    // unified pointer events API covers mouse, touch, and pen with
    // one event family. Saves us writing three parallel
    // touch/mouse/pointer paths.
    let cb = callback.clone();
    let el_cloned = el.clone();
    let down = Closure::wrap(Box::new(move |evt: web_sys::PointerEvent| {
        el_cloned.set_pointer_capture(evt.pointer_id()).ok();
        cb(translator(evt, PointerPhase::Down, &el_cloned));
    }) as Box<dyn FnMut(_)>);
    el.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref()).unwrap();

    // …similar for move/up/cancel. Each Closure stashed so it
    // survives until release_pointer.
}
```

The `setPointerCapture` call is important — it locks subsequent
events for that pointer to the same element even when the cursor
leaves the visible bounds. That's what makes a drag continue when
the pointer slides off the element, matching native drag UX.

### Android

Touch listener on the `View`. Android's `MotionEvent` carries all
the data we need — multi-touch is exposed as a per-pointer-index
array; the backend translates to one `PointerEvent` per active
pointer.

```kotlin
view.setOnTouchListener { v, motionEvent ->
    // Translate every active pointer into a PointerEvent and
    // call back to Rust through JNI. The action code identifies
    // which pointer changed phase this tick.
    nativeTouchEvent(nativePtr, motionEvent.toFrameworkBundle(v))
    true
}
```

Coordinate translation: Android's `MotionEvent.getX(idx)` already
returns view-local coordinates, so the backend just forwards.
Pointer ids come from `getPointerId(idx)`.

Hover: Android exposes `setOnHoverListener` (API 14+); fires only
for mouse / stylus pointers on touch devices, which matches the
mouse-only contract.

### iOS

Two parallel paths because UIKit doesn't have a unified pointer API:

- For touches: subclass `UIView` with `touchesBegan/Moved/Ended/Cancelled`
  overrides. Native gesture recognizers stay out of it — we want raw
  events, not gesture interpretation.
- For mouse/trackpad/pencil on iPad: `UIPointerInteraction` (iOS 13.4+).

```objc
- (void)touchesBegan:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    for (UITouch *touch in touches) {
        CGPoint p = [touch locationInView:self];
        framework_pointer_event(/* …translate… */ PointerPhase::Down);
    }
}
```

`UITouch.estimationUpdateIndex` / `preciseLocation:` are available if
we later want pressure/tilt fields — out of scope here.

### Backends without an implementation

Default no-op. Views with `on_pointer` listeners silently never fire
on those platforms — same posture as every other "backend hasn't
implemented this yet" path.

---

## Composing gestures

The framework ships no named gestures. Authors build them as Rust
components reading the raw stream. A few canonical sketches:

### Tap

```rust
#[component]
pub fn tappable(props: &TappableProps, children: Vec<Primitive>) -> Primitive {
    let start: Signal<Option<(f32, f32, std::time::Instant)>> = signal!(None);
    let on_tap = props.on_tap.clone();
    let slop_px = 8.0_f32;
    let max_ms = 250_u128;

    ui! {
        View(style = props.style.clone()).on_pointer(move |evt| match evt.phase {
            PointerPhase::Down => start.set(Some((evt.x, evt.y, std::time::Instant::now()))),
            PointerPhase::Up => {
                if let Some((sx, sy, t0)) = start.get() {
                    let dx = (evt.x - sx).abs();
                    let dy = (evt.y - sy).abs();
                    let dt = t0.elapsed().as_millis();
                    if dx < slop_px && dy < slop_px && dt < max_ms {
                        on_tap();
                    }
                }
                start.set(None);
            }
            PointerPhase::Cancel => start.set(None),
            PointerPhase::Move => {}
        }) {
            children
        }
    }
}
```

A few dozen lines. Lives in user space. Customize the slop and
timing constants per design system — something that's hard to do
when the gesture is hardcoded in the framework.

### Long-press

Same shape, with a scheduled callback after a threshold:

```rust
#[component]
pub fn long_pressable(props: &LongPressableProps, children: Vec<Primitive>) -> Primitive {
    let task: Signal<Option<ScheduledTask>> = signal!(None);
    let on_long = props.on_long_press.clone();
    let threshold_ms = props.threshold_ms.unwrap_or(500);

    ui! {
        View.on_pointer(move |evt| match evt.phase {
            PointerPhase::Down => {
                let on = on_long.clone();
                let t = runtime_core::after_ms(threshold_ms, move || on());
                task.set(Some(t));
            }
            PointerPhase::Move | PointerPhase::Up | PointerPhase::Cancel => {
                if let Some(t) = task.update_take() { drop(t); }   // cancel
            }
        }) {
            children
        }
    }
}
```

Uses the existing `runtime_core::after_ms` scheduler — its
`ScheduledTask` drops cancel-on-drop, so simply replacing the
signal value aborts the pending callback.

### Drag

```rust
#[component]
pub fn draggable(props: &DraggableProps, children: Vec<Primitive>) -> Primitive {
    let start: Signal<Option<(f32, f32, u32)>> = signal!(None);
    let on_drag = props.on_drag.clone();

    ui! {
        View.on_pointer(move |evt| match evt.phase {
            PointerPhase::Down => start.set(Some((evt.x, evt.y, evt.pointer_id))),
            PointerPhase::Move => {
                if let Some((sx, sy, id)) = start.get() {
                    if id == evt.pointer_id {
                        on_drag(DragEvent { dx: evt.x - sx, dy: evt.y - sy, … });
                    }
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => start.set(None),
        }) {
            children
        }
    }
}
```

The `pointer_id` check is what makes this multi-touch-safe — only
the pointer that started the drag drives subsequent moves.

### Pinch

Two pointers tracked simultaneously:

```rust
let pointers: Signal<HashMap<u32, (f32, f32)>> = signal!(HashMap::new());
// …on Move with two active pointers, compute the distance ratio
//   against the initial-down distance to get a scale factor…
```

Twenty-some lines. Same shape as the others — track per-pointer
state, derive the gesture from coordinate deltas.

---

## A small companion library

The framework itself stays bare-bones. But there's room for a
companion crate — `framework-gestures` or similar — that ships the
common ones: tap, long-press, double-tap, swipe, drag, pinch,
rotate. Like `runtime-macros` is to `runtime-core`, the gesture
crate is a layer on top that authors who don't want to write the
tap component above pull in for free.

That crate doesn't have to be part of the framework's structural
contract. It's a userspace library, just one the framework's
maintainers happen to write. New gestures can land there without
touching `runtime-core` or any backend.

---

## Why builder-on-View rather than a separate primitive

Considered and rejected: a separate `Touchable` primitive that
wraps content.

The reasons builder-on-View wins:

1. **Every interactive container is structurally a View anyway.** A
   `Touchable` would, on every backend, create exactly the same
   native container as `View`, just with listeners attached. Two
   primitives for what's effectively "View with listeners" doubles
   the trait surface for no semantic gain.

2. **No coercion at the boundary.** A button-shaped helper like
   `Card { … }` returns a `Bound<ViewHandle>`. Tacking `.on_pointer`
   on the end fits naturally — same `Bound`, one more builder.
   With a separate primitive, the author has to remember "wrap in
   `Touchable` to make this interactive," and the `Bound<H>` type
   shifts at the wrap point.

3. **Match `attach_states` precedent.** The existing state-bit
   machinery already attaches event listeners to whichever View the
   user styled with state overlays. It's `attach_states(view_node,
   …)` — no `Touchable` wrapper. Pointer events extend that
   precedent.

4. **`None` is cheap.** Adding `on_pointer: Option<Rc<dyn Fn>>` to
   `View` costs one word per View at the Primitive layer (the enum
   already has Options for `style` and `ref_fill`), and the walker
   skips the attach call if it's None. A view without pointer
   listeners pays nothing in the backend.

The cost is `Primitive::View` grows two more fields. Worth it.

---

## What `Touchable` would look like if you want it anyway

For authors who prefer "I want a clearly-marked interactive
container," a one-liner component in userspace covers it:

```rust
#[component]
pub fn touchable<F: Fn(PointerEvent) + 'static>(on_pointer: F, children: Vec<Primitive>) -> Primitive {
    ui! { View.on_pointer(on_pointer) { children } }
}
```

Same shape as `Card`, lives in user code, costs zero framework
surface.

---

## Outstanding questions

1. **Should the callback fire inside or outside an Effect?** Pointer
   callbacks read signals and write to signals. If we wrap them in
   an `Effect`, the wrapping subscribes to whatever signals the
   callback reads on first run — which is wrong (the callback isn't
   reactive; it's an event handler). Current proposal: fire as a
   plain `Fn` callback, not inside an Effect. Same shape as
   `Button`'s `on_click`. Authors who *want* a signal subscription
   set one up explicitly with `Effect::new` inside their component.

2. **Should hover events report `pointer_type`?** Hover is mouse-
   and pen-only on every platform, so the field is redundant. The
   proposal omits it — `HoverEvent` is just (x, y, phase). Easy to
   add later if a need emerges.

3. **Should pointer events bubble?** Currently no — each View's
   `on_pointer` only sees events that landed on *that* view's
   geometry. A nested interactive button inside a draggable card
   would steal the pointer. That's the same posture every native
   platform has by default; authors who want bubbling implement it
   themselves by hoisting listeners. Reconsider if a need emerges.

4. **Should we expose a way for a component to "capture" an active
   pointer?** Web's `setPointerCapture` is invoked on `Down` by
   default in the proposed web backend impl, which means a drag
   that starts inside a View keeps firing events on that View even
   when the pointer slides off. But it doesn't let *another* view
   later capture the same active gesture. That's almost never
   needed (the use case is "drag handle is one element, drag
   surface is another," which is structurally rare). Defer.

5. **Wheel / scroll events?** Out of scope. Trackpad/mouse-wheel
   events are a separate event family; if needed, a parallel
   `.on_wheel` builder lands later. Touch scroll is a platform
   concern that lives inside `ScrollView`, not here.

6. **Keyboard tracking?** As you said, separate module. The
   intended shape is a `Keyboard::pressed(key) -> Signal<bool>` or
   similar — a global tracker that components subscribe to,
   parallel to but independent of the pointer event stream. Not
   part of this proposal.

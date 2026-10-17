# Refs

A **Ref** is how you get a programmatic handle to a mounted thing
— a view, a text input, a custom component — so you can call
methods on it imperatively. You'll reach for one whenever you need
to **trigger an action** on the thing rather than describe it
through reactive state.

The most common case isn't a built-in primitive. It's giving a
component you wrote an imperative surface — "reset this form,"
"scroll this list to the top," "focus this input," "play this
animation" — and calling it from a parent. This page leads with
that pattern because it's the one app code actually uses every
day.

## The two flavors

Refs come in two shapes:

1. **Component refs** — a handle to one of your own
   `#[component]` functions, generated from the nested fns you mark
   `#[method]`. Used to call imperative methods you declared on the
   component.
2. **Element refs** — a handle to a built-in primitive
   (`ViewHandle`, `TextInputHandle`, etc.). Used to trigger a press,
   read a frame, focus an input, scroll a list to the top.

The mechanics are the same in both cases. A `Ref<H>` is a slot. You
hand it to the thing in `ui!` (`bind_to = r` on a component,
`bind = r` on a primitive), the framework fills it at mount, and
`r.get()` gives you the handle. The two flavors differ only in
**what** the handle exposes.

## Component refs

This is the pattern you'll reach for most. A parent component
wants to imperatively trigger something inside a child — reset a
form, scroll a list, focus an input, play an animation. Instead
of routing the action through a prop and a signal flip, the
child **exposes the action as a method** and the parent **calls
it directly**.

### Declaring methods on a component

Inside a `#[component]` function body, mark nested fns with
`#[method]` to declare the imperative methods the component
exposes:

```rust
use runtime_core::{component, signal, ui, Element};

#[component]
pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
    let value = signal(initial);

    /// Reset the count to zero.
    #[method]
    fn reset() {
        value.set(0);
    }

    #[method]
    fn bump_by(n: i32) {
        value.update(|v| v + n);
    }

    #[method]
    fn set_to(n: i32) {
        value.set(n);
    }

    ui! {
        view {
            text { "Count: {value}" }
        }
    }
}
```

What the macro does with them:

1. **Each `#[method]` fn becomes a method on a generated
   `CounterHandle`** — the component name plus `Handle`. You don't
   write the struct.
2. **Method bodies run against captures from the component body**
   — here, the `value` signal. That's why there's no `&self` and no
   `pub`: the handle carries the public surface.
3. **Methods return `()`.** They are commands, not queries — read
   state through signals, not through the handle.
4. **The component gets an injected prop,
   `bind_to: Option<Ref<CounterHandle>>`.** Don't declare your own
   `bind_to` parameter on a component with methods.

The return type stays `Element`. Methods need the inline-props form
(props as fn parameters; zero parameters is fine) and a non-generic
component. A method's doc comment is kept, and the methods are also
invokable by name from the robot and the inspector.

Writes inside a method stage and commit at the flush, exactly the
way they would if the same write happened inside an `on_click`.

### Capturing the handle in a parent

The parent allocates a `Ref<CounterHandle>` and passes it through
`bind_to`:

```rust
use runtime_core::Ref;

#[component]
pub fn ParentApp() -> Element {
    let counter_ref: Ref<CounterHandle> = Ref::new();

    ui! {
        Counter(initial = 10, bind_to = counter_ref)

        button(
            label = "Reset",
            on_click = move || { counter_ref.get().map(|h| h.reset()); },
        )

        button(
            label = "Add 5",
            on_click = move || { counter_ref.get().map(|h| h.bump_by(5)); },
        )
    }
}
```

What happens at runtime:

1. `Ref::new()` allocates an empty slot.
2. The framework builds the counter. Once its body has built the
   handle, it fills `counter_ref` with a clone of it.
3. **Before that**, `counter_ref.get()` returns `None`, so the
   click does nothing. **After it**, `get()` returns the handle.
4. Calling `h.bump_by(5)` runs the method body, which updates
   `value`. The counter's text updates on the next flush.

Prefer `get()` over `with(|h| ...)` for component handles. `with`
holds a borrow of the ref table for the whole closure, and method
bodies write signals — `get()` clones the handle out first.

### Sibling-to-sibling through a parent

Two children of the same parent can talk to each other through
a ref the parent holds:

```rust
let target: Ref<CounterHandle> = Ref::new();

ui! {
    Counter(bind_to = target)

    button(
        label = "Set target to 100",
        on_click = move || { target.get().map(|h| h.set_to(100)); },
    )
}
```

The button doesn't know about the counter directly — it knows
about the ref. The ref is the wire that threads imperative
reach across the tree.

### Method vs signal prop — when to pick which

A natural question: "Should `reset` be a method, or should I
take a `Signal<bool>` prop the parent flips to trigger reset?"

**The method wins when:**
- The action is genuinely fire-and-forget — "do this thing now."
- Multiple call sites need to invoke it from different points in
  the tree.
- The action involves more than just changing one piece of state
  — focus shifts, animations, scroll position changes.

**The signal wins when:**
- The trigger is itself a piece of state worth observing.
- Sibling components also care about the same state and should
  react to it.

In practice, most apps mix both: signals for state that drives
UI, methods for actions the parent dispatches.

## Element refs

The framework's built-in primitives expose handles the same way.
Allocate a `Ref<H>`, pass it as `bind = r`, call methods:

```rust
use runtime_core::{Ref, ScrollViewHandle};

let list: Ref<ScrollViewHandle> = Ref::new();

ui! {
    scroll_view(bind = list) {
        // rows
    }

    button(
        label = "Back to top",
        on_click = move || {
            if let Some(h) = list.get() {
                h.scroll_to_top();
            }
        },
    )
}
```

The primitives that take `bind` in `ui!` are `view`, `text`,
`button`, `icon`, `image`, `text_input`, `text_area`, and
`scroll_view`. The handle types you'll meet most:

- **`ViewHandle`** — `.frame()`, `.absolute_frame()` for geometry
- **`TextInputHandle`** — `.focus()`, `.blur()`, `.select_all()`
- **`ScrollViewHandle`** — `.scroll_to(x, y)`, `.scroll_to_top()`
- **`PressableHandle`** and the button handle — `.click()` fires
  the press
- **`StackHandle`** / **`SwapHandle`** — navigator commands
  (`.push`, `.pop`, …), bound with `.bind(r)` on the navigator
  builder; see [Navigation](navigation)

Other primitives have handles too (`TextHandle`, `ImageHandle`,
`IconHandle`, `TextAreaHandle`, `PresenceHandle`, `PortalHandle`).
Third-party SDKs ship their own — e.g. `webview::WebViewHandle`
with `.post_message()` / `.reload()` / `.execute_js()`.

The button's handle type is not re-exported from `runtime_core`, so
you can't name it. Let inference pick it: `let b = Ref::new();`
followed by `button(bind = b, …)` types `b` from the prop.

### Reading geometry

A common primitive-ref use is reading a view's bounding rect:

```rust
let row: Ref<ViewHandle> = Ref::new();

ui! {
    view(bind = row) { /* content */ }

    button(label = "Where am I?", on_click = move || {
        if let Some(rect) = row.get().and_then(|h| h.frame()) {
            println!("row is at {:?}", rect);
        }
    })
}
```

Two methods:

- **`.frame()`** — rect in the *parent's* coordinate system
- **`.absolute_frame()`** — rect in viewport (window) coordinates

Both return `Option<ViewportRect>` because layout might not have
happened yet, or the backend might not expose geometry. Backends
that do: web (`getBoundingClientRect`), iOS (`UIView.frame` +
`UIView.convert`), Android (`View.getLocationOnScreen`).

### Anchoring overlays

`ViewHandle`, `PressableHandle`, and the button handle implement
the `AnchorableHandle` trait. `anchored_overlay` reads it to
position itself relative to the anchor:

```rust
use runtime_core::{AnchorTarget, ElementAlign, ElementSide, Ref, ViewHandle};

let trigger: Ref<ViewHandle> = Ref::new();
let is_open = signal(false);

ui! {
    view(bind = trigger) {
        button(
            label = "Open menu",
            on_click = move || is_open.update(|v| !v),
        )
    }

    if is_open.get() {
        anchored_overlay(
            target = AnchorTarget::from(trigger),
            side = ElementSide::Below,
            align = ElementAlign::Start,
        ) {
            // menu content
        }
    }
}
```

The overlay tracks the anchor through scrolls, layout shifts,
and orientation changes — same rect machinery as
`.absolute_frame()`, read continuously. See [Portal](portal)
for how overlays mount.

## The Ref<H> API

```rust
let r: Ref<H> = Ref::new();

r.get();                           // Option<H> — clone of the handle once filled
r.with(|h| h.some_method());      // Option<R> — borrow instead of clone
r.is_mounted();                    // bool — has the slot been filled
```

A `Ref<H>` is a `Copy` token, like `Signal<T>`. Pass it into
closures, into child components, anywhere, without `.clone()`. The
token indexes a thread-local ref table; it is separate from the
world that holds your signals.

Inside `with(|h| ...)` the table is borrowed, so don't write
signals or call component methods from that closure. Read values
out, or use `get()` and act after.

Allocate refs early — typically right at the top of a component
— and bind them inline in the `ui!` block. The slot stays empty
until the primitive (or component) mounts.

## Binding forms

Where a ref goes depends on what you're binding:

```rust
// Primitives in ui!: the `bind` prop.
view(bind = view_ref) { /* ... */ }
button(label = "...", on_click = on_click, bind = button_ref)
scroll_view(bind = scroll_ref) { /* ... */ }

// Components with #[method] fns: the injected `bind_to` prop.
Counter(initial = 0, bind_to = counter_ref)

// Builders outside ui! (navigators, `pressable`): `.bind(r)`.
StackNavigator::new(&HOME).screen(DETAIL, detail_screen).bind(nav_ref)
```

The handle type has to match the primitive (or component) kind —
binding is type-checked. Passing a `Ref<ScrollViewHandle>` to a
`view` is a compile error.

## Lifecycle

A ref's slot is empty until the thing it's bound to mounts. Before
that, `.get()` returns `None` and `.is_mounted()` is `false`. A
primitive fills the slot when it mounts; a component fills it when
its body builds the handle. If the bound thing mounts again — an
`if` branch flipping back, a list row rebuilt — the slot is
refilled with the new handle.

When the bound thing unmounts, its slot empties: after an `if`
branch flips away, `.get()` returns `None`, so
`r.get().map(|h| h.reset())` is skipped instead of reaching an element
that is gone. If the new element mounts before the old one tears down,
the new handle stays.

## Refs vs Signals

Two distinctions worth keeping straight:

- **`Signal<T>`** — reactive state. Reads inside tracked contexts
  subscribe; writes notify subscribers. Use for anything that
  changes over time and drives UI.
- **`Ref<H>`** — an imperative handle slot, filled at mount.
  Reading it doesn't subscribe to anything. Use to invoke methods
  on a mounted thing.

Both are `Copy` tokens; signals live in the world, refs in a
thread-local ref table. They solve different problems and coexist
freely — you'll often fire a ref-driven action from inside a
signal-driven event handler.

## Where to read more

- [Components](components) — `#[component]`, inline props,
  and `#[method]`.
- [Navigation](navigation) — `StackHandle`, `SwapHandle` and
  their command surfaces.
- [Portal](portal) — how `anchored_overlay` mounts and
  anchors.
- [Primitives](primitives) — the full list of primitives and
  their handles.

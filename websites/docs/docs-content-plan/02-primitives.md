# Primitives

Primitives are the fixed set of things the framework knows how to put on
screen. Every Idealyst app — and every component library built on top of
the framework, including idea-ui — reduces to a tree of these.

Each primitive is a mount handler on the scene registry, generic over
the capability traits it calls through. Adding one to the FIRST-PARTY
set means adding a capability every backend is then expected to carry,
so the cost is high and deliberately so — but the registry itself is
open: a third-party crate registers its own payload the same way (see
[Third-party primitives](third-party-primitives)). What you build out of
the primitives is unbounded.

## What every primitive shares

A few things are true of every primitive on this page, so they're worth
saying once instead of repeating per entry:

- Styles are orthogonal. Every primitive takes an optional `style` slot.
  A primitive can have any style applied to it, and styling is its own
  subsystem — see Styles for the full story.
- Refs are optional. Primitives take `bind = my_ref` so parent code can
  hold a handle to the underlying node and call imperative methods on
  it. See Refs.
- Test ids are optional. Primitives accept a `test_id` in every build;
  when the app is built with `--features robot`, the Robot introspection
  layer uses it to find the node. See Robot.
- Some primitives are reactive on their content. When an input is a
  closure that reads a signal (or a `format!` that reads `.get()`), the
  framework wraps that read in an Effect and updates the live node when
  the signal changes. The relevant primitives note this below.

## Containers

The structural primitives — boxes, scrolling boxes, and tappable boxes.
Everything else nests inside one of these at some level.

### View

A generic box. The structural workhorse — everything composes inside a
View at some level.

```rust
ui! {
    view(style = my_view_style()) {
        text { "hello" }
        text { "world" }
    }
}
```

Children are a flat list of primitives, laid out by the platform's flex
engine (via runtime-layout on native backends, via the browser on web).
A View has no behavior of its own — no press target, no scrolling, no
clipping unless its style says so.

Optional `safe_area` opts the view into per-side safe-area padding. The
backend reactively adds the platform's inset (status bar, home
indicator, dynamic island) to the matching sides; rotation and
dynamic-island changes propagate without a rebuild.

### ScrollView

A View that scrolls. Vertical by default; `horizontal = true` flips the
axis.

```rust
ui! {
    scroll_view {
        // ...children scroll vertically
    }
}
```

Maps to a `div` with `overflow: scroll` on web, `UIScrollView` on iOS,
`ScrollView` or `HorizontalScrollView` on Android. Like `View`, it can
opt into safe-area padding per side — useful at the screen root so
content can pass under the status bar while headers respect the inset.

### pressable

A View that's also tappable. No native chrome — the visual is whatever
its children and style say it is. It is a function, not a `ui!` tag:
`pressable(children, on_click)`.

```rust
use runtime_core::{pressable, ui};

let row = ui! { text { "Open" } };
ui! {
    { pressable(vec![row], move || open_sheet()) }
}
```

Use pressable when you want button behavior without button visuals:
tappable card surfaces, menu rows, custom-styled buttons whose look is
owned by the stylesheet. For a button with a label and native semantics
(form submission, default focus ring), use `Button` instead.

State styling works the same as any other primitive — `state hovered {
... }`, `state pressed { ... }`, `state focused { ... }`, `state
disabled { ... }` blocks in a stylesheet apply automatically.

## Content

Primitives that render media — text runs, bitmaps, vector icons, and
embedded web content.

### Text

A run of text.

```rust
ui! {
    text { "Hello, Idealyst" }
    text { format!("Count: {}", count.get()) }
}
```

The child can be any expression that produces a string. If the
expression reads a signal (via `.get()`), the framework wraps the read
in an Effect and updates the live node when the signal changes. A static
literal is computed once and never updated.

### Image

A bitmap from a URL.

```rust
ui! {
    image(src = "https://example.com/avatar.png", alt = "Avatar")
}
```

`src` accepts a string or a closure for reactive URLs. `alt` maps to the
platform accessibility label (`alt` on web, `accessibilityLabel` on iOS,
`contentDescription` on Android). Codec support is whatever the platform
handles natively.

### Icon

A vector icon, rendered as inline SVG on web, `CAShapeLayer` on iOS, and
an `android.graphics.Path` drawable on Android.

```rust
ui! {
    icon(data = icons_lucide::CHEVRON_RIGHT)
}
```

The icon registry is tree-shakeable — only icons referenced by your code
end up in the binary. Icons support a reactive `color` override and a
stroke-draw animation (the path can progressively reveal itself). See
the Icons page for the registry and authoring custom icons.

### Video

Video playback lives in the `video` SDK crate, not the builtin primitive
set. `AVPlayerLayer` on iOS and macOS, `android.widget.VideoView` on
Android, `<video>` on web. Add the dependency and pass `video::register`
to the app's boot call (`start_in("#app", video::register, app)`), which
installs its handler on the scene registry.

```rust
use video::prelude::*;
ui! {
    { Video(VideoProps {
        source: url("https://example.com/clip.mp4"),
        autoplay: true,
        controls: true,
        ..Default::default()
    }) }
}
```

### WebView

Embedded web content lives in the `webview` SDK crate, not the builtin
primitive set. An iframe on web, WebKitGTK on Linux, WebView2 on
Windows. iOS and Android have no native renderer yet and show the host's
"not supported" placeholder. Pass `webview::register` to the app's boot
call, then use the tag:

```rust
use webview::WebView;

ui! {
    view {
        WebView(url = webview::url("https://example.com"))
    }
}
```

## Inputs

All four input primitives are controlled: the parent owns the value as a
signal, and the input round-trips through `on_change`.

### Button

A labeled tappable with native button semantics.

```rust
ui! {
    button(
        label = "Increment",
        on_click = move || count.update(|n| n + 1),
    )
}
```

Optional `leading_icon` / `trailing_icon` render before/after the label
using the platform's native button-icon API (`UIButton.setImage` on iOS,
compound drawable on Android, inline SVG on web).

The optional `disabled` is a reactive flag — a closure that returns
`bool`. When it flips, the framework marks the native widget inert and
toggles the `state disabled` styling block.

### TextInput

A text field whose value is owned by the parent.

```rust
let value = signal(String::new());

ui! {
    text_input(
        value = value,
        on_change = move |s| value.set(s),
        placeholder = "Search...",
    )
}
```

The framework writes the value back into the native widget when the
signal changes (cyclic but stable — widgets no-op when set to their
current value).

### Toggle

A switch / checkbox bound to a `Signal<bool>`.

```rust
let enabled = signal(true);

ui! {
    toggle(
        value = enabled,
        on_change = move |v| enabled.set(v),
    )
}
```

The optional `disabled` takes a plain `bool` or a live source. While it
is on, the switch can't be flipped or focused, `on_change` never fires,
and the `state disabled` styling block applies. Every backend uses its
native disabled state.

### Slider

A numeric slider with min/max bounds and an optional step.

```rust
let volume = signal(50.0);

ui! {
    slider(
        value = volume,
        on_change = move |v| volume.set(v),
        min = 0.0,
        max = 100.0,
        step = 5.0,
    )
}
```

When `step` is set, the framework snaps incoming `on_change` values to
the nearest step before dispatching — behavior matches across backends,
regardless of whether the platform widget supports stepping natively.

`slider` takes the same `disabled` as `toggle`: the knob can't be
dragged or moved with the keyboard, and `on_change` never fires.

## Feedback

Primitives whose only job is to show that something is happening.

### ActivityIndicator

An indeterminate loading spinner. No methods, no value — it just spins.
`size` is `Small` (the default) or `Large`; `color` is a `Color`.

```rust
ui! {
    activity_indicator(size = ActivityIndicatorSize::Large, color = my_color)
}
```

## Reactive control flow

These forms express dynamic structure: they decide what to build based
on a signal, and rebuild atomically when the signal changes. You write
ordinary `if`, `match`, and `for` inside `ui!`; the macro picks the
lowering.

### When

Reactive `if`/`else`. You usually don't construct this directly — write
a plain `if` inside `ui!` whose condition reads a signal, and the macro
lowers it to `when(...)`.

```rust
ui! {
    if logged_in.get() {
        text { "Welcome back!" }
    } else {
        button(label = "Log in", on_click = move || logged_in.set(true))
    }
}
```

When the condition changes, the old branch's scope drops (freeing every
signal, effect, and node inside it) and the new branch builds in a fresh
scope. See Reactivity for how this works.

### Switch

Reactive multi-way match — the n-way version of `when`. Write a `match`
inside `ui!` whose scrutinee reads a signal, and the macro lowers it to
`switch(...)`.

```rust
ui! {
    match mode.get() {
        Mode::Loading => { activity_indicator() }
        Mode::Ready => { text { "ready" } }
        _ => { text { "error" } }
    }
}
```

The scrutinee type must be `PartialEq`: the arm is rebuilt only when the
value actually changes.

### Repeat

Bulk children. A `for` inside `ui!` over a static range with a
single-node body (`for i in 0..n { ... }`) lowers to a repeat, which
mounts the whole expansion in one batched backend call — one FFI hop
instead of N.

Other loops pick their lowering from the iterator's type: a plain `Vec`
or slice builds its rows once, and a `Signal<Vec<T>>` becomes a keyed
reactive list (`for item in items, key = item.id { ... }`).

```rust
ui! {
    view {
        for item in items, key = item.id {
            Card { text { item.title.clone() } }
        }
    }
}
```

For large lists you want `flat_list`, which only mounts visible rows —
see below.

## Lists

The virtualized-list entry point. Use this when row count is large
enough that materializing every row would be wasteful.

### Virtualizer (via flat_list)

A virtualized list that only realizes the visible rows. The typed entry
point is `flat_list(data, key, size, render)`.

```rust
ui! {
    flat_list(
        data = signal_of_items,
        key = |_idx, item| item.id,
        size = fixed_size(72.0),
        render = |_idx, item| ui! { Card { text { item.title.clone() } } },
    )
}
```

Backends drive their native virtualization widget (`UICollectionView` on
iOS, `RecyclerView` on Android, a scroll-listener windowing engine on
web).

See Lists for keying, overscan, item-size strategies, and horizontal
lists.

## Navigation

Navigation has its own page — these are the entry points.

### Navigator

`navigator` is the one navigation primitive — a registry-dispatched
extension point the navigator SDKs build on. Two SDK behaviors exist:
`swap-navigator` (a flat set of co-equal screens switched via `Select`)
and `stack-navigator` (push/pop with depth). Both take a declarative
route table via `.screen(route, |params| ...)` builders and an author
`.layout(|nav| ...)` that wraps the navigator's single outlet (`{
nav.outlet }`) in whatever chrome the design calls for — a tab bar, a
drawer, a header. The docs site's shell is a swap navigator inside an
`idea_ui_nav::AppShell`; there is no separate tab or drawer navigator
kind.

Backends stay native where it matters: the stack mounts a real
`UINavigationController` on iOS and the Fragment back-stack on Android
inside its outlet (native transitions and swipe-back, native bar
hidden), while web gets automatic History-API URL sync.

### Link

Declarative navigation. Wraps a child that, when pressed, dispatches a
`NavCommand` against the closest ambient navigator.

```rust
ui! {
    link(route = &PROFILE_ROUTE, params = ProfileParams { id: 42 }) {
        text { "Open profile" }
    }
}
```

`route` takes a typed `&Route<P>` constant
(`Route::<ProfileParams>::new("profile", "/profile/:id")`), resolved
against the enclosing navigator. For an off-app URL use `external =
"https://…"` instead.

On web, the wrapper emits an `<a href=…>` so right-click
"open-in-new-tab" works. On native, the wrapper is invisible and the
press dispatches in-process.

See Navigation for the full route / params / dispatch model.

## Floating UI — Portal, Overlay, AnchoredOverlay

Floating subtrees that escape the parent's layout and clipping — modals,
popovers, drawers, sheets, tooltips.

### Portal

The one render-elsewhere primitive. `portal` renders its children at a
different location in the host tree, escaping the parent's layout and
clipping. On each backend it mounts at the platform's window-level
surface — body portal on web, key-window addSubview on iOS, window-level
addView on Android.

`PortalTarget` carries both the mount location AND the positioning
intent: `Viewport(placement)` for window-relative (centered,
edge-pinned, full-screen) and `Anchor { target, side, align, offset }`
for element-tracking (popovers, dropdowns, tooltips). The backend
re-queries an anchored portal's rect on each scroll / layout /
orientation event and repositions automatically.

See [Portal & Overlays](portal) for the full target model, dismissal
contract, focus-trap semantics, and authoring novel floating UX directly
against the primitive.

### overlay (composition)

`overlay()` is not a primitive — it's a composition that lowers to
`portal` with a viewport target plus a backdrop child. Defaults:
`Center` placement, `Dismiss` backdrop, focus-trap on. Use for modals,
drawers, sheets.

The host owns open/close state. Mounting opens the overlay; unmounting
closes it. Wire `on_dismiss` to flip your open-state signal when the
platform requests dismissal (Escape, back gesture, backdrop tap).

### anchored_overlay (composition)

`anchored_overlay()` is also a composition, lowering to `portal` with
`PortalTarget::Anchor`. Use for popovers, tooltips, dropdowns, context
menus — anything that follows a trigger element.

Defaults: `Below` side, `Start` align, `BackdropMode::None` (page behind
stays interactive), focus-trap off — the popover defaults.

### Presence

Mount/unmount with enter and exit animations. Backed by a `Signal<bool>`
for the present/absent state; the framework defers the actual unmount
until the exit animation's duration elapses, so the leaving subtree
stays alive long enough to play its exit.

Presence adds no box of its own on any backend: its child lays out
exactly as if it were the direct child of presence's parent. A
`position: Absolute` child resolves its insets against presence's
parent, a `flex_grow` child fills, and a column of presence-wrapped
toasts keeps its gap. The enter and exit animation moves the child
itself.

See Overlays and animation for placement, backdrop modes, focus
trapping, and the animation primitives.

## Graphics

A GPU canvas primitive. You own the rendering; the framework hands you a
wgpu device and stays out of the way.

### Graphics primitive

A GPU canvas. You own the rendering — `on_ready` runs once after the
backend has a `wgpu` device available and produces your render state;
`on_resize` and (implicitly) per-frame callbacks let you update.

The framework does not interpret any of it. The GPU context is
type-erased, so runtime-core stays wgpu-free even though backends that
support graphics carry the dependency.

Not supported in runtime-server dev mode — the wire protocol can't ship
GPU work, so an runtime-server host renders a placeholder. Local-render
mode is required.

See Graphics for the lifecycle, surface configuration, and the
constraints.

## What about styles?

Every primitive accepts an optional `style` and an optional set of
state-driven overrides. The mechanics of how a stylesheet is declared,
themed, and resolved is its own subsystem — see Styles for the model.

## Where to read more

- Reactivity — reactive `if`, `match`, and `for` rest on the reactive
  substrate.
- Styles — the styling system every primitive's `style` slot feeds into.
- Refs — programmatic handles on a primitive.
- Navigation — `SwapNavigator`, `StackNavigator`, the outlet model, and
  `Link`.
- Lists — `flat_list` in depth.
- [Portal & Overlays](portal) — `Portal` (the primitive), `overlay()` /
  `anchored_overlay()` (compositions), and `Presence`.
- [Third-party primitives](third-party-primitives) — registering your
  own payload handler for primitives that need per-platform FFI.
- Graphics — the wgpu canvas primitive.
- Robot — `test_id` and the introspection layer.

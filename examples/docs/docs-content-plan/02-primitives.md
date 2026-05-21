# Primitives

Primitives are the fixed set of things the framework knows how to put
on screen. Every Idealyst app — and every component library built on
top of the framework, including idea-ui — reduces to a tree of these.

There is no way to add a new primitive without changing the framework
itself, because each primitive corresponds to a method on the
`Backend` trait that every backend has to implement. The cost of
adding one is high; that's deliberate. What you build out of the
primitives is unbounded.

## What every primitive shares

A few things are true of every primitive on this page, so they're
worth saying once instead of repeating per entry:

- **Styles are orthogonal.** Every primitive takes an optional
  `style` slot. A primitive can have any style applied to it, and
  styling is its own subsystem — see [Styles](#) for the full
  story.
- **Refs are optional.** Every primitive takes an optional ref so
  parent code can hold a handle to the underlying node and call
  imperative methods on it. See [Refs](#).
- **Test ids are optional.** With `--features robot`, every
  primitive accepts a `test_id` that the Robot introspection layer
  uses to find it. See [Robot](#).
- **Some primitives are reactive on their content.** When an input
  is a closure that reads a signal (or a `format!` that reads
  `.get()`), the framework wraps that read in an Effect and updates
  the live node when the signal changes. The relevant primitives
  note this below.

## Containers

### `View`

A generic box. The structural workhorse — everything composes inside
a View at some level.

```rust
ui! {
    View(style = my_view_style()) {
        Text { "hello" }
        Text { "world" }
    }
}
```

Children are a flat list of primitives, laid out by the platform's
flex engine (via [framework-native-layout](#) on native backends, via
the browser on web). A View has no behavior of its own — no press
target, no scrolling, no clipping unless its style says so.

Optional `safe_area_sides` opts the view into per-side safe-area
padding. The backend reactively adds the platform's inset (status
bar, home indicator, dynamic island) to the matching sides; rotation
and dynamic-island changes propagate without a rebuild.

### `ScrollView`

A View that scrolls. Vertical by default; `horizontal = true` flips
the axis.

```rust
ui! {
    ScrollView {
        // ...children scroll vertically
    }
}
```

Maps to a `div` with `overflow: scroll` on web, `UIScrollView` on
iOS, `ScrollView` or `HorizontalScrollView` on Android. Like `View`,
it can opt into safe-area padding per side — useful at the screen
root so content can pass under the status bar while headers respect
the inset.

### `Pressable`

A View that's also tappable. No native chrome — the visual is
whatever its children and style say it is.

```rust
ui! {
    Pressable(on_click = move || open_sheet()) {
        Card { /* ... */ }
    }
}
```

Use Pressable when you want button *behavior* without button
*visuals*: tappable card surfaces, menu rows, custom-styled buttons
whose look is owned by the stylesheet. For a button with a label and
native semantics (form submission, default focus ring), use `Button`
instead.

State styling works the same as any other primitive — `state hovered
{ ... }`, `state pressed { ... }`, `state focused { ... }`, `state
disabled { ... }` blocks in a stylesheet apply automatically.

## Content

### `Text`

A run of text.

```rust
ui! {
    Text { "Hello, Idealyst" }
    Text { format!("Count: {}", count.get()) }
}
```

The child can be any expression that produces a string. If the
expression reads a signal (via `.get()`), the framework wraps the
read in an Effect and updates the live node when the signal changes.
A static literal is computed once and never updated.

### `Image`

A bitmap from a URL.

```rust
ui! {
    Image(src = "https://example.com/avatar.png", alt = "Avatar")
}
```

`src` accepts a string or a closure for reactive URLs. `alt` maps to
the platform accessibility label (`alt` on web,
`accessibilityLabel` on iOS, `contentDescription` on Android). Codec
support is whatever the platform handles natively.

### `Icon`

A vector icon, rendered as inline SVG on web, `CAShapeLayer` on iOS,
`VectorDrawable` on Android.

```rust
ui! {
    Icon(name = "chevron-right")
}
```

The icon registry is tree-shakeable — only icons referenced by your
code end up in the binary. Icons support a reactive `color` override
and a stroke-draw animation (the path can progressively reveal
itself). See the [Icons](#) page for the registry and authoring
custom icons.

### `Video`

Video playback. URL-only — backends use their native players, so
codec support is whatever the platform handles.

```rust
ui! {
    Video(src = "https://...", autoplay = true, controls = true, loop_playback = false)
}
```

### `WebView`

Embedded web content. A sandboxed iframe on web, `WKWebView` on iOS,
`android.webkit.WebView` on Android.

```rust
ui! {
    WebView(url = "https://example.com")
}
```

## Inputs

All four input primitives are **controlled**: the parent owns the
value as a signal, and the input round-trips through `on_change`.

### `Button`

A labeled tappable with native button semantics.

```rust
ui! {
    Button(
        label = "Increment",
        on_click = move || count.update(|n| *n += 1),
    )
}
```

Optional `leading_icon` / `trailing_icon` render before/after the
label using the platform's native button-icon API (`UIButton.setImage`
on iOS, compound drawable on Android, inline SVG on web).

The optional `disabled` is a reactive flag — a closure that returns
`bool`. When it flips, the framework marks the native widget inert
and toggles the `state disabled` styling block.

### `TextInput`

A text field whose value is owned by the parent.

```rust
let value = signal!(String::new());

ui! {
    TextInput(
        value = value,
        on_change = move |s| value.set(s),
        placeholder = "Search...",
    )
}
```

The framework writes the value back into the native widget when the
signal changes (cyclic but stable — widgets no-op when set to their
current value).

### `Toggle`

A switch / checkbox bound to a `Signal<bool>`. The builder function
is `switch(...)`; the primitive variant is `Toggle`. They're the
same thing — the constructor is named for what you'd call the
control on screen.

```rust
let enabled = signal!(true);

ui! {
    Switch(
        value = enabled,
        on_change = move |v| enabled.set(v),
    )
}
```

### `Slider`

A numeric slider with min/max bounds and an optional step.

```rust
let volume = signal!(50.0);

ui! {
    Slider(
        value = volume,
        on_change = move |v| volume.set(v),
        min = 0.0,
        max = 100.0,
        step = Some(5.0),
    )
}
```

When `step` is set, the framework snaps incoming `on_change` values
to the nearest step before dispatching — behavior matches across
backends, regardless of whether the platform widget supports
stepping natively.

## Feedback

### `ActivityIndicator`

An indeterminate loading spinner. No methods, no value — it just
spins.

```rust
ui! {
    ActivityIndicator(size = ActivityIndicatorSize::Medium, color = Some(my_color))
}
```

## Reactive control flow

These three primitives express dynamic structure: they decide what to
build based on a signal, and rebuild atomically when the signal
changes.

### `When`

Reactive `if`/`else`. You usually don't construct this directly —
write a plain `if` inside `ui!` whose condition reads a signal, and
the macro lowers it to `When`.

```rust
ui! {
    if logged_in.get() {
        Text { "Welcome back!" }
    } else {
        Button(label = "Log in", on_click = move || logged_in.set(true))
    }
}
```

When the condition changes, the old branch's scope drops (freeing
every signal, effect, and node inside it) and the new branch builds
in a fresh scope. See [Reactivity](#) for how this works.

### `Switch`

Reactive multi-way match — the n-way version of `When`. Each arm has
a JSON-serializable pattern; the framework picks the first arm whose
pattern equals the discriminant.

```rust
ui! {
    Switch(discriminant = mode) {
        Arm(pattern = "loading") { ActivityIndicator() }
        Arm(pattern = "ready") { Body { /* ... */ } }
        Default { Text { "error" } }
    }
}
```

(The exact `ui!` syntax for this is settling — see [Reactive
control flow](#) for current shape.)

The JSON constraint exists because the match must round-trip
through both the runtime path and the generator-backend wire format
with the same equality semantics.

### `Repeat`

Bulk children. The macro lowers `for i in 0..n { ... }` inside `ui!`
into `Repeat`, which collapses the whole expansion into a single
batched backend call when the rows are simple enough — one FFI hop
instead of N.

You don't write `Repeat` by hand; you write a `for`.

```rust
ui! {
    View {
        for item in items {
            Card { Text { item.title } }
        }
    }
}
```

For large lists you want a `Virtualizer`, not a `Repeat` — see
below.

## Lists

### `Virtualizer` (via `flat_list<T>`)

A virtualized list that only realizes the visible rows. The typed
entry point is `flat_list<T>(items, render_item, …)`.

```rust
ui! {
    flat_list(
        items = signal_of_items,
        item_size = ItemSize::Fixed(72.0),
        render_item = |i, item| ui! { Card { Text { &item.title } } },
    )
}
```

Backends drive their native virtualization widget (`UICollectionView`
on iOS, `RecyclerView` on Android, intersection-observer-based on
web). For Roku and other generator backends, the row template is
pre-built once and the device runtime materializes per-row
instances.

See [Lists](#) for keying, overscan, item-size strategies, and
horizontal lists.

## Navigation

Navigation has its own page — these are the entry points.

### `Navigator`

A stack-based navigator. Push, pop, replace, reset, with a
declarative route table built up via `Screen(route = ..., title =
...)` children. Backends own the platform-native stack
(`UINavigationController` on iOS, `FragmentManager` on Android, an
inline subtree swap on web).

### `TabNavigator`

A tab bar plus a switched content region. An ordered list of
`Screen` entries plus a route table.

### `DrawerNavigator`

A slide-in side panel plus a switched body region. Can be pinned
beside the body above a viewport-width breakpoint (becomes a
sidebar). The docs site uses this at the top level.

### `Link`

Declarative navigation. Wraps a child that, when pressed, dispatches
a `NavCommand` against the closest ambient navigator.

```rust
ui! {
    Link(route = "/profile/:id", params = ProfileParams { id: 42 }) {
        Text { "Open profile" }
    }
}
```

On web, the wrapper emits an `<a href=…>` so right-click
"open-in-new-tab" works. On native, the wrapper is invisible and the
press dispatches in-process.

See [Navigation](#) for the full route / params / dispatch model.

## Floating UI — Portal, Overlay, AnchoredOverlay

Floating subtrees that escape the parent's layout and clipping —
modals, popovers, drawers, sheets, tooltips.

### `Portal`

The one render-elsewhere primitive. `Primitive::Portal` renders
its children at a different location in the host tree, escaping
the parent's layout and clipping. On each backend it mounts at
the platform's window-level surface — body portal on web,
key-window addSubview on iOS, window-level addView on Android.

`PortalTarget` carries both the mount location AND the
positioning intent: `Viewport(placement)` for window-relative
(centered, edge-pinned, full-screen) and
`Anchor { target, side, align, offset }` for element-tracking
(popovers, dropdowns, tooltips). The backend re-queries an
anchored portal's rect on each scroll / layout / orientation
event and repositions automatically.

See [Portal & Overlays](#) for the full target model, dismissal
contract, focus-trap semantics, and authoring novel floating UX
directly against the primitive.

### `Overlay` (composition)

`overlay()` is not a primitive — it's a composition that lowers
to `Primitive::Portal` with a viewport target plus a backdrop
child. Defaults: `Center` placement, `Dismiss` backdrop,
focus-trap on. Use for modals, drawers, sheets.

The host owns open/close state. Mounting opens the overlay;
unmounting closes it. Wire `on_dismiss` to flip your open-state
signal when the platform requests dismissal (Escape, back
gesture, backdrop tap).

### `AnchoredOverlay` (composition)

`anchored_overlay()` is also a composition, lowering to
`Primitive::Portal` with `PortalTarget::Anchor`. Use for
popovers, tooltips, dropdowns, context menus — anything that
follows a trigger element.

Defaults: `Below` side, `Start` align, `BackdropMode::None`
(page behind stays interactive), focus-trap off — the popover
defaults. Backends can route the underlying `Portal` to a native
anchored presentation (`UIContextMenuInteraction`,
`UIPopoverPresentationController`, Android `PopupWindow`, web
`popover` + CSS anchor positioning) or fall back to manual
positioning with a scroll-tracking observer.

### `Presence`

Mount/unmount with enter and exit animations. Backed by a
`Signal<bool>` for the present/absent state; the framework defers
the actual unmount until the exit animation's duration elapses, so
the leaving subtree stays alive long enough to play its exit.

See [Overlays and animation](#) for placement, backdrop modes,
focus trapping, and the animation primitives.

## Graphics

### `Graphics`

A GPU canvas. You own the rendering — `on_ready` runs once after the
backend has a `wgpu` device available and produces your render
state; `on_resize` and (implicitly) per-frame callbacks let you
update.

The framework does not interpret any of it. The GPU context is
type-erased, so framework-core stays wgpu-free even though backends
that support graphics carry the dependency.

Not supported in AAS dev mode — the wire protocol can't ship GPU
work, so an AAS host renders a placeholder. Local-render mode is
required.

See [Graphics](#) for the lifecycle, surface configuration, and the
constraints.

## What about styles?

Every primitive accepts an optional `style` and an optional set of
state-driven overrides. The mechanics of how a stylesheet is
declared, themed, and resolved is its own subsystem — see
[Styles](#) for the model.

## Where to read more

- [Reactivity](#) — `When`, `Switch`, and `Repeat` rest on the
  reactive substrate.
- [Styles](#) — the styling system every primitive's `style` slot
  feeds into.
- [Refs](#) — programmatic handles on a primitive.
- [Navigation](#) — `Navigator`, `TabNavigator`, `DrawerNavigator`,
  `Link`.
- [Lists](#) — `Virtualizer` / `flat_list` in depth.
- [Portal](#) — `Portal`, `overlay()`, `anchored_overlay()`,
  `Presence`.
- [Graphics](#) — the wgpu canvas primitive.
- [Robot](#) — `test_id` and the introspection layer.

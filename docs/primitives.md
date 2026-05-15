# Primitives

Primitives are the framework's **structural vocabulary**. They are the
smallest set of "things the renderer knows about" — every backend
implements them, every component composes them, every higher-level
widget is built out of them.

Application authors don't usually build their own primitives. They
build *components* out of primitives — that's the [`ui-layer.md`](./ui-layer.md)
story. Primitives are the bottom of the stack the framework gives
you. Everything you'd recognize as a UI library — buttons, cards,
modals, tabs, forms, design-system kits — is **your** code,
composed of primitives.

This doc explains what a primitive *is*, what the existing primitives
provide, and how each one's contract is shaped so you can pick the
right one when building.

---

## What is a primitive?

Concretely, a primitive is a variant of the `framework_core::Primitive`
enum. Three views on what that means:

**Data view.** A `Primitive` is an inert tree node — a description of
"a `Button` with this label, this click handler, this style," not the
button itself. The render walker consumes the tree and turns each
node into a backend call.

**Contract view.** A primitive is a contract between the framework
and every backend. Adding a primitive variant means adding a
`Backend` trait method that every backend either implements or
defaults. The framework guarantees backends a stable set of
construction + update + lifecycle hooks per primitive.

**Composition view.** A primitive is what composes into other
primitives. Every `Primitive` can sit inside a `View { children:
Vec<Primitive> }`, can be returned from a `#[component]`, can be
the body of a `when`/`switch` arm. The set of primitives defines
the set of composable building blocks.

The framework deliberately keeps this set small. Each primitive is
expensive: every backend pays implementation cost, the trait gets
wider, the `Primitive` enum grows, the walker grows. So the bar
for adding one is high: a primitive earns its place only when it
**can't reasonably be composed** from existing ones — i.e., it
needs platform behavior that doesn't decompose into smaller
primitives.

## Primitive vs. component

This is the most important distinction in the framework.

| | **Primitive** | **Component** |
| --- | --- | --- |
| Defined in | `framework-core` | Your code |
| Backend impl required | Yes | No |
| Cross-platform implementation | One per backend | Shared (compiles for every target) |
| Set is | Small, stable, fixed | Unbounded |
| Lives in | `Primitive` enum variant | `#[component] fn` |
| Examples | `View`, `Button`, `TextInput`, `Virtualizer` | `Card`, `Modal`, `Tabs`, `LoginForm` |

A **component** is composed Rust — a function that returns a
`Primitive`, with reactivity and refs wired by `#[component]`. Components
are how you build a design system. You can have thousands.

A **primitive** is the platform-bound substrate components are built
on. The set the framework ships is intentionally narrow:

- **Layout / content**: `View`, `Text`, `ScrollView`
- **Controls**: `Button`, `TextInput`, `Toggle`, `Slider`
- **Media**: `Image`, `Video`, `WebView`
- **Feedback**: `ActivityIndicator`
- **Lists**: `Virtualizer` (used through the typed `flat_list<T>` wrapper)
- **GPU**: `Graphics`
- **Navigation**: `Navigator`
- **Structural conditionals**: `When`, `Switch`

That's it. Roughly fourteen. Everything else in any app you build —
including everything that has a "look" — is *your* component code.

This is the framework's leverage. The set of primitives is the
cross-platform contract. The set of components is your design.
The two don't fight.

---

## The shape of every primitive

Every primitive variant in the `Primitive` enum has the same
structural pieces:

```rust
pub enum Primitive {
    Button {
        label:    TextSource,                       // —┐ primitive-specific data
        on_click: Rc<dyn Fn()>,                     // —┤
        disabled: Option<Box<dyn Fn() -> bool>>,    // —┘ reactive prop

        style:    Option<StyleSource>,              // — universal: any primitive can be styled
        ref_fill: Option<RefFill>,                  // — universal: any primitive can be bound to a Ref
    },
    // …
}
```

Three slots are universal:

- **Primitive-specific data**: the props that define what this
  primitive *is*. Static for one-shot values (strings, sizes), boxed
  closures for reactive ones (`Fn() -> String`, `Fn() -> bool`,
  `Signal<T>`).
- **`style: Option<StyleSource>`**: an optional stylesheet
  application. Styling is *orthogonal to structure* — every visible
  primitive accepts a style without each primitive having to know
  about styling.
- **`ref_fill: Option<RefFill>`**: an optional ref binding.
  Set by `.bind(r)` on the `Bound<H>` wrapper; the walker uses it to
  fill a typed handle slot so the parent can drive the primitive
  imperatively.

Reactive props are **closures**, not direct values. A `Button` with a
reactive label carries `label: TextSource::Reactive(Box<dyn Fn() ->
String>)`. The walker wraps that closure in an `Effect` so signals
read inside subscribe naturally. The widget exists once and is
mutated in place via `Backend::update_button_label` — no diff, no
re-render. (See [`reactivity.md`](./reactivity.md) for the model.)

### `Bound<H>` — the builder façade

The author doesn't construct `Primitive` variants directly. Each
primitive has a constructor that returns `Bound<H>` — a thin
wrapper that exposes a fluent builder:

```rust
pub fn button<L, F>(label: L, on_click: F) -> Bound<ButtonHandle> { … }

button("Save", || save())
    .with_style(primary_button_style())
    .bind(save_button)
    .disabled(move || saving.get())
```

Each builder method mutates the inner `Primitive`'s optional slot
(`style`, `ref_fill`, `disabled`) and returns `Self`. The DSLs (`ui!`,
`jsx!`) emit `.with_style(...)`, `.bind(...)`, `.disabled(...)` on
the `Bound<H>` returned from the constructor.

### Handles and `Ops`

Each primitive that supports imperative actions ships a
**handle type** plus an **ops trait**:

```rust
pub struct TextInputHandle    { node: Rc<dyn Any>, ops: &'static dyn TextInputOps }
pub trait  TextInputOps       { fn focus(&self, node: &dyn Any); … }
pub struct ScrollViewHandle   { node: Rc<dyn Any>, ops: &'static dyn ScrollViewOps }
pub trait  ScrollViewOps      { fn scroll_to(&self, node: &dyn Any, x: f32, y: f32); … }
```

The handle holds a type-erased `Rc<dyn Any>` (the backend's node)
plus a static reference to the backend's `Ops` impl. When the user
calls `handle.focus()`, the handle invokes
`ops.focus(&*self.node)`; the ops impl downcasts the `dyn Any` back
to the concrete backend node type and runs the platform call.

This is the seam that keeps imperative APIs **platform-portable** but
**type-erased at the call site**:

- The author calls `handle.focus()` — one method, one type, works
  across every backend.
- Backends that don't implement an imperative API leave the trait
  default (a no-op handle backed by `Rc::new(())`); the call
  silently does nothing on that platform.

You'd reach into this layer if you were building a primitive that
needs imperative platform actions exposed to authors (think:
`scroll_to`, `play`, `focus`). For most components you compose,
you only **use** existing handles.

---

## The primitives, individually

This section is a guided tour: what each primitive is, why it
exists, what its contract is. Read in the order presented — they
build on each other conceptually.

### `View` — the structural container

```rust
pub fn view(children: Vec<Primitive>) -> Bound<ViewHandle>
```

The framework's default container. Holds an ordered list of children;
the backend decides what "container" means natively (a `<div>` on
web, a `LinearLayout` on Android, a `UIView` on iOS).

**Style controls layout.** The framework's flex-like style vocabulary
(`flex_direction`, `justify_content`, `align_items`, `padding`,
`margin`, `width`, `height`) is the universal way to control how a
View arranges its children. Default direction is `Column` (stack
top-to-bottom), matching React Native.

A `View` has no native behavior beyond "be a container." It exists
to be the thing components hang structure off of — every layout
component you write is going to compose `View`s.

### `Text` — the content leaf

```rust
pub fn text<T: IntoTextSource>(source: T) -> Bound<TextHandle>
```

A leaf of text content. `source` can be:

- A `String` / `&str` — static content.
- A closure `Fn() -> String` — reactive content. Signals read inside
  the closure subscribe naturally.

The walker wraps reactive sources in an `Effect` that calls
`Backend::update_text` on change. The native widget exists once,
its text is mutated in place.

Text wrapping, font, color, alignment are style concerns —
controlled via the optional `style` slot, not separate props.

### `Button` — interactive trigger

```rust
pub fn button<L, F>(label: L, on_click: F) -> Bound<ButtonHandle>
```

A pressable widget with a label and a callback. Label is a
`TextSource` (static or reactive); the callback is `Fn() + 'static`.

The button carries a **`disabled` slot** because being inert is
fundamentally different from being styled "looks disabled": the
backend marks the native widget as non-interactive (`disabled`
attr on web, `setEnabled(false)` on Android). Wired via
`.disabled(move || some_signal.get())` — the closure is reactive.
The disabled flag also flips the `DISABLED` style state bit so any
`state disabled { … }` overlay applies.

The reason `Button` is a primitive rather than "a View with a
click handler" is platform native behavior: accessibility
affordances, keyboard activation, focus rings, haptic feedback,
hit-target sizing. Each backend gets to use its native button
widget, which gives all of that for free. Building it from a View
+ event handler would mean re-implementing native behavior, badly.

### `TextInput` — controlled text input

```rust
pub fn text_input<F>(value: Signal<String>, on_change: F) -> Bound<TextInputHandle>
where F: Fn(String) + 'static
```

A controlled single-line text field. **Controlled** means: the
`Signal<String>` is the source of truth. The framework subscribes
to it and writes its value to the native widget; the native widget
fires `on_change` for every input event with the new text. The
canonical pattern:

```rust
let name = signal!(String::new());
ui! {
    TextInput(value = name, on_change = move |s| name.set(s))
}
```

Cyclic but stable — backends are required to no-op when set to
their current value, so the round-trip terminates.

Imperative handle: `focus()`, `blur()`, `select_all()`.

**Why controlled?** The rest of the framework's reactive shape
assumes a single source of truth per piece of state. Uncontrolled
inputs would create a parallel universe where the input's "real"
value diverges from the signal. Components can layer validation,
transformation, masking around the controlled signal without
fighting the primitive.

### `Toggle` — controlled boolean

```rust
pub fn toggle<F>(value: Signal<bool>, on_change: F) -> Bound<ToggleHandle>
where F: Fn(bool) + 'static
```

Same shape as `TextInput`, for boolean state. Native widget per
platform (`<input type="checkbox">` on web, `Switch` on Android,
`UISwitch` on iOS). Use it for "is the option on/off."

### `Slider` — controlled numeric

```rust
pub fn slider<F>(
    value: Signal<f32>,
    min: f32,
    max: f32,
    step: Option<f32>,
    on_change: F,
) -> Bound<SliderHandle>
```

Controlled numeric input with bounds and an optional step. Same
controlled-signal pattern. The framework snaps `on_change` values
to the nearest step before dispatching, so behavior is uniform
across platforms regardless of native step support.

The three controlled inputs (`TextInput`, `Toggle`, `Slider`) all
share the same reactive shape: parent owns the signal, the
primitive flows changes both ways. This is the framework's
opinionated input model.

### `ScrollView` — single-axis scroll container

```rust
pub fn scroll_view(children: Vec<Primitive>) -> Bound<ScrollViewHandle>
```

A scrolling container. Children scroll along the configured axis
(vertical by default; `.horizontal(true)` for left-right). Two-axis
scrolling isn't supported — pick one direction.

Imperative handle: `scroll_to(x, y)`, `scroll_to_top()`.

**Use `ScrollView` for finite content**, `Virtualizer`/`flat_list`
for unbounded or large content. ScrollView mounts every child up
front; on 10,000-item lists the cost will hurt.

### `Image` — raster image content

```rust
pub fn image<S: IntoImageSource>(src: S) -> Bound<ImageHandle>
```

Reactive image source. `src` can be a `String`/`&str` for a static
URL or a closure that reads a signal. The walker installs an
effect that calls `Backend::update_image_src` on source changes.

`alt`/`accessibilityLabel`/`contentDescription` is set through a
builder method.

### `Video`, `WebView`, `ActivityIndicator`

```rust
pub fn video<S: IntoVideoSrc>(src: S)       -> Bound<VideoHandle>
pub fn web_view<U: IntoWebViewUrl>(url: U)  -> Bound<WebViewHandle>
pub fn activity_indicator()                 -> Bound<ActivityIndicatorHandle>
```

Three "embed platform functionality" primitives:

- `Video` — backend uses native player (`<video>`, `AVPlayer`,
  `MediaPlayer`). Handle exposes `play`/`pause`/`seek`.
- `WebView` — embedded browser surface (`<iframe>`, `WKWebView`,
  `android.webkit.WebView`). Reactive `url`.
- `ActivityIndicator` — indeterminate loading spinner. Static
  size/color. No methods; passive widget.

Each exists because re-implementing native equivalents in
user-space would lose huge amounts of platform behavior (codec
support, autoplay policies, web security model, native spinner
animations). The framework's job here is "expose the native thing
in a way that participates in layout and styling."

### `Virtualizer` — windowed list (used via `flat_list<T>`)

```rust
// Type-erased primitive (rarely called directly):
pub fn virtualizer(item_count, item_key, item_size, render_item) -> Bound<VirtualizerHandle>

// Typed wrapper (what you'll actually use):
pub fn flat_list<T>(
    data: Signal<Vec<T>>,
    key: impl Fn(usize, &T) -> u64,
    item_size: FlatListItemSize<T>,
    render_item: impl Fn(usize, &T) -> Primitive,
) -> Bound<VirtualizerHandle>
```

A virtualized list — only the visible window plus an overscan
buffer is mounted at any time. Mount/release happens through
framework-managed per-item scopes (so signals/effects/refs inside
an item are freed when it leaves the window).

Three concepts to understand:

1. **Stable identity.** `key(idx, &item)` returns a `u64` per item.
   When the data changes, the framework diffs old keys vs new keys
   to decide what to preserve. Items whose key still exists keep
   their mounted subtree intact (signals retain their values, refs
   remain bound). Items whose key is gone get their scope dropped.
2. **Size strategy.** `Known(f)` — author provides exact sizes;
   layout is deterministic. `Measured(f)` — author provides an
   *estimate*, backend measures the actual rendered size after
   mount and updates layout. Use `Measured` when content size
   depends on layout/wrap (e.g. text whose width depends on its
   container).
3. **`render_item`** runs **once per mount** inside a fresh per-item
   `Scope`. Re-mount happens only when an item enters the window,
   not on every scroll tick.

This is one of the few primitives that *can't* be composed: every
backend has a fundamentally different way of doing recycling
(DOM removal + insertion on web; `UICollectionView.prepareForReuse`
on iOS; `RecyclerView.onBindViewHolder` + `DiffUtil` on Android).
Pretending otherwise would mean re-implementing native recycling
in framework Rust — pointless.

### `Graphics` — GPU surface

```rust
pub fn graphics(on_ready, on_resize, on_lost) -> Bound<GraphicsHandle>
```

A backend-provided drawable surface, exposed as a
`raw_window_handle`-compatible handle. The author owns the
rendering — the framework just stands up the surface, fires
lifecycle callbacks, and otherwise stays out.

Pair with `wgpu`, `softbuffer`, `glow`, `vello`, or anything else
that accepts `HasWindowHandle + HasDisplayHandle`. The framework
doesn't link any GPU crate.

Lifecycle:

```
mount → on_ready → (on_resize …)* → unmount
mount → on_ready → on_lost → on_ready → … → unmount      // Android backgrounding
```

`on_lost` is critical on Android — the SurfaceView destroys its
surface on backgrounding, then recreates it later. Author code
**must** drop any state derived from the previous surface on
`on_lost`, then expect a fresh `on_ready` when it returns.

Why this primitive exists: rendering custom 2D/3D content is the
one thing no composition of other primitives can express. The
framework gets out of the way and hands you a window handle.

### `Navigator` — screen-stack navigator

```rust
pub fn navigator(initial: &Route<()>) -> Bound<NavigatorHandle>

ui! {
    Navigator()
        .screen(HOME_ROUTE, |_| ui! { Home() })
        .screen(DETAIL_ROUTE, |params: DetailParams| ui! { Detail(id = params.id) })
        .initial(HOME_ROUTE, ())
        .bind(nav_ref)
}
```

A declared route table + a stack-based imperative API. The backend
owns the platform-native stack: `UINavigationController` on iOS,
`FragmentManager` on Android, an inline subtree on web. Browser
back/forward and `history` integration are handled by the web
backend.

`NavigatorHandle::{push, pop, replace, reset}` drive the stack;
the framework manages per-screen `Scope`s so navigation leaves
behind no leaked signals/effects.

The reason this is a primitive: each backend's navigation
machinery is so different that abstracting them at the user level
would require re-implementing huge chunks of platform behavior
(animations, gesture handling, back-stack persistence, deep
linking). Wrapping each native stack instead lets every platform
get its native feel.

### `When` / `Switch` — structural conditionals

```rust
pub fn when<C, T, O>(cond: C, then: T, otherwise: O) -> Primitive
pub fn switch<S: PartialEq, F: Fn() -> S, B: Fn(&S) -> Primitive>(scrutinee: F, branches: B) -> Primitive
```

The framework's two reactive conditional primitives. `when` is a
binary condition; `switch` keys on any `PartialEq + 'static` value
(typically an enum).

Both wrap their decision closure in an `Effect`. When a signal the
closure reads changes:

- `when`: rebuilds when the boolean flips.
- `switch`: rebuilds only when the new key fails equality against
  the previous. Unrelated signal reads in the scrutinee don't tear
  down the active subtree.

**State in a hidden branch is gone on toggle.** The old subtree's
`Scope` drops, freeing every signal/effect/ref inside it. This is
the framework's "dispose on hide" model. Components that need to
keep state across visibility should hoist it into a parent.

Most authors don't call these directly — the DSLs lower
`if cond.get() { … } else { … }` → `when`, and
`match value.get() { Variant => … }` → `switch`.

These are primitives because they shape the *render walk itself* —
they own a per-branch `Scope` and manage rebuild ordering. They
don't have a backend method; the walker handles them entirely
inside framework-core.

---

## Patterns for building components on top of primitives

Now that you've seen the vocabulary, here's how it actually plays
out when you build something.

### Compose: 95% of components

Most components are pure composition. No new primitive, no new
backend code — just a `#[component] fn` that arranges primitives:

```rust
#[component]
pub fn card(props: &CardProps, children: Vec<Primitive>) -> Primitive {
    ui! {
        View(style = card_outer_style()) {
            if let Some(title) = &props.title {
                Text(style = card_title_style()) { title.clone() }
            }
            View(style = card_body_style()) { children }
        }
    }
}
```

`Card` carries no platform-specific code, no backend impl. It
works wherever the primitives it composes work — meaning, every
platform. This is the path you'll take for the overwhelming
majority of UI you build.

### Wrap a primitive with stronger types

The framework's primitives are intentionally generic. Building a
typed wrapper gives you a clean API:

```rust
#[component]
pub fn icon(props: &IconProps) -> Primitive {
    ui! {
        Image(src = props.icon.url(), alt = props.icon.label())
            .with_style(icon_style().size(props.size))
    }
}
```

`Icon` constrains `Image`'s string-typed source to an `Icon` enum
that knows what URLs it owns. Same primitive underneath, much
narrower interface above.

### Treat a primitive as a slot

`Virtualizer` and `Navigator` expose **slot-shaped** APIs — they
take rendering closures (`render_item`, screen builders) rather
than static children. Components that build on them can layer
abstractions on top without re-implementing the recycling /
navigation core:

```rust
#[component]
pub fn user_list(users: Signal<Vec<User>>) -> Primitive {
    flat_list(
        users,
        |_, u| u.id,
        FlatListItemSize::Known(Rc::new(|_, _| 64.0)),
        |idx, user| ui! { UserRow(user = user.clone(), index = idx) },
    )
    .into_primitive()
}
```

Same `Virtualizer` underneath. `user_list` is just an opinionated
wrapper that fixes the row size, the key, and the row component.

### Build a control out of `Button` + state

Many "controls" (a checkbox-like toggle that looks custom, a
segmented control, a date picker entry point) are really just
styled `Button`s with reactive labels and click handlers. They
don't need their own primitive — they need a component that
composes the primitive smartly:

```rust
#[component]
pub fn segmented<T: Eq + Clone>(props: &SegmentedProps<T>) -> Primitive {
    let view: Vec<Primitive> = props.options.iter().enumerate().map(|(i, opt)| {
        let selected = props.value.get() == opt.value;
        let value = opt.value.clone();
        let on_pick = props.on_change.clone();
        ui! {
            Button(label = opt.label.clone(), on_click = move || on_pick(value.clone()))
                .with_style(segment_style().selected(selected).position(position_for(i, ...)))
        }
        .into_primitive()
    }).collect();
    ui! { View(style = segmented_container_style()) { view } }.into_primitive()
}
```

No new primitive. The "segmented control" experience is the
component's job; the buttons, click handling, layout — all
primitives.

### Build a control out of `Graphics`

Sometimes a "control" really needs to render custom visuals — a
spinner with a non-platform animation, a sparkline, a color picker
canvas, a chart. `Graphics` is where you go:

```rust
#[component]
pub fn sparkline(data: Signal<Vec<f32>>) -> Primitive {
    let state: Ref<RendererState> = Ref::new();
    ui! {
        Graphics(
            on_ready = move |evt| { /* set up GPU pipeline */ state.fill(rs); },
            on_resize = move |evt| { /* … */ },
            on_lost   = move ||     { /* drop GPU state */ },
        )
    }
}
```

The framework gives you a real platform surface; your code does
the drawing. The component is still pure-Rust; it works on every
platform that implements `Graphics`.

### When should you add a primitive?

You almost shouldn't. The right test is: **is the behavior something
that fundamentally has to come from the platform, and that no
composition of existing primitives can express?**

- A "card with a title and body" — compose `View`s and `Text`.
- A "video player with custom controls" — compose `Video` +
  `Button`s + `Slider`. Don't add a primitive.
- A "fancy custom-rendered chart" — compose `Graphics`. Don't add
  a primitive.
- A "platform-native segmented control" — *maybe* a primitive, if
  the design system actually requires native iOS/Android segmented
  behavior. But usually you'd build it as a component out of
  `Button`s and live with the small native-feel sacrifice.
- A "native date picker UI" — probably a primitive, because the
  platform date pickers are massive, opinionated, and not
  realistically expressible in primitives.

If you do conclude that you need one: the path is a new `Primitive`
enum variant, new `Backend` trait method(s) for `create_*` /
`update_*` / (maybe) `release_*`, and an impl in every backend
you care about. See [`backend.md`](./backend.md) for the trait
contract.

The framework's posture: keep the primitive set small enough that
every backend can plausibly implement all of it, and rich enough
that components don't have to fight to express what they want.
You should rarely need to expand it.

---

## A quick exercise

A useful test of whether the framework's vocabulary fits your
mental model: pick a UI you'd want to build and decompose it.

> **A login screen with email, password, "remember me" toggle, and a
> submit button. Errors appear below each field; the submit button
> disables while the request is in flight.**

- Outer layout: `View` (with vertical flex style).
- "Email" label + field: `Text` + `TextInput` (controlled, bound to
  a `Signal<String>`).
- Per-field error message (conditional): reactive `if email_error.get().is_some()`
  → `Text { … }` → lowers to `when(...)`.
- "Password" — same as email.
- "Remember me" toggle: `Toggle`.
- Submit button: `Button` with a reactive `.disabled(move ||
  submitting.get())`.
- Loading state (if needed): `ActivityIndicator` shown via
  `when(submitting.get(), …, …)`.

No primitive missing. The form's *behavior* (validation rules,
submit flow, error mapping) is your component code. The framework
gives you the structural pieces and gets out of the way.

This decomposition exercise is genuinely the most useful tool for
deciding what the framework owes you: if every UI you can imagine
maps onto the primitives + components without forcing a primitive,
the vocabulary is the right size. If something forces a new
primitive, the framework has a missing piece and we want to know
about it.

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

Concretely, a primitive is a **payload type plus a registered mount
handler**. The payload is a plain struct in
`crates/runtime/vocabulary/src/prims/` (`ViewPrim`, `TextPrim`,
`TogglePrim`, …); the handler lives in
`crates/runtime/vocabulary/src/handlers/` and is installed on a
backend's `runtime_scene::Registry` by
`runtime_vocabulary::handlers::register_builtins`. Three views on what
that means:

**Data view.** A `runtime_scene::Element` is an inert tree node. Its
`Item` variant carries a type-erased payload plus children — a
description of "a button with this label, this click handler, this
style," not the button itself. The scene never interprets the payload;
`realize` looks up the handler by the payload's `TypeId` and hands it
the mount context (`crates/runtime/scene/src/realize.rs`).

**Contract view.** A primitive is a contract between its handler and
every backend, expressed as the `runtime_vocabulary::caps` traits the
handler bounds on. `mount_toggle` needs `ToggleOps`; `mount_view` needs
`ViewOps + InputOps + StyleServices + SafeAreaOps + IntrospectionOps`.
Adding a primitive means adding those capability methods, which every
backend either implements or inherits as a documented default
(placeholder, or degradation to a container). See
[`backend.md`](./backend.md).

**Composition view.** A primitive is what composes into other
primitives. Every `Element` can sit in another item's children, be
returned from a `#[component]`, or be the body of a reactive `if` /
`match` arm. The set of primitives defines the set of composable
building blocks.

The framework deliberately keeps this set small. Each primitive is
expensive: every backend pays implementation cost, the capability
surface gets wider, another handler ships in every bundle. So the bar
for adding one is high: a primitive earns its place only when it
**can't reasonably be composed** from existing ones — i.e., it needs
platform behavior that doesn't decompose into smaller primitives.

**Third-party primitives use the identical contract.** A payload type
declared outside the framework, with a handler registered at the boot
seam, is indistinguishable to the scene from a first-party one — which
is why there is no separate "External" concept any more. See
[`external-export.md`](./external-export.md) and
[`migrating-to-runtime-v2.md` § External SDKs](./migrating-to-runtime-v2.md#external-sdks-the-third-party-primitive-layer).

## Element vs. component

This is the most important distinction in the framework.

| | **Primitive** | **Component** |
| --- | --- | --- |
| Defined in | `runtime-vocabulary` (or an SDK) | Your code |
| Backend capability impls required | Yes | No |
| Cross-platform implementation | One handler + per-backend caps | Shared (compiles for every target) |
| Set is | Small, stable, fixed | Unbounded |
| Lives in | a payload struct + a `Registry` handler | `#[component] fn` |
| Spelled in `ui!` | snake_case (`view`, `text_input`) | PascalCase (`Card`, `LoginForm`) |
| Examples | `view`, `button`, `text_input`, `flat_list` | `Card`, `Modal`, `Tabs`, `LoginForm` |

A **component** is composed Rust — a function that returns a
`Element`, with reactivity and refs wired by `#[component]`. Components
are how you build a design system. You can have thousands.

A **primitive** is the platform-bound substrate components are built
on. The set the framework ships is intentionally narrow — these are the
snake_case tags `ui!` / `jsx!` recognize
(`crates/runtime/macros/src/primitives.rs::canonical_primitive`):

- **Layout / content**: `view`, `text`, `scroll_view`
- **Controls**: `button`, `text_input`, `text_area`, `toggle`, `slider`
- **Media**: `image`, `icon`, `link`
- **Feedback**: `activity_indicator`
- **Lists**: `flat_list` (the typed face of the virtualizer)
- **GPU**: `graphics`
- **Layering**: `overlay`, `anchored_overlay`, `presence`
- **Structural conditional**: `when` (and `switch`, reached through
  reactive `match`)

That's it — under twenty tags, all installed by one
`register_builtins` call. Everything else in any app you build — including
everything that has a "look" — is *your* component code, or an SDK
shipping its own payload + handler (`video`, `WebView`, `maps`,
`canvas`, `markdown`, `table`, `codeblock`, `svg`).

**PascalCase is never a primitive.** A PascalCase tag in `ui!` always
routes to `#[component]` dispatch, which is what frees a component
library to define its own `Image` / `Link` / `Toggle` without the
same-named primitive shadowing it.

This is the framework's leverage. The set of primitives is the
cross-platform contract. The set of components is your design.
The two don't fight.

---

## The shape of every primitive

Every primitive payload has the same structural pieces
(`crates/runtime/vocabulary/src/prims/`):

```rust
pub struct ButtonPrim {
    pub label:         Value<String>,            // —┐ primitive-specific data
    pub on_press:      Action,                   // —┤
    pub leading_icon:  Option<IconData>,         // —┤
    pub trailing_icon: Option<IconData>,         // —┤
    pub disabled:      Option<Value<bool>>,      // —┘ reactive prop

    pub style:    Option<StyleProp>,             // — universal: any primitive can be styled
    pub a11y:     AccessibilityProps,            // — universal: accessibility metadata
    pub test_id:  Option<&'static str>,          // — universal: robot/test identity
    pub ref_fill: Option<Box<dyn FnOnce(ButtonHandle)>>,  // — universal: imperative handle
}
```

Four slots are universal:

- **Primitive-specific data**: the props that define what this
  primitive *is*, each wrapped in `Value<T>` (see below).
- **`style: Option<StyleProp>`**: an optional stylesheet application.
  Styling is *orthogonal to structure* — every visible primitive accepts
  a style without each primitive knowing about styling.
- **`a11y` + `test_id`**: accessibility props and the robot-registry
  identity, uniform across primitives.
- **`ref_fill`**: an optional handle sink. Set by `.bind(r)` on the
  builder; the handler calls it with the minted handle at mount so the
  parent can drive the primitive imperatively.

Reactive props are `Value<T>`, a two-arm enum:
`Value::Const(T)` for a statically known value and
`Value::Dyn(Box<dyn Fn() -> T>)` for a reactive one
(`crates/runtime/world/src/lib.rs`). A `Const` prop is applied once and
creates **no reactive machinery at all**; a `Dyn` prop gets a binding
effect whose body calls the matching capability method, so signals read
inside subscribe naturally. The widget exists once and is mutated in
place through e.g. `ButtonOps::update_button_label` — no diff, no
re-render. (See [`reactivity.md`](./reactivity.md) for the model.)

### The builders — the author-facing façade

The author doesn't construct payloads directly. Each primitive has a
constructor returning a small builder that exposes a fluent surface
(`crates/runtime/vocabulary/src/glue.rs`, `GlueView` / `GlueText` /
`GlueButton` / …):

```rust
pub fn button(label: impl TextContent, on_click: impl IntoAction) -> GlueButton { … }

button("Save", || save())
    .with_style(primary_button_style())
    .bind(save_button)
    .disabled(move || saving.get())
```

Each builder method fills one of the payload's optional slots and
returns `Self`. `ui!` / `jsx!` emit exactly these calls on the builder
the constructor returned, so the macro form and the fn-call form produce
identical payloads. Every builder also carries the universal setters —
`with_style`, `test_id`, `accessibility`, `a11y_label`, `a11y_role`,
`a11y_hidden`, `live_region`, … — generated once by a shared macro
(`glue_wrapper_common!`).

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
pub fn view(children: Vec<Element>) -> GlueView
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
pub fn text(content: impl TextContent) -> GlueText
```

A leaf of text content. `source` can be:

- A `String` / `&str` — static content.
- A closure `Fn() -> String` — reactive content. Signals read inside
  the closure subscribe naturally.

A `Value::Dyn` source gets a binding effect that calls
`TextOps::update_text` on change. The native widget exists once, its
text is mutated in place.

Text wrapping, font, color, alignment are style concerns —
controlled via the optional `style` slot, not separate props.

#### Styled runs — inline-styled ranges in one paragraph

```rust
pub fn styled_text(runs: Vec<TextRun>) -> StyledText
```

A text node whose content is a list of `TextRun`s, each optionally
carrying a `TextRunStyle` delta (font family/weight/size, foreground,
background). This is how mixed-style text that must wrap as ONE
paragraph is expressed — inline code chips in prose being the
canonical case:

```rust
styled_text(vec![
    TextRun::plain("the "),
    TextRun::styled("ui!", TextRunStyle {
        font_family: Some(FontFamily::System("ui-monospace, monospace".into())),
        background: Some(Tokenized::token("color-surface-alt", Color("#eee".into()))),
        ..Default::default()
    }),
    TextRun::plain(" macro"),
])
.with_style(paragraph_style)
```

The node's own style is the paragraph style; run deltas layer over
it. Each backend realizes the runs through its platform's own
attributed-text mechanism (`TextOps::create_styled_text`): nested
`<span>`s on web/SSR, `NSAttributedString` on iOS/macOS,
`SpannableString` on Android, cosmic-text rich spans on the GPU
renderer. Inline wrapping happens INSIDE the platform text engine —
the framework's layout tree has no inline formatting context, which
is exactly why runs live inside one text node instead of being
sibling nodes. Backends without a styled realization fall back to
the concatenated plain text (same words, no styling).

Run colors/sizes are `Tokenized`, so chips track theme swaps: web
emits them as `var(--token)` (CSS cascade), native backends
re-realize through the theme cohort. The run list is static —
reactive content stays on the plain `text(...)` closure path, and a
`TextRunStyle` deliberately has no padding/radius (per-range
box-decoration can't be expressed uniformly across attributed-text
engines).

### `Button` — interactive trigger

```rust
pub fn button(label: impl TextContent, on_click: impl IntoAction) -> GlueButton
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
pub fn text_input(
    value: impl IntoValue<String>,
    on_change: impl Fn(String) + 'static,
) -> GlueTextInput
```

A controlled single-line text field. **Controlled** means: the
`Signal<String>` is the source of truth. The framework subscribes
to it and writes its value to the native widget; the native widget
fires `on_change` for every input event with the new text. The
canonical pattern:

```rust
let name = signal(String::new());
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
pub fn toggle(
    value: impl IntoValue<bool>,
    on_change: impl Fn(bool) + 'static,
) -> GlueToggle
```

Same shape as `TextInput`, for boolean state. Native widget per
platform (`<input type="checkbox">` on web, `Switch` on Android,
`UISwitch` on iOS). Use it for "is the option on/off."

### `Slider` — controlled numeric

```rust
pub fn slider(
    value: impl IntoValue<f32>,
    on_change: impl Fn(f32) + 'static,
) -> GlueSlider
// bounds and step are builder methods: .range(min, max), .step(step)
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
pub fn scroll_view(children: Vec<Element>) -> GlueScrollView
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
pub fn image(src: impl IntoValue<String>) -> GlueImage
```

Reactive image source. `src` can be a `String`/`&str` for a static
URL or a closure that reads a signal. A reactive source gets a binding
effect that calls `ImageOps::update_image_src` on change.

`alt`/`accessibilityLabel`/`contentDescription` is set through a
builder method.

### `activity_indicator` — passive feedback

```rust
pub fn activity_indicator() -> GlueActivityIndicator
```

An indeterminate loading spinner. Size and color are style/builder
concerns; it exposes no methods.

### Video and WebView are SDKs, not primitives

`video` and `WebView` embed platform functionality — a native player
(`<video>`, `AVPlayer`, `MediaPlayer`) and an embedded browser surface
(`<iframe>`, `WKWebView`, `android.webkit.WebView`). Both ship as SDKs
with their own payload type and per-host handler rather than as
framework primitives, because neither needs anything from the framework
beyond the registry contract:

```rust
ui! { WebView(url = "https://example.com") }     // crates/sdk/client/webview
```

`web_view` was never a first-party primitive tag, and the macro no
longer special-cases it — the SDK ships `type WebView = WebViewProps`
plus its `BuildElement` impl, so the tag is ordinary component dispatch
(`crates/runtime/macros/src/primitives.rs`, the `web_view` note). The
same is true of `maps`, `canvas`, `markdown`, `table`, `codeblock`, and
`svg`. Per-SDK host coverage is tabulated in
[`migrating-to-runtime-v2.md` § External SDKs](./migrating-to-runtime-v2.md#external-sdks-the-third-party-primitive-layer).

### `Virtualizer` — windowed list (used via `flat_list<T>`)

```rust
// Type-erased primitive (rarely called directly):
pub fn virtualizer(item_count, item_key, item_size, render_item) -> VirtualizerBuilder

// Typed wrapper (what you'll actually use):
pub fn flat_list<T, K, S, R>(
    data: Signal<Vec<T>>,
    key: K,                       // Fn(usize, &T) -> ItemKey
    item_size: FlatListItemSize<T>,
    render_item: R,               // Fn(usize, &T) -> Element
) -> GlueFlatList
where T: Clone + PartialEq + 'static
```

A virtualized list — only the visible window plus an overscan
buffer is mounted at any time. Mount/release happens through framework-managed per-row scopes, so
signals and effects inside a row are freed when it leaves the window.

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
3. **`render_item`** runs **once per mount**, inside a fresh per-row
   ownership scope (`MountCx::realize_detached`). Re-mount happens only
   when a row enters the window, not on every scroll tick; dropping the
   row's `Realized` when it leaves is the whole teardown.

This is one of the few primitives that *can't* be composed: every
backend has a fundamentally different way of doing recycling
(DOM removal + insertion on web; `UICollectionView.prepareForReuse`
on iOS; `RecyclerView.onBindViewHolder` + `DiffUtil` on Android).
Pretending otherwise would mean re-implementing native recycling
in framework Rust — pointless.

### `Graphics` — GPU surface

```rust
pub fn graphics(on_ready: impl FnMut(OnReadyEvent) + 'static) -> GlueGraphics
// .on_resize(f) / .on_lost(f) are builder methods
```

A backend-provided render target, delivered as
`OnReadyEvent.target`. The author owns the rendering — the
framework just stands up the target, fires lifecycle callbacks,
and otherwise stays out. It doesn't link any GPU crate.

`GraphicsTarget` has two shapes, because not every toolkit can
hand out a per-widget window handle:

| Variant | Backends | How you use it |
| --- | --- | --- |
| `RawWindow(GraphicsSurface)` | web, iOS, macOS, Android | `event.into_surface()`, then `wgpu::Instance::create_surface(&surface)` — or `softbuffer` / `glow` / `vello`, anything taking `HasWindowHandle + HasDisplayHandle`. |
| `Gl(GlTarget)` | Linux (GTK4) | `event.gl()`, then adopt the lent GL context (`wgpu_hal::gles::Adapter::new_external`, `glow::Context::from_loader_function`). |

GTK4 forces the second shape: it removed GTK3's per-widget native
windows, so there is no per-widget `wl_surface`/XID to wrap, and
the toplevel's handle is both the wrong rect and already driven by
GTK's own renderer. `GtkGLArea` lends a context + framebuffer
instead. Making this an enum keeps that visible — an author who
only supports swapchains gets `None` from `into_surface()` and
degrades, rather than a fabricated handle that fails inside
`create_surface`.

Working with a `GlTarget`: `make_current()` (binds the context
*and* the target framebuffer), draw, `present()`. Two properties
must feed your base transform:

- `framebuffer()` — render into this FBO, not the default `0`.
  Re-read after every `make_current()`; it changes on resize.
- `origin()` — `BottomLeft` on GL. A top-left-origin renderer
  must flip vertically or it draws upside down. Reported as data
  on the target (like `scale`) so folding it in is a sign change
  in a matrix you already have, never a platform check.

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
framework gets out of the way and hands you the drawable.

### Navigation — `swap` / `stack` + the outlet

Navigation is two primitives, both registered by `register_builtins`
and therefore identical on every host
(`crates/runtime/vocabulary/src/builders/navigator.rs`,
`crates/runtime/vocabulary/src/handlers/navigator.rs`):

```rust
pub fn swap_navigator(initial: &Route<()>)  -> SwapNavigatorBuilder   // flat, co-equal screens
pub fn stack_navigator(initial: &Route<()>) -> StackNavigatorBuilder  // push / pop depth
pub fn navigator_outlet()                   -> …                       // where the screen renders
```

Each declares a route table, and the **author supplies the chrome** as
ordinary layout wrapped around `{nav.outlet}` — the analog of
react-router's `<Outlet/>`. A tab bar is a bar around the outlet; a
drawer is an idea-ui `Drawer` around the outlet. Apps normally reach
these through the SDK faces `SwapNavigator` / `StackNavigator`
(`crates/sdk/client/navigators/{swap,stack}`), which lower to the same
builders:

```rust
SwapNavigator::new(&home)
    .screen(home.clone(),     |_| Screen::new(/* … */))
    .screen(settings.clone(), |_| Screen::new(/* … */))
    .layout(|nav| ui! {
        view {
            { nav.outlet }
            TabBar(active = nav.active_route, on_select = nav.on_select) { /* … */ }
        }
    })
    .bind(nav.clone());
```

Three properties follow from the handler design:

- **Dispatch is handler-safe.** `on_select`, `pop`, and
  `NavHandle::dispatch` never mount a screen directly — they push a
  command onto a queue and bump a tick signal (both handle-routed, legal
  anywhere), and a driver effect drains the queue inside the flush where
  realize is legal. One navigation is therefore one logical update.
- **A screen is `(root node, Realized)`.** A persistent policy keeps the
  pair cached while the node is detached from the tree; returning
  re-inserts the *same* node, so the route builder does not re-run and
  row state survives. Dropping the `Realized` (evict, pop, replace,
  reset, teardown) is the whole screen teardown.
- **URL sync is a seam, not a per-app opt-in.** A URL-bearing host
  installs a `UrlSyncService` and both navigators register at mount;
  hosts without URLs install nothing and the hooks vanish
  (web: `crates/backend/web/src/newcore_url_sync.rs`).

The reason navigation is a primitive at all: retention policy, screen
scope lifetime, and the outlet's structural swap have to sit next to the
mount machinery. What is *not* in the primitive any more is the
platform-native stack — there is one backend-neutral handler, which is
what keeps behavior uniform across hosts.

### `when` / `switch` — structural conditionals

```rust
pub fn when(cond: impl Fn() -> bool, then: …, otherwise: …) -> Element
pub fn switch<S: PartialEq>(scrutinee: impl Fn() -> S, branches: impl Fn(&S) -> Element) -> Element
```

The framework's two reactive conditionals. `when` is a binary
condition; `switch` keys on any `PartialEq + 'static` value (typically
an enum). Both lower to the scene's **guarded** structural hole
(`runtime_scene::dyn_keyed`), so the key is what decides a rebuild
(`crates/runtime/vocabulary/src/glue.rs::when`/`switch`):

- `when`: rebuilds when the boolean flips. A predicate that reads extra
  signals does not rebuild when only those extras change.
- `switch`: rebuilds only when the new scrutinee fails `PartialEq`
  against the previous. Consequence: `touch()` on a scrutinee signal is
  inert — change the value to force a rebuild.

**State in a hidden branch is gone on toggle.** The outgoing subtree's
`Realized` drops, freeing every signal and effect inside it and running
their cleanups. This is the framework's "dispose on hide" model.
Components that need state to survive visibility hoist it into a parent.

Most authors don't call these directly — the DSLs lower
`if cond.get() { … } else { … }` → `when`, and
`match value.get() { Variant => … }` → `switch`.

These are structural rather than platform primitives: they emit no
capability calls of their own. A hole is realized either anchorless
(spliced into the real parent, when `Host::supports_splice`) or under a
`Host::create_anchor` node — the only host ops involved.

---

## Patterns for building components on top of primitives

Now that you've seen the vocabulary, here's how it actually plays
out when you build something.

### Compose: 95% of components

Most components are pure composition. No new primitive, no new
backend code — just a `#[component] fn` that arranges primitives:

```rust
#[component(children)]
pub fn Card(title: Option<String>, children: Vec<Element>) -> Element {
    ui! {
        view(style = card_outer_style()) {
            if let Some(title) = title.clone() {
                text(style = card_title_style()) { title }
            }
            view(style = card_body_style()) { children }
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
pub fn Avatar(source: AvatarSource, size: f32) -> Element {
    ui! {
        image(src = source.url(), alt = source.label(), style = avatar_style().size(size))
    }
}
```

`Avatar` constrains the `image` primitive's string-typed source to a
type that knows which URLs it owns. Same primitive underneath, much
narrower interface above. Note the component is PascalCase and the
primitive it wraps is snake_case — that separation is what lets a
component library name a component `Image` without shadowing the
primitive.

### Treat a primitive as a slot

`flat_list` and the navigators expose **slot-shaped** APIs — they take
rendering closures (`render`, screen builders) rather than static
children. Components that build on them can layer abstractions on top
without re-implementing the recycling / navigation core:

```rust
#[component]
pub fn UserList(#[prop(static)] users: Signal<Vec<User>>) -> Element {
    ui! {
        flat_list(
            data = users,
            key = |_, u: &User| u.id,
            size = FlatListItemSize::Known(Rc::new(|_, _| 64.0)),
            render = |idx, user: &User| ui! { UserRow(user = user.clone(), index = idx) },
        )
    }
}
```

Same virtualizer underneath. `UserList` is an opinionated wrapper that
fixes the row size, the key, and the row component.

### Build a control out of `Button` + state

Many "controls" (a checkbox-like toggle that looks custom, a
segmented control, a date picker entry point) are really just
styled `Button`s with reactive labels and click handlers. They
don't need their own primitive — they need a component that
composes the primitive smartly:

```rust
#[component]
pub fn Segmented(
    options: Vec<Segment>,
    #[prop(static)] value: Signal<SegmentId>,
    on_change: Rc<dyn Fn(SegmentId)>,
) -> Element {
    ui! {
        view(style = segmented_container_style()) {
            for (i, opt) in options.iter().enumerate() {
                button(
                    label = opt.label.clone(),
                    on_click = {
                        let (id, pick) = (opt.id, on_change.clone());
                        move || pick(id)
                    },
                    style = segment_style()
                        .selected(value.get() == opt.id)
                        .position(position_for(i, options.len())),
                )
            }
        }
    }
}
```

The `for` lives **inside** `ui!` — the macro splats the siblings flat and
sees the reactive reads in the loop body. Building a `Vec<Element>`
outside the macro and splatting it in would defeat that.

No new primitive. The "segmented control" experience is the
component's job; the buttons, click handling, layout — all
primitives.

### Build a control out of `Graphics`

Sometimes a "control" really needs to render custom visuals — a
spinner with a non-platform animation, a sparkline, a color picker
canvas, a chart. `Graphics` is where you go:

```rust
#[component]
pub fn Sparkline(#[prop(static)] data: Signal<Vec<f32>>) -> Element {
    let renderer = Rc::new(RefCell::new(None::<RendererState>));
    ui! {
        graphics(
            on_ready  = { let r = renderer.clone(); move |evt| { *r.borrow_mut() = Some(setup(evt)); } },
            on_resize = { let r = renderer.clone(); move |evt| { /* … */ } },
            on_lost   = { let r = renderer.clone(); move || { *r.borrow_mut() = None; } },
        )
    }
}
```

The renderer state is held in a plain `Rc<RefCell<_>>` captured by the
callbacks — GPU state isn't reactive, so it doesn't need a signal, and
this keeps the lifetime tied to the closures the component owns.

The framework gives you a real platform surface; your code does
the drawing. The component is still pure-Rust; it works on every
platform that implements `Graphics`.

### When should you add a primitive?

You almost shouldn't. The right test is: **is the behavior something
that fundamentally has to come from the platform, and that no
composition of existing primitives can express?**

- A "card with a title and body" — compose `view`s and `text`.
- A "video player with custom controls" — compose the video SDK's tag +
  `button`s + `slider`. Don't add a primitive.
- A "fancy custom-rendered chart" — compose `graphics`. Don't add a
  primitive.
- A "platform-native segmented control" — *maybe* a primitive, if the
  design system actually requires native iOS/Android segmented
  behavior. Usually you'd build it as a component out of `button`s and
  live with the small native-feel sacrifice.
- A "native date picker UI" — probably a primitive, because the
  platform date pickers are massive, opinionated, and not
  realistically expressible in primitives.

If you do conclude that you need one: the path is a payload struct, a
mount handler registered on the `Registry`, and — if it needs platform
work the existing capability traits can't express — new capability
methods with a documented default, implemented in every backend you care
about. Peripheral features should take the third-party route instead: a
payload plus a handler registered at the boot seam, shipped as its own
crate, with no framework change at all. See
[`backend.md`](./backend.md) for the capability contract and
[`external-export.md`](./external-export.md) for the registration seam.

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

- Outer layout: `view` (with vertical flex style).
- "Email" label + field: `text` + `text_input` (controlled, bound to a
  `Signal<String>`).
- Per-field error message (conditional): reactive
  `if email_error.get().is_some()` → `text { … }` → lowers to `when`.
- "Password" — same as email.
- "Remember me" toggle: `toggle`.
- Submit button: `button` with a reactive
  `disabled = move || submitting.get()`.
- Loading state (if needed): `activity_indicator` inside a reactive
  `if submitting.get() { … }`.

No primitive missing. The form's *behavior* (validation rules,
submit flow, error mapping) is your component code. The framework
gives you the structural pieces and gets out of the way.

This decomposition exercise is genuinely the most useful tool for
deciding what the framework owes you: if every UI you can imagine
maps onto the primitives + components without forcing a primitive,
the vocabulary is the right size. If something forces a new
primitive, the framework has a missing piece and we want to know
about it.

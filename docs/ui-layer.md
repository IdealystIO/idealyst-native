# The UI layer

The UI layer is everything the application author touches:
`#[component]` functions, the `ui!` / `jsx!` macros, the typed handle
system (`Ref<H>`), the `stylesheet!` macro. It produces a tree of
`Primitive` values — the framework's structural IR — which the render
walker hands to a `Backend`.

The big idea: **the surface DSL is a frontend, not a structural
commitment.** `ui!`, `jsx!`, and any third macro you might write all
emit the same primitive vocabulary. Components, refs, styles, and
reactivity work identically across them.

---

## The structural IR: `Primitive`

`runtime_core::Primitive` is an enum — one variant per "kind of thing
the renderer knows about." A small sample:

```rust
pub enum Primitive {
    View    { children: Vec<Primitive>, style: Option<StyleSource>, ref_fill: Option<RefFill> },
    Text    { source: TextSource, style: Option<StyleSource>, ref_fill: Option<RefFill> },
    Button  { label: TextSource, on_click: Rc<dyn Fn()>, style: …, ref_fill: …, disabled: … },
    ScrollView { children: Vec<Primitive>, horizontal: bool, style: …, ref_fill: … },
    Virtualizer { item_count, item_key, item_size, render_item, … },
    Graphics { on_ready, on_resize, on_lost, … },
    Navigator(Box<primitives::navigator::Navigator>),
    When    { cond: Box<dyn Fn() -> bool>, then: …, otherwise: …, style: … },
    Switch  { key: Box<dyn Fn() -> Box<dyn Any>>, eq: …, build: …, style: … },
    // …
}
```

Three patterns recur across variants:

- **`style: Option<StyleSource>`** — every visual primitive can carry a
  style. The framework applies it in a dedicated `Effect`, independent
  of content updates, so style changes and content changes don't
  invalidate each other.
- **`ref_fill: Option<RefFill>`** — if the call site used `.bind(r)`,
  the renderer fills the `Ref<H>` with a handle to the just-created
  node. Imperative APIs (`focus`, `scroll_to`, `play`, etc.) flow
  through these handles.
- **Closures for reactive props** — `label: TextSource::Reactive(Fn ->
  String)`, `src: Box<dyn Fn() -> String>`, `disabled: Option<Box<dyn
  Fn() -> bool>>`. The walker installs an `Effect` around each, so
  signals read inside the closure drive updates automatically.

There is **no virtual DOM, no diff pass**. A primitive is built once;
subsequent updates flow through `Effect`-driven mutation calls on the
already-existing native node. The only "rebuild" path is the
conditional primitives (`When`, `Switch`, plus virtualized list
items) — and even there, only the affected subtree is rebuilt, not
its siblings.

### Reactive conditionals

```rust
pub fn when<C, T, O>(cond: C, then: T, otherwise: O) -> Primitive
pub fn switch<S: PartialEq, F: Fn() -> S, B: Fn(&S) -> Primitive>(scrutinee: F, branches: B)
   -> Primitive
```

`when` is a two-way conditional, `switch` is a multi-way conditional
keyed on any `PartialEq + 'static` type.

Both wrap their decision closure in an `Effect`. When a signal the
closure reads changes:

- `when` rebuilds the branch if the condition flipped.
- `switch` rebuilds only when the new key fails equality against the
  previous key — so unrelated signal changes that don't affect the
  branch identity don't tear down its state.

The old subtree's `Scope` drops on rebuild, freeing every signal,
effect, and ref inside it. **State in a hidden branch is gone on toggle
— this is the "dispose on hide" model.**

The rebuild itself is deferred to a microtask. This matters: the
triggering event (click handler, scroll callback, etc.) is itself a
wasm-bindgen closure or JNI trampoline. Tearing down the old subtree
synchronously would drop other closures that the platform may still
have queued events for. Deferring lets the platform finish draining
those events before the closures vanish.

---

## Components

```rust
#[component]
pub fn counter(props: &CounterProps) -> Primitive {
    let count = signal!(0);
    ui! {
        Button(label = "Inc", on_click = move || count.update(|n| *n += 1))
        Text { format!("Count: {}", count.get()) }
    }
}
```

The `#[component]` attribute does three jobs:

1. **Reactivity rewrite** — walks the function body and rewrites
   expressions that contain `.get()` (signal reads) into reactive
   closures the underlying primitive constructors accept. The rewrite
   targets the props of built-in primitives (`Text`, `Button`,
   `Image`, …) where the constructor accepts an `IntoTextSource`-style
   wrapper that distinguishes static from reactive.
2. **Invocation macro generation** — emits a sibling `counter!`
   macro_rules! that handles named-argument passing, `default(...)`
   support, and the children block from `ui!` / `jsx!`. This is what
   lets `Counter(label = "Score")` work inside `ui!` — it expands to
   `counter!(label = "Score")` which expands to a regular function
   call.
3. **`methods!` block lifting** — if the body declares a `methods! {
   fn ping(&self) { … } }` block, the macro generates a typed handle
   struct (`CounterHandle` with a `ping()` method) plus the wiring so
   `Ref<CounterHandle>` and `.bind(ref)` work on this component the
   same way they work on primitives.

The author writes a function. The framework gets a Rust function (still
callable normally), an invocation macro (used by the DSLs), and
optionally a handle type. None of these depend on which DSL was used to
write the body.

### Why two return paths

Built-in primitive constructors return `Bound<H>` (the typed-handle
wrapper that supports `.with_style(...)`, `.bind(...)`, `.disabled(...)`).
A `#[component]` returns `Primitive` directly — components are leaf
units of composition; the DSL coerces both via `IntoPrimitive`. The
result is that user components participate in the same composition
slots (`children: Vec<Primitive>`) as the built-ins.

---

## Refs

`Ref<H>` is a copy-handle pointing at an arena slot. The slot is owned
by the active `Scope`, so refs free deterministically.

```rust
let input_ref: Ref<TextInputHandle> = Ref::new();
ui! {
    TextInput(value = name, on_change = move |s| name.set(s)).bind(input_ref)
    Button(label = "Focus", on_click = move || input_ref.with(|h| h.focus()))
}
```

`.bind(r)` on a `Bound<H>` is what lifts the ref into the `ref_fill`
slot on the underlying primitive. The render walker reads `ref_fill`
after construction and calls `Ref::fill(handle)` to populate the
slot — so the slot is `None` between `Ref::new()` and mount, and `Some`
after, matching `useRef`'s lifecycle in React.

Each primitive's handle type is built by a `make_*_handle` method on
the backend. Backends that don't implement a given imperative API
return a default no-op handle (the `Backend` trait's `make_*_handle`
defaults do this). So calling `handle.focus()` on a backend that hasn't
implemented `TextInputOps::focus` is a silent no-op rather than a
build error — useful when you're filling in a new backend
incrementally.

User components declared with `#[component] + methods! { ... }` get
the same machinery: the macro generates a handle struct, and
`#[component]`'s rewrite turns the function's return type into a
`Bindable<MyHandle>` that supports `.bind(ref)` exactly like
primitives.

### Mount-time scoping

Refs created inside a component body are registered to the active
`Scope` (the same one the component's signals and effects join). When
the component unmounts — its enclosing `when`/`switch` branch flips,
the parent is rebuilt, the `Owner` drops — the scope's `Drop` frees
the ref slot along with the signals. There's no manual cleanup.

The handle inside the slot is dropped at that same point; backends'
handle types are responsible for any platform-specific teardown they
need (most are zero-cost wrappers and don't need any).

---

## DSLs

The DSLs (`ui!`, `jsx!`) are parsers that emit calls into the
primitive constructors and per-component invocation macros. They do
**not** know about reactivity, the backend, or the rendering model.

```text
ui! { Counter(label = "Score", value = score) }

  ↓ parsed by runtime_macros::ui

counter! { label = "Score", value = score }      // generated invocation macro

  ↓ expanded by the macro generated by #[component]

counter(&CounterProps { label: "Score".into(), value: score })

  ↓ runs the (rewritten) fn body, returns a Primitive
```

### Reactive `if`

`if` inside `ui!` / `jsx!` is rewritten to `when(...)` **only when the
condition contains a `.get()` call**. A condition without `.get()`
parses as a normal `if` expression and the branch is chosen once at
construction.

This is the macro's only reactivity heuristic and it's intentionally
overt: the author writes `.get()` when they want reactivity, the
macro emits `when` accordingly.

### Reactive `match`

The DSLs lower `match scrutinee { ... }` to `switch(...)` when the
scrutinee contains `.get()`. The arms then become the branches and
the framework's switch primitive handles the rebuild-on-key-change
logic.

### Why this matters for extensibility

The contract a UI macro needs to satisfy is small:

1. Emit calls to `runtime_core::{text, button, view, when, switch}`
   for built-in primitives.
2. Emit calls to per-component `name!(...)` macros (generated by
   `#[component]`) for user components.
3. For reactive conditionals, wrap dependency closures with
   `runtime_core::when` / `switch`.
4. Coerce the final expression via `IntoPrimitive::into_primitive(...)`.

Anything that satisfies those four can serve as a front-end. The
shipped `jsx!` is the proof-of-concept: identical primitive output,
different surface grammar, fully interoperable in the same component.

---

## `Bound<H>` and the builder chain

Most primitive constructors don't return `Primitive` directly. They
return `Bound<H>` — a small wrapper holding the in-progress
`Primitive` and exposing a fluent builder:

```rust
pub fn button<L, F>(label: L, on_click: F) -> Bound<ButtonHandle> { … }

button("Click", || …)
    .with_style(primary_button_style())
    .bind(my_ref)
    .disabled(move || disabled.get())
```

Each builder method mutates the inner `Primitive`'s optional slot
(`style`, `ref_fill`, `disabled`) and returns `Self`. When the
chain ends inside `ui!` children, the `IntoPrimitive` impl unwraps
the `Bound<H>` back to a bare `Primitive`.

This is what makes `style = ...` work uniformly on every primitive:
the DSL emits `.with_style(expr)` on the constructed `Bound<H>`,
the builder method stuffs it into the `Primitive`'s `style` slot,
and the walker picks it up at build time.

---

## Stylesheets at the call site

A `stylesheet!` declaration produces a `Rc<StyleSheet>`-returning
function plus a typed variant builder:

```rust
stylesheet! {
    PrimaryButton<MyTheme> {
        base |theme| {
            background_color: theme.colors.accent,
            padding: 12.0,
            corner_radius: 8.0,
        }
        variants {
            size: Size {
                Small => |t| { font_size: 12.0 },
                #default Medium => |t| { font_size: 14.0 },
                Large => |t| { font_size: 18.0 },
            }
        }
    }
}

// Use at the call site:
ui! {
    Button(label = "Save", on_click = move || …)
        .with_style(PrimaryButton().size(Size::Large))
}
```

The variant builder returns a `StyleApplication` — the value the
framework resolves against the active theme into concrete `StyleRules`
before handing off to the backend. See [`styling.md`](./styling.md)
for the full story.

---

## Children, lists, optionals

`ChildList::append_to` is the trait the DSL uses to flatten anything
into the surrounding `Vec<Primitive>`:

- `Primitive` → push as-is.
- `Option<Primitive>` → push if `Some`.
- `Vec<Primitive>` → extend.
- `Bound<H>` → unwrap and push.
- Iterators in `for` blocks → push each.

This is why `if let Some(x) = … { Text { x } }` and `for item in items
{ Text { item.name.clone() } }` work seamlessly inside `ui!` without
the macro special-casing every shape. The shape work is in the trait
impls; the macro just calls `append_to`.

---

## Navigator

`Navigator` is the stack-based screen container. It's declared
up-front with a route table and exposes an imperative handle:

```rust
let nav: Ref<NavigatorHandle> = Ref::new();
ui! {
    Navigator()
        .screen(HOME_ROUTE, move |_| ui! { Home() })
        .screen(DETAIL_ROUTE, move |params: DetailParams| ui! { Detail(id = params.id) })
        .initial(HOME_ROUTE, ())
        .bind(nav)
}
```

Architecturally, `Navigator` is "a `Primitive` that holds a route
table plus the framework-side `NavigatorControl` that handles
dispatch." The backend creates the native stack container
(UINavigationController / FragmentManager / inline subtree on web),
installs its dispatcher closure on the control plane, and calls
back into the framework's per-screen mount/release callbacks when
the user navigates.

`NavigatorHandle::{push, pop, replace, reset}` dispatch
`NavCommand`s into the control plane; the backend's installed
dispatcher executes them. The backend is responsible for:

- Building/dismissing the native stack frame.
- Calling `mount_screen(name, params)` to get a screen subtree.
- Calling `release_screen(scope_id)` when a screen leaves the stack.
- Calling `depth_changed(new_depth)` so the framework's control
  plane stays in sync.

This is the same shape as the [`Virtualizer` callbacks](./backend.md#virtualizer)
— framework holds the data + scope ledger, backend holds the visible
state and calls back for mount/release.

---

## Where to put things

If you want to:

| Goal | Where it lives |
| --- | --- |
| Add a new built-in primitive | New `Primitive` variant + walker arm in `runtime_core::lib`, plus a `create_*` / `update_*` method on `Backend` |
| Add a new user-facing component | A `#[component] fn name(...) -> Primitive` in app code |
| Add imperative methods on a component | A `methods! { fn foo(&self) { … } }` block inside the `#[component]` |
| Make a prop reactive | Wrap with a closure containing `.get()`; the constructor accepts `IntoTextSource` etc. or `Box<dyn Fn() -> T>` |
| Add a new DSL | A new proc-macro that emits primitive / `name!` calls (see [`ui-layer.md` § DSLs](#dsls)) |
| Add a new style property | A field on `StyleRules` + the matching `stylesheet!` grammar + a backend branch in `apply_style` |
| Wire imperative platform features | A new method on the relevant `*Ops` trait + backend impl + handle method |

Each one is a localized change — none of the others has to know.

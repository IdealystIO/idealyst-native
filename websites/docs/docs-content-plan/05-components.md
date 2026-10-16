# Components

A component is a Rust function that returns an `Element`. The
`#[component]` attribute turns that function into a tag you can call
from `ui!` or `jsx!`, gives it a stable identity for hot reload, can
expose imperative methods, and rewrites reactive reads in its body. This
page covers all of that.

## The shape

```rust
use runtime_core::{component, signal, ui, Element};

#[component]
pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
    let count = signal(initial);

    ui! {
        view {
            text { format!("Count: {}", count.get()) }
            button(
                label = "Increment",
                on_click = move || count.update(|n| n + 1),
            )
        }
    }
}
```

Three rules cover the basic shape:

- Annotate the function with `#[component]`. Name it in PascalCase — the
  tag you write in `ui!` is the function name.
- Declare props as ordinary parameters. The macro generates a
  `CounterProps` struct from them, a `Default` impl from the
  `#[prop(default = …)]` values, a `pub type Counter = CounterProps`
  alias, and a `BuildElement` impl. That alias and impl are what `ui!`
  calls — there is no per-component macro. Data props are wrapped in
  `Reactive<T>` by default; `#[prop(static)]` keeps a prop as a plain
  value.
- Return `Element`. `ui!` already produces one, so the macro block is
  the whole tail.

### Calling a component

A component is called by its tag:

```rust
ui! {
    Counter(initial = 0)
    Counter()            // every prop is optional; omitted ones use the default
}
```

The tag lowers to a struct literal plus a `BuildElement::build` call:
`CounterProps { initial: (0).into(), ..defaults() }`. Because the call
site is a real struct literal, rust-analyzer gives completion, hover,
and go-to-definition on every prop, and the tag resolves across crates
by ordinary `use` paths.

Inside `ui!`, the known snake_case names (`view`, `text`, `button`, …)
are primitives. Every other tag is a component, so write component tags
in PascalCase.

### The explicit props form

When the props struct needs extra derives or a hand-written `Default`,
declare it yourself and take it as the one parameter:

```rust
#[derive(Default)]
pub struct BadgeProps {
    pub label: String,
    pub count: i32,
}

#[component(default(count = 3))]
pub fn Badge(props: &BadgeProps) -> Element {
    ui! { text { format!("{} ({})", props.label, props.count) } }
}
```

`#[component(default(field = expr, …))]` overrides the struct's
`Default` for the listed fields. Defaults are evaluated per call, not
once. Take `props: BadgeProps` by value when the component needs to move
something out of it, such as a `children: Vec<Element>` field.
`#[method]` fns (below) need the parameter form.

## Macros at a glance

The authoring macros, each doing one thing:

- `#[component]` — turns a function into a tag. Generates the props
  glue, rewrites reactive reads, and handles hot reload. Covered on this
  page.
- `#[method]` — marks a nested fn inside a `#[component]` body as an
  imperative method. Covered below.
- `effect!({ … })` — a reactive side effect owned by the surrounding
  component scope (`watch(…)` is the out-of-tree peer). Covered in
  Reactivity.
- `ui! { … }` — the primary UI DSL. Lowers to plain runtime-core calls.
  Covered below and on the UI DSL page.
- `jsx! { … }` — a JSX-flavored variant of `ui!` with identical output.
  Covered below.
- `stylesheet! { … }` — declares a themed stylesheet. Covered in Styles.

State is a plain function: `signal(value)` returns a `Signal<T>`. See
Reactivity.

## #[method] — imperative handles

Sometimes a parent needs to trigger an action on a child — reset a
counter, clear a form, scroll a list to the top. The reactive substrate
isn't the right tool for "do something now"; an imperative handle is.

Declare methods as nested fns marked `#[method]` inside the component
body:

```rust
use runtime_core::{component, signal, ui, Element};

#[component]
pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
    let value = signal(initial);

    #[method]
    /// Reset the count to zero.
    fn reset() {
        value.set(0);
    }

    #[method]
    fn bump_by(n: i32) {
        value.update(|v| v + n);
    }

    ui! {
        view {
            text { format!("Count: {}", value.get()) }
        }
    }
}
```

The macro generates a `CounterHandle` struct with `reset` and `bump_by`
methods, and adds a `bind_to: Option<Ref<CounterHandle>>` prop to
`CounterProps`. The component still returns `Element`.

The parent passes a `Ref` through that prop:

```rust
use runtime_core::Ref;

#[component]
pub fn ParentApp() -> Element {
    let counter: Ref<CounterHandle> = Ref::new();

    ui! {
        Counter(initial = 0, bind_to = counter)

        button(
            label = "Reset",
            on_click = move || { counter.get().map(|c| c.reset()); },
        )
    }
}
```

A few rules:

- No `pub` and no `&self`: the body captures the component's locals
  directly, and the handle carries the public surface.
- Methods return `()`. They are commands; reads stay signals.
- Call through `Ref::get()`, not `Ref::with()`: methods write signals,
  and `with` holds a borrow while the closure runs.
- The handle name is the function name plus `Handle`: `Counter` →
  `CounterHandle`.
- Methods are also registered for the robot and inspector, which can
  call them by name with JSON arguments.
- Methods need the parameter form of props (zero parameters is fine).
  The explicit `props: &FooProps` form and generic components are
  rejected with a compile error.

See Refs for the full handle / ref surface.

## ui! and jsx! — the same DSL, two surfaces

The framework ships two UI DSLs. They produce the same output. The
choice is purely stylistic.

### ui!

```rust
ui! {
    view(style = card_style()) {
        text { "Hello" }
        button(label = "Click me", on_click = move || println!("click"))

        if logged_in.get() {
            text { "Welcome back!" }
        } else {
            text { "Please log in." }
        }

        for item in items.iter() {
            text { item.name.clone() }
        }
    }
}
```

### jsx!

```rust
jsx! {
    <view style={card_style()}>
        <text>"Hello"</text>
        <button label="Click me" on_click={move || println!("click")} />

        if logged_in.get() {
            <text>"Welcome back!"</text>
        } else {
            <text>"Please log in."</text>
        }

        for item in items.iter() {
            <text>{item.name.clone()}</text>
        }
    </view>
}
```

Both produce identical output. You can mix them in the same file — the
choice is per-call-site.

The mechanical differences are minor:

- `ui!`: parens for props, braces for children. `style = expr`, `text {
  "hi" }`, `Card(variant = card::variant::Outline) { Counter() }`.
- `jsx!`: angle brackets. String attrs are bare (`label="x"`),
  expression attrs are braced (`value={signal}`). Closing tags must
  match (`<Foo>...</Foo>`); `</>` is not supported. Text content goes
  through a `<text>` wrapper.
- Both: primitives are snake_case (`view`, `<view>`), components are
  PascalCase.

Pick whichever reads better to you.

## What ui! actually emits

`ui!` is syntax sugar — the macro parses tokens and emits ordinary Rust
calls into runtime-core's primitive constructors. You can write the same
component without the macro, and the framework can't tell the
difference.

Here's the counter from above in three forms.

### With ui!

```rust
#[component]
pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
    let count = signal(initial);

    ui! {
        view {
            text { format!("Count: {}", count.get()) }
            button(
                label = "Increment",
                on_click = move || count.update(|n| n + 1),
            )
        }
    }
}
```

### With jsx!

```rust
#[component]
pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
    let count = signal(initial);

    jsx! {
        <view>
            <text>{format!("Count: {}", count.get())}</text>
            <button
                label="Increment"
                on_click={move || count.update(|n| n + 1)}
            />
        </view>
    }
}
```

### With no macro at all

```rust
use runtime_core::{button, component, signal, text, view, Element, IntoElement};

#[component]
pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
    let count = signal(initial);

    // Builders outside a macro do need `.into_element()`;
    // `ui!` / `jsx!` output is already an `Element`.
    view(vec![
        text(move || format!("Count: {}", count.get())).into_element(),
        button("Increment", move || count.update(|n| n + 1)).into_element(),
    ])
    .into_element()
}
```

All three build the same tree. The third form shows that primitives are
just functions — you can drop the DSL, write your tree out of plain
calls, and the result is the same. You might do this where the DSL gets
in the way (highly procedural generation), or just to see what the macro
produces when debugging.

### The pieces being emitted

Looking at the no-macro form, you can see what `ui!` does:

- `view { ... }` → `view(vec![...])`. The `view` constructor takes a
  `Vec<Element>`. The primitive constructors (`text`, `button`, etc.)
  return their own builder types, so each child is coerced to an
  `Element` before joining the vec.
- `text { format!("...", count.get()) }` → a reactive text, `text(move
  || format!(...))`. A literal `text { "hi" }` emits the static form,
  `text("hi")`.
- `button(label = "...", on_click = ...)` → `button(label, on_click)`.
  Both arguments go through the framework's coercion traits
  (`TextContent`, `IntoAction`).
- A component tag `Counter(initial = 0)` → `BuildElement::build(Counter
  { initial: (0).into(), ..defaults() })`.
- The macro coerces its whole output to `Element`, so a `ui!` block is
  the function's return value as is.

### Reactive if

```rust
ui! {
    if count.get() > 0 {
        text { "positive" }
    } else {
        text { "zero or negative" }
    }
}
```

…lowers to:

```rust
when(
    move || count.get() > 0,
    move || text("positive"),
    move || text("zero or negative"),
)
```

`when` is the reactive conditional. The macro uses it for any condition
that might read a signal — a method call, a function call, or a `.get()`
anywhere in it. Only a condition that provably reads no signal lowers to
a plain Rust `if`. To force a plain `if`, read with `.peek()` or compute
the condition in a `let` above the block.

### Reactive for

```rust
ui! {
    for todo in todos, key = todo.id {
        text { todo.title.clone() }
    }
}
```

The iterator's type picks the lowering:

- A `Signal<Vec<T>>` (or another signal of an iterable) → a keyed
  reactive list. The `key = …` clause is required; rows whose key
  survives an update keep their state.
- Any other `IntoIterator` (a `Vec`, a slice, a map) → built once, no
  key needed.
- A range whose bound reads a signal (`for i in 0..n.get()`) → a
  reactive list keyed by index.
- A static range with a single-node body (`for i in 0..n`) → one batched
  repeat.

## Bringing your own front-end

`ui!` and `jsx!` are sugar over the same set of runtime-core calls.
Nothing about the framework privileges either one: a third macro that
emits the same calls would slot in alongside them.

Building one means writing a `proc_macro` that emits the shapes shown in
the "What `ui!` actually emits" section above — `view(...)`,
`text(...)`, `button(...)`, `when(...)`, and `BuildElement::build(Foo {
… })` for component tags. The `ui!` and `jsx!` sources in
`crates/runtime/macros-expand/src/` are the working references.

## Recap: a finished component

Tying everything together:

```rust
use runtime_core::{component, signal, ui, Element};

#[component]
pub fn Counter(#[prop(static, default = 0)] initial: i32) -> Element {
    let value = signal(initial);

    #[method]
    fn reset() { value.set(0); }

    #[method]
    fn bump_by(n: i32) { value.update(|v| v + n); }

    ui! {
        view {
            text { format!("Count: {}", value.get()) }
            button(label = "++", on_click = move || value.update(|n| n + 1))
        }
    }
}
```

What's happening here:

- `#[component]` turns `Counter` into a tag and generates `CounterProps`
  with a default of `0` for `initial`.
- `signal(initial)` allocates a reactive state slot owned by the
  component's scope.
- `#[method]` declares `reset` and `bump_by`. The macro generates
  `CounterHandle` and adds the `bind_to` prop.
- `ui!` lowers to plain runtime-core calls, with the text bound
  reactively to `value`.

The parent calls this with:

```rust
let counter = Ref::<CounterHandle>::new();
ui! {
    Counter(bind_to = counter)
    button(label = "Reset", on_click = move || { counter.get().map(|c| c.reset()); })
}
```

— `Counter()` reads as a constructor, `bind_to` fills the parent's ref
when the component builds, and `counter.get().map(|c| c.reset())` later
fires the method.

## Where to read more

- [Reactivity](reactivity) — signals, effects, the substrate components
  run on.
- [The UI DSL](ui-dsl) — the full `ui!` grammar, including styles,
  control flow, and trailing method chains.
- [Refs](refs) — `Ref<H>` and the surface for built-in handles plus
  user-component handles via `#[method]`.
- [Dev tools](dev-tools) — hot reload and what `#[component]` does to
  make each function swappable at runtime.

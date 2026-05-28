# Components

A component is a Rust function that returns a `Element`. The
framework wraps that function in some compile-time machinery so it
can be called from a DSL, reuse a stable identity for hot reload,
expose imperative methods, and rewrite reactive call sites for
ergonomics. This page covers all of that.

## The shape

```rust
use runtime_core::{component, signal, ui, Element};

pub struct CounterProps {
    pub initial: i32,
}

#[component]
pub fn counter(props: &CounterProps) -> Element {
    let count = signal!(props.initial);

    ui! {
        View {
            Text { format!("Count: {}", count.get()) }
            Button(
                label = "Increment",
                on_click = move || count.update(|n| *n += 1),
            )
        }
    }
}
```

Three rules cover the basic shape:

1. **Annotate the function with `#[component]`.** This is what
   generates the per-component invocation macro, handles hot reload,
   and rewrites the body for reactivity.
2. **Take one parameter, by reference: `props: &MyProps`.** The
   props struct is a regular Rust struct you declare next to (or
   above) the function. Field names become prop names in the
   invocation macro.
3. **Return `Element`.** The framework wraps the returned value
   through `IntoElement::into_element(...)`, so you can return a
   bare `Element`, a `Bound<H>` from a primitive constructor, or
   anything else that implements `IntoElement`.

### Calling a component

The macro generates two ways to invoke a `#[component]` function:

```rust
// Inside a ui! / jsx! block (the normal case):
ui! {
    counter(initial = 0)
}

// Directly, as a plain Rust call:
let prim: Element = counter(&CounterProps { initial: 0 });
```

Inside a DSL, the call site reads like a constructor. Outside, it's
a function call with a struct-literal props argument. They produce
the same `Element`.

### Variants of the signature

The shape above is the common case. Three legitimate variants:

- **No props:** `pub fn header() -> Element`. The invocation macro
  accepts `Header()` with no arguments.
- **By value:** `pub fn list_view(props: MyProps) -> Element`.
  Used when the component needs to take ownership of something in
  `props` — typically a `Vec<Element>` of children it consumes.
  The macro detects this and emits the right ownership form.
- **Bindable return:** `pub fn counter(props: &Props) -> Bindable<CounterHandle>`.
  Used when the component exposes a `methods!` block (see below).
  The DSL coerces it back to a `Element` automatically.

### Defaults

If you want some props to default to a value when the caller omits
them, declare them on the attribute:

```rust
#[component(default(initial = 0, step = 1))]
pub fn counter(props: &CounterProps) -> Element {
    // ...
}
```

The invocation macro then accepts the call without those props
(`counter()` or `counter(initial = 5)`), filling in the defaults at
the call site. Defaults are evaluated per call, not once at
component definition, so an expression like
`default(now = std::time::Instant::now())` does what you'd expect.

## Sub-macros at a glance

Five macros come with the framework. Each does one thing:

| Macro | What it does | Where it's covered |
| --- | --- | --- |
| `#[component]` | Wraps a function as a component. Generates the invocation macro, handles reactivity rewriting and hot reload. | This page. |
| `signal!(value)` | Shorthand for `Signal::new(value)`. | [Reactivity](#). |
| `ui! { … }` | The primary UI DSL. Lowers to plain runtime-core calls. | Below + the [UI DSL](#) page. |
| `jsx! { … }` | A JSX-flavored variant of `ui!` with identical output. | Below. |
| `stylesheet! { … }` | Declares a themed stylesheet. | [Styles](#). |
| `methods! { … }` | (Inside a component body) declares imperative methods exposed through a handle. | Below. |

Everything else in the framework is a plain function or type — no
hidden macro magic.

## `methods!` — imperative handles

Sometimes a parent component needs to trigger an imperative action
on a child — focus an input, reset a counter, scroll a list to the
top. The reactive substrate isn't the right tool for "do something
now"; an imperative handle is.

You declare methods inside the component's body:

```rust
use runtime_core::{component, signal, ui, Bindable, Element};

#[derive(Default)]
pub struct CounterProps {
    pub initial: i32,
}

#[component]
pub fn counter(props: &CounterProps) -> Bindable<CounterHandle> {
    let value = signal!(props.initial);

    methods! {
        fn reset(&self) {
            value.set(0);
        }
        fn bump_by(&self, n: i32) {
            value.update(|v| *v += n);
        }
    }

    ui! {
        View {
            Text { format!("Count: {}", value.get()) }
        }
    }
}
```

The macro generates a `CounterHandle` struct with `reset` and
`bump_by` methods. The component now returns `Bindable<CounterHandle>`
instead of `Element`.

The parent captures the handle via a `Ref`:

```rust
use runtime_core::Ref;

#[component]
pub fn parent_app() -> Element {
    let handle: Ref<CounterHandle> = Ref::new();

    ui! {
        counter(initial = 0).bind(handle)

        Button(
            label = "Reset",
            on_click = move || { handle.with(|h| h.reset()); },
        )
    }
}
```

A few rules:

- A component may have **at most one** `methods!` block.
- Each method takes `&self` (cosmetic — captures come from the
  closure, not from struct fields) plus zero or more typed
  parameters.
- Method bodies return `()`. (Returning a value from a method is a
  v1 limitation.)
- The handle name is derived from the function name:
  `counter` → `CounterHandle`. The macro converts snake_case to
  PascalCase and appends `Handle`.

See [Refs](#) for the full handle / ref surface.

## `ui!` and `jsx!` — the same DSL, two surfaces

The framework ships two UI DSLs. They produce the same output. The
choice is purely stylistic.

### `ui!`

```rust
ui! {
    View(style = card_style()) {
        Text { "Hello" }
        Button(label = "Click me", on_click = move || println!("click"))

        if logged_in.get() {
            Text { "Welcome back!" }
        } else {
            Text { "Please log in." }
        }

        for item in items.iter() {
            Text { item.name.clone() }
        }
    }
}
```

### `jsx!`

```rust
jsx! {
    <View style={card_style()}>
        <Text>"Hello"</Text>
        <Button label="Click me" on_click={move || println!("click")} />

        if logged_in.get() {
            <Text>"Welcome back!"</Text>
        } else {
            <Text>"Please log in."</Text>
        }

        for item in items.iter() {
            <Text>{item.name.clone()}</Text>
        }
    </View>
}
```

Both produce identical output. You can mix them in the same file —
the choice is per-call-site.

The mechanical differences are minor:

- **`ui!`**: parens for props, braces for children. `style = expr`,
  `Text { "hi" }`, `Card(kind = Outlined) { Counter() }`.
- **`jsx!`**: angle brackets. String attrs are bare (`label="x"`),
  expression attrs are braced (`value={signal}`). Closing tags must
  match (`<Foo>...</Foo>`); `</>` is not supported. Text content
  goes through a `Text` wrapper.

Pick whichever reads better to you.

## What `ui!` actually emits

This is where it gets fun. `ui!` is **syntax sugar** — the macro
parses tokens and emits ordinary Rust calls into runtime-core's
primitive constructors. You can write the same component without
the macro, and the framework can't tell the difference.

Here's the counter from above in three forms.

### With `ui!`

```rust
#[component]
pub fn counter(props: &CounterProps) -> Element {
    let count = signal!(props.initial);

    ui! {
        View {
            Text { format!("Count: {}", count.get()) }
            Button(
                label = "Increment",
                on_click = move || count.update(|n| *n += 1),
            )
        }
    }
}
```

### With `jsx!`

```rust
#[component]
pub fn counter(props: &CounterProps) -> Element {
    let count = signal!(props.initial);

    jsx! {
        <View>
            <Text>{format!("Count: {}", count.get())}</Text>
            <Button
                label="Increment"
                on_click={move || count.update(|n| *n += 1)}
            />
        </View>
    }
}
```

### With no macro at all

```rust
use runtime_core::{button, component, signal, text, view, IntoElement, Element};

#[component]
pub fn counter(props: &CounterProps) -> Element {
    let count = signal!(props.initial);

    view(vec![
        text(move || format!("Count: {}", count.get())).into_element(),
        button("Increment", move || count.update(|n| *n += 1)).into_element(),
    ])
    .into_element()
}
```

All three compile to the same code. The third form makes the
"primitives are just functions" claim concrete — you can drop the
DSL entirely, write your tree out of plain function calls, and the
result is indistinguishable. You might do this in places where the
DSL gets in the way (highly procedural generation, programmatic
trees built from data), or just to read what the macro is producing
when debugging.

### The pieces being emitted

Looking at the no-macro form, you can see what `ui!` does:

- **`View { ... }`** → `view(vec![...])`. The `view` constructor
  takes a `Vec<Element>`. The primitive constructors
  (`text`, `button`, etc.) return `Bound<H>` handles, so each child
  is coerced to a `Element` via `.into_element()` before
  joining the vec.
- **`Text { format!("...", count.get()) }`** → because the
  expression contains `.get()`, the macro emits a *reactive* text:
  `text(move || format!(...))`. A static `Text { "hi" }` emits the
  non-reactive form: `text("hi")`.
- **`Button(label = "...", on_click = ...)`** → `button(label,
  on_click)`. Both arguments go through the framework's coercion
  traits (`IntoTextSource`, `IntoAction`).
- **The trailing coercion** — `.into_element()` on the outer
  `view(...)` returns a `Element`, which is what the function
  signature expects. The macro adds this coercion automatically
  when the function's return type is `Element`.

### Reactive `if`

```rust
ui! {
    if count.get() > 0 {
        Text { "positive" }
    } else {
        Text { "zero or negative" }
    }
}
```

…lowers to:

```rust
when(
    Derived { /* ...captures count, evaluates count.get() > 0... */ },
    || text("positive"),
    || text("zero or negative"),
)
```

`when` is the reactive conditional primitive. The macro picks it up
when the `if`'s condition contains `.get()`; a plain boolean
condition lowers to a regular Rust `if` (the branch is decided once
at construction).

### Reactive `for`

```rust
ui! {
    for item in items {
        Text { item.name.clone() }
    }
}
```

…lowers to a `Repeat` primitive or a regular `Vec<Element>`
build, depending on whether the iterator is signal-backed. The
macro takes care of the dispatch.

## Bringing your own front-end

`ui!` and `jsx!` are sugar over the same set of runtime-core
calls. Nothing about the framework privileges either one: a third
macro that emits the same calls would slot in alongside them.

Building one today means writing a `proc_macro` that emits the
shapes shown in the "What `ui!` actually emits" section above —
`view(...)`, `text(...)`, `button(...)`, `when(...)`,
per-component `name!(...)` invocations, and a final
`.into_element()` coercion. That's all that's required, and it's
all there is — but it does mean parsing tokens and emitting them
by hand.

Tooling to make this easier (so you can describe a DSL's shape
declaratively rather than write a proc-macro from scratch) is on
the roadmap. Until that lands, the existing `ui!` and `jsx!`
sources in [`crates/framework/macros/src/`](#) are the working
references.

## Recap: a finished component

Tying everything together:

```rust
use runtime_core::{component, signal, ui, Bindable, Element};

#[derive(Default)]
pub struct CounterProps {
    pub initial: i32,
}

#[component(default(initial = 0))]
pub fn counter(props: &CounterProps) -> Bindable<CounterHandle> {
    let value = signal!(props.initial);

    methods! {
        fn reset(&self) { value.set(0); }
        fn bump_by(&self, n: i32) { value.update(|v| *v += n); }
    }

    ui! {
        View {
            Text { format!("Count: {}", value.get()) }
            Button(label = "++", on_click = move || value.update(|n| *n += 1))
        }
    }
}
```

What's happening here:

- `#[component(default(initial = 0))]` registers the function as a
  component and declares a default for `initial`.
- `signal!` allocates a reactive state slot.
- `methods!` declares `reset` and `bump_by` as imperative
  operations. The macro generates `CounterHandle` and rewrites the
  return type from `Element` to `Bindable<CounterHandle>`.
- `ui!` lowers to plain runtime-core calls, with the reactive
  text being wrapped in an Effect and the trailing value coerced
  to `Element`.

The parent calls this with:

```rust
let handle = Ref::<CounterHandle>::new();
ui! {
    counter().bind(handle)
    Button(label = "Reset", on_click = move || { handle.with(|h| h.reset()); })
}
```

— and `counter()` reads as a constructor, `bind` attaches the handle
to the parent's ref, and `handle.with(|h| h.reset())` later fires
the method.

## Where to read more

- [Reactivity](#) — signals, effects, the substrate components run
  on.
- [The UI DSL](#) — the full `ui!` grammar, including styles,
  control flow, refs, and the trailing-method escape hatch.
- [Refs](#) — `Ref<H>` and the surface for built-in handles plus
  user-component handles via `methods!`.
- [Hot reload](#) — what `#[component]` does to make each function
  swappable at runtime.
- [Building your own DSL](#) — a worked example of a third
  front-end macro.

# Structured Reactivity for Generator Backends

Status: **Design proposal**, no code yet. Red-pen at will.

## Problem

The framework's reactive primitives encode behavior as `Box<dyn Fn() -> T>`
closures. iOS/Android/Web backends are *runtime services*: the host process
lives throughout the app's lifetime, and the backend can invoke closures
whenever a signal changes.

Roku is a *generator*: the Rust process runs once on a build host, emits a
wire stream into a `.pkg`, and dies. The device replays the wire stream
and then runs alone with no Rust anywhere. Closures can't be serialized,
and even if they could there's no Rust interpreter on Roku to evaluate
them.

The framework's existing primitive vocabulary doesn't accommodate this. To
bridge it, I added Roku-specific side channels (`WhenBinding`,
`ActionBinding`, optional `binding` fields, `*Decl` primitive variants,
`bind_X!` macros). That worked but it's parallel-paths workaround — it
broke the framework's "backend determines HOW the primitive is realized"
abstraction. The Roku-shaped escape hatches leaked into framework-core.

## The Core Insight

`Box<dyn Fn() -> T>` is opaque: it carries *behavior* (the callable) but
not *description* (what it computes from what). Runtime backends only
need the behavior. Generator backends only have access to the
description, via their wire format.

The fix: make the framework's reactive types carry **both** the behavior
and a structured description. Both kinds of backends use what they need;
neither is privileged. The framework's API stays single-sourced — no
Roku-specific overlay.

## Proposed Core Types

### `Derived<T>`

The canonical reactive expression. Replaces every `Box<dyn Fn() -> T>` in
reactive positions.

```rust
pub struct Derived<T> {
    /// Stable name of the pure transformation. Generator backends
    /// emit this symbol into their wire stream; runtime backends
    /// can use it for tooling / debug labels.
    pub method: &'static str,

    /// Arena ids of every signal `method` reads, in the order they
    /// appear in the method's parameter list.
    pub inputs: Vec<SignalId>,

    /// JSON snapshot of `inputs`' current values, captured at
    /// construction. Generator backends use this to seed signal
    /// state on the device.
    pub initial: Vec<serde_json::Value>,

    /// Runtime evaluator. Closes over the same signals + method.
    /// Runtime backends call this on signal change; generator
    /// backends never invoke it.
    pub compute: Rc<dyn Fn() -> T>,
}
```

Constructed by a macro that extracts the structure from call shape:

```rust
let label: Derived<String> = derive!(format_count(count, prefix));
```

`format_count` is a `#[method]`; `count` and `prefix` are `Signal`s. The
macro emits a struct with all four fields populated — backends pick what
they want.

### `Action`

The canonical event handler. Replaces every `Rc<dyn Fn(...)>` in
button-press / press-handler positions.

```rust
pub struct Action {
    /// Stable name of the transformation the event fires.
    pub method: &'static str,

    /// Signals the method reads.
    pub inputs: Vec<SignalId>,

    /// Optional signal the method's return value writes back to.
    /// `None` for fire-and-forget; `Some` for the common
    /// read-modify-write pattern (e.g. `count → increment(count) → count`).
    pub output: Option<SignalId>,

    /// Runtime evaluator.
    pub fire: Rc<dyn Fn()>,
}
```

```rust
let on_press: Action = action!(increment(count) => count);
```

### Primitive shapes after the refactor

Every primitive's reactive positions take `Derived<T>` or `Action`. No
optional binding fields, no `*Decl` variants:

```rust
enum Primitive {
    Text {
        content: TextContent,
        style: Option<StyleSource>,
    },
    Button {
        label: String,             // could later become TextContent
        on_click: Action,
        style: Option<StyleSource>,
    },
    When {
        cond: Derived<bool>,
        then: Box<Primitive>,
        else_: Box<Primitive>,
    },
    Switch {
        discriminant: Derived<SwitchPattern>,
        arms: Vec<(SwitchPattern, Primitive)>,
        default: Box<Primitive>,
    },
    Repeat {
        count: Derived<usize>,
        row_template: Box<Primitive>,
        // The macro mints a per-row index signal so the row
        // template can reference it like any other Signal<i32>.
        row_index_signal: SignalId,
    },
    Virtualizer {
        count: Derived<usize>,
        item_template: Box<Primitive>,
        item_size: ItemSize,
        overscan: f32,
        row_index_signal: SignalId,
    },
    // ...
}

enum TextContent {
    Static(String),
    Bound(Derived<String>),
}
```

### `Backend` trait

The trait stays small. Reactive arguments come in already structured:

```rust
trait Backend {
    fn create_text(&mut self, content: &TextContent) -> Self::Node;
    fn create_when(&mut self, cond: &Derived<bool>, /* ... */ ) -> Self::Node;
    fn create_repeat(&mut self, count: &Derived<usize>, /* ... */) -> Self::Node;
    // ...
}
```

- **Runtime backends** match on `TextContent`. `Static` sets the text
  once. `Bound(d)` installs an Effect that re-evaluates `d.compute()`
  whenever any of `d.inputs` changes.
- **Generator backends** match on the same `TextContent`. `Static` emits
  `CreateText { content }`. `Bound(d)` emits `CreateText { content: "" }`
  + a `BindText { node_id, inputs, method }` wire op.

Same primitive, two interpretations, zero side channels. No
`handles_*_natively` flags — every backend sees the structure and
chooses how to realize it.

## Author-facing macros

| Macro | Produces | Replaces |
|---|---|---|
| `derive!(method(sigs...))` | `Derived<T>` | `bind!(...)` |
| `action!(method(sigs...) => out)` | `Action` | `bind_press!(...)` |
| `when!(cond, then, else_)` | `Primitive::When` | `bind_when!(...)` |
| `switch!(disc, arms..., default)` | `Primitive::Switch` | `bind_switch!(...)` |
| `repeat!(count, row)` | `Primitive::Repeat` | `bind_repeat!(...)` |
| `virtualizer!(count, item, ...)` | `Primitive::Virtualizer` | (no equivalent today) |

The `bind_X!` family I added earlier becomes the canonical macros.
Existing closure-driven primitives + their constructors retire.

## What this looks like for the author

Before (today, mixed closure + macro):
```rust
let count: Signal<i32> = signal!(0);

ui! {
    Text { count.map(|n| format!("Count: {n}")) }
    When {
        cond: move || count.get() % 2 == 0,
        then: ui! { Text { "Even" } },
        else_: ui! { Text { "Odd" } },
    }
}
```

After (structured DSL):
```rust
#[method] fn label(n: i32) -> String { format!("Count: {n}") }
#[method] fn is_even(n: i32) -> bool { n % 2 == 0 }

let count: Signal<i32> = signal!(0);

ui! {
    Text { derive!(label(count)) }
    when!(derive!(is_even(count)),
        then = ui! { Text { "Even" } },
        else_ = ui! { Text { "Odd" } },
    )
}
```

Authors pay ~20% more boilerplate (extracting reactive expressions into
named `#[method]`s) and in exchange get cross-platform deployment
including Roku.

## Constraints the DSL imposes

1. **All reactive computation flows through `#[method]`-tagged functions.**
   Inline closures inside reactive positions are gone. This is the price
   of analyzability.
2. **`#[method]`s are pure transformations of their inputs.** No captures,
   no ambient state. Roku's transpiler enforces this; runtime backends
   treat it as a strong convention.
3. **The DSL is the entire reactive surface.** New reactive operators are
   added by extending the DSL, not by escaping into closures.

## Migration plan

Phased, each phase shippable on its own:

1. **Add `Derived<T>` and `Action` to framework-core.** Introduce
   `derive!` and `action!` macros. Don't remove anything yet.
2. **Refactor primitives one at a time.** `Text` and `Button` first
   (simplest). Then `When` (delete `WhenBinding`). Then `Repeat` /
   `Switch` (delete `*Decl` variants). Existing call sites get
   migrated as they're touched.
3. **Update each backend.** iOS/Android/Web: replace
   `closure.call()` with `derived.compute()` — mostly mechanical.
   Roku: replace `BindWhen`/`BindRepeat`/etc. wire ops' source of
   truth from `*Binding` side channels to the structured fields on
   the primitives.
4. **Delete the legacy surface.** `*Decl` variants, `*Binding`
   fields, `handles_*_natively` flags, `bind_X!` macros all come
   out. The DSL is the only path.
5. **Land Roku's Virtualizer→MarkupList lowering.** With
   `Primitive::Virtualizer` carrying structured metadata, the Roku
   backend can generate a cell component + ContentNode wiring with no
   parallel scaffolding.

## Alternatives considered

### A: Separate Roku-only primitive surface

Authors write entirely different code for Roku. Clean architecture,
zero portability. Rejected — defeats the cross-platform value of the
framework.

### B: Compile arbitrary Rust closures to BS

Build pipeline analyzes closure bodies and emits equivalent BS.
Authors write zero new code. Rejected — requires a real
Rust-subset-to-BS compiler, BS is too limited to host arbitrary Rust,
engineering cost is months not days.

### C: Keep the current parallel-paths shape, accept the side
channels

What we have today. Rejected — every new primitive needs the same
Roku-shaped scaffolding bolted on, and the framework's "backend
determines HOW" abstraction stays broken. We'd be paying this cost
forever.

### D: Structured DSL (this proposal)

Framework primitives carry both behavior and description. Backends
use what they need. Single API surface, no parallel paths.

## Open questions

1. **Composition of `Derived<T>`s.** Can the output of one `Derived<T>`
   feed into another's inputs? (Probably yes via a `chain!` macro, but
   the ergonomics need thought.)
2. **Tuple-returning methods.** Does `Derived<(A, B)>` make sense?
   How does the Roku transpiler handle tuples?
3. **Action chaining.** Should an `Action` be able to fire another
   `Action` synchronously? Roku-side this is a tree of signal-sets;
   it should probably be explicit, not chained closures.
4. **Author-extensible operators.** Can authors define custom
   `Derived`-shaped types? If so, how do generator backends know
   what to do with them?
5. **Closure escape hatch.** For authors targeting only runtime
   backends, should there be a `bind_unstructured!` that takes a raw
   closure and explicitly forfeits Roku compatibility? Or is the DSL
   strictly mandatory?
6. **State machine signals.** Some UI state (e.g. animations) is hard
   to express in a single `#[method]`. Does the DSL cover those, or
   do they get a different shape?

## Recommendation

Do this. It's a real refactor — probably a week's careful work,
especially phase 2-3 (migrating every primitive + every backend)
— but the alternative is permanent architectural drift.

Cleanups this enables:
- One coherent reactive model across every backend.
- A natural home for future structured hints (identity / lifecycle
  / size / mutability).
- Optimizable runtime backends (the structure permits skipping
  re-evaluation when no relevant inputs changed).
- Roku is no longer a special case; it's just a backend with
  no runtime, treated exactly like every other backend.

---

## What we have today (for context)

A summary of what's currently in framework-core that this refactor would
remove or change:

- `Primitive::When` has `binding: Option<WhenBinding>`. Removed; the
  whole primitive shape changes to take `Derived<bool>`.
- `Primitive::SwitchDecl` and `Primitive::RepeatDecl`. Removed; replaced
  by canonical `Switch` and `Repeat` that take `Derived<T>`.
- `Primitive::Button` has `on_click_binding: Option<ActionBinding>`.
  Removed; `on_click` becomes `Action`.
- `Backend::handles_when_natively` / `handles_switch_natively` /
  `handles_repeat_natively`. Removed; no backend distinguishes "native"
  vs "closure" paths anymore.
- `Backend::note_text_binding` / `note_when_binding` / `note_switch_binding`
  / `note_repeat_binding` / `note_button_action` / `note_signal_initial`.
  Removed; the structure is on the primitive itself, no separate
  notification needed.
- `Backend::begin_slot_capture` / `end_slot_capture` /
  `supports_lazy_slot_capture`. Removed; lazy materialization for
  generator backends becomes part of how `When` / `Switch` / `Repeat`
  are realized internally, not a separate hook.
- `bind!`, `bind_press!`, `bind_when!`, `bind_switch!`, `bind_repeat!`
  macros. Renamed to `derive!` / `action!` / `when!` / `switch!` /
  `repeat!` and elevated to canonical.
- `WhenBinding`, `ActionBinding`, `TextSource::Bound`. Either removed
  or merged into `Derived<T>` / `Action`.

If we go this direction, that's the scope of what changes.

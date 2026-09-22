# The UI layer

The UI layer is everything the application author touches:
`#[component]` functions, the `ui!` / `jsx!` macros, the typed handle
system (`Ref<H>`), the `stylesheet!` macro. It produces a tree of
`Element` values — the framework's structural IR — which
`runtime_scene::realize` mounts against a platform.

The big idea: **the surface DSL is a frontend, not a structural
commitment.** `ui!`, `jsx!`, and any third macro you might write all
emit the same primitive vocabulary. Components, refs, styles, and
reactivity work identically across them.

---

## The structural IR: `Element`

`runtime_scene::Element` is an enum with six variants, and it describes
**structure only** — it carries primitive payloads without interpreting
them (`crates/runtime/scene/src/element.rs`):

```rust
pub enum Element {
    Item { data: Box<dyn Any>, children: Vec<Element> },  // a primitive + children
    Fragment(Vec<Element>),                               // siblings with no node
    Dyn(DynSpec),                                         // a reactive hole
    Keyed { items: …, render: … },                        // a keyed reactive list
    Owned { element: Box<Element>, owned: Owned },        // a component boundary
    Many { data: Box<dyn Any> },                          // N siblings from one payload
}
```

Only `Item` and `Many` ever become real platform nodes. Everything a
primitive *is* lives in its payload struct
(`crates/runtime/vocabulary/src/prims/`), and `realize` dispatches each
payload to the handler registered for its `TypeId`. Three patterns recur
across payloads:

- **`style: Option<StyleProp>`** — every visual primitive can carry a
  style. The handler attaches it in a dedicated binding effect,
  independent of content updates, so style changes and content changes
  don't invalidate each other.
- **`ref_fill`** — if the call site used `.bind(r)`, the handler calls
  it with the handle it minted for the new node. Imperative APIs
  (`focus`, `scroll_to`, `play`) flow through those handles.
- **`Value<T>` for props** — `Value::Const(T)` is applied once and
  creates no reactive machinery; `Value::Dyn(Box<dyn Fn() -> T>)` gets a
  binding effect, so signals read inside the closure drive updates
  automatically.

There is **no virtual DOM, no diff pass**. A primitive is built once;
subsequent updates flow through binding effects into capability calls on
the already-existing native node. The only rebuild paths are the
structural holes (`Dyn`, `Keyed` — what reactive `if` / `match` / keyed
`for` and virtualized rows lower to), and even there only the affected
subtree is rebuilt, not its siblings.

### Reactive conditionals

```rust
pub fn when(cond: impl Fn() -> bool, then: …, otherwise: …) -> Element
pub fn switch<S: PartialEq>(scrutinee: impl Fn() -> S, branches: impl Fn(&S) -> Element) -> Element
```

`when` is a two-way conditional, `switch` is a multi-way conditional
keyed on any `PartialEq + 'static` type. Both lower to the scene's
**guarded** hole (`runtime_scene::dyn_keyed`), so the key decides:

- `when` rebuilds when the boolean flips; other signals the predicate
  reads don't tear the branch down.
- `switch` rebuilds only when the new scrutinee fails equality against
  the previous one. `touch()` on a scrutinee is inert — change the value.

The outgoing subtree's `Realized` drops on rebuild, running effect
cleanups and freeing every signal and effect inside it. **State in a
hidden branch is gone on toggle — this is the "dispose on hide" model.**

The rebuild runs inside the world's flush, not inside the event handler
that triggered it: the write stages, the driver effect for the hole
re-runs during the flush, and the swap happens there
(`crates/runtime/scene/src/realize.rs`). So the triggering platform
closure has already returned before the old subtree's closures are
dropped — the property the old core bought with a microtask deferral now
falls out of the flush boundary. See
[`automatic-batching.md`](./automatic-batching.md).

---

## Components

Props can be declared **inline** — as ordinary fn parameters — or as an
explicit `#[props]` struct. Inline is the preferred form:

```rust
#[component]
pub fn Badge(
    /// Text shown inside the badge (hover docs at the call site).
    label: String,
    #[prop(default = 3)] count: i32,
) -> Element {
    // `label`/`count` arrive wrapped `Reactive<String>`/`Reactive<i32>`
    // (same reactive-by-default rule as `#[props]` fields). A call site
    // can pass a literal, a `Signal`, or `rx!(…)`. In text, interpolate
    // them directly — an f-string slot is live or static by the value's
    // TYPE (see "Text f-strings" below).
    ui! {
        text { "{label} ({count})" }
    }
}
```

The macro generates `BadgeProps` from the parameter list — each data
param `T` wrapped to `Reactive<T>`, with `Signal`/handler/`Ref`/
`Vec<Element>` shapes left bare (the `#[props]` skip-list) — plus a
`Default` impl carrying the `#[prop(default = …)]` values. Per-arg
`#[prop(static)]` / `#[prop(reactive)]` override the wrap heuristic;
`#[prop(optional)]` / `#[prop(into)]` are accepted no-ops (every prop is
already optional, every value already coerced via `.into()`). A param
named `children: Vec<Element>` receives the call site's `{ … }` block.
Optional callbacks should use the `Option<Rc<dyn Fn()>>` shape (defaults
to `None`); a bare `Rc<dyn Fn()>` param needs an explicit
`#[prop(default = Rc::new(|| {}) as Rc<dyn Fn()>)]` since it has no
`Default`.

The explicit-struct form — one `props: &CounterProps` / `props:
CounterProps` parameter referencing a hand-written `#[props]` struct —
remains for components whose props need extra derives
(`IdealystSchema`, doc-controls) or a hand-rolled `Default`:

```rust
#[component]
pub fn counter(props: &CounterProps) -> Element {
    let count = signal(0);
    ui! {
        Button(label = "Inc", on_click = move || count.update(|n| *n += 1))
        Text { format!("Count: {}", count.get()) }
    }
}
```

Both forms produce the identical dispatch contract; `ui!` cannot tell
them apart. The `#[component]` attribute does three jobs:

1. **Reactivity rewrite** — walks the function body and rewrites
   expressions that contain `.get()` (signal reads) into reactive
   closures the underlying primitive constructors accept. The rewrite
   targets the props of built-in primitives (`Text`, `Button`,
   `Image`, …) where the constructor accepts an `IntoTextSource`-style
   wrapper that distinguishes static from reactive.
2. **Dispatch-glue generation** — emits a `pub type Counter =
   CounterProps` tag alias plus an `impl runtime_core::BuildElement for
   CounterProps` (whose `build` calls the function and whose `defaults`
   carries any `default(...)` values). This is what lets `Counter(label =
   "Score")` work inside `ui!`: it lowers to a plain struct literal,
   `BuildElement::build(Counter { label: ("Score").into(),
   ..<Counter as BuildElement>::defaults() })`. No per-component
   `macro_rules!` — dispatch resolves by ordinary paths (cross-crate
   without `#[macro_export]`/`#[macro_use]`), and the call site is a real
   struct literal so rust-analyzer gives field completion + go-to-def.
   (A component's props must therefore be `Default`; omitted props take
   their default. `Ref`/`Signal` have non-allocating sentinel `Default`s
   so required handle props are supplied at the call site and overwrite
   them. For inline props the macro generates the struct AND its
   `Default` impl, folding the `#[prop(default = …)]` values in.)
3. **`#[method]` fn lifting** — nested fns marked `#[method]` (no
   `pub`, no `&self`, `()` returns only — commands, not queries) become
   a typed handle struct (`CounterHandle` with a `ping()` method). The
   macro auto-injects a `bind_to: Option<Ref<CounterHandle>>` prop and
   fills it in-body, so the ordinary tag form binds:
   `ui! { Counter(bind_to = h) }`, then `h.get().map(|c| c.ping())`
   (`.get()`, not `.with()` — methods write signals). `#[method]`
   requires this inline-props shape; the legacy explicit-props /
   generic form is a compile error
   (`crates/runtime/vocabulary/src/robot_methods.rs`).

The author writes a function. The framework gets a Rust function (still
callable normally), the `BuildElement` dispatch glue (used by the DSLs),
and optionally a handle type. None of these depend on which DSL was used
to write the body.

### Why two return paths

Built-in primitive constructors return a builder (`GlueView`,
`GlueButton`, … — the wrappers that support `.with_style(...)`,
`.bind(...)`, `.disabled(...)`). A `#[component]` returns `Element`
directly — components are leaf units of composition; the DSL coerces
both via `IntoElement`. The result is that user components participate in
the same composition slots (`children: Vec<Element>`) as the built-ins.

---

## Refs

`Ref<H>` is a copy-handle pointing at a slot in the shared substrate's
arena (`crates/runtime/shared/src/reactive.rs`).

```rust
let input_ref: Ref<TextInputHandle> = Ref::new();
ui! {
    text_input(value = name, on_change = move |s| name.set(s)).bind(input_ref)
    button(label = "Focus", on_click = move || input_ref.with(|h| h.focus()))
}
```

`.bind(r)` installs a `ref_fill` closure on the primitive's payload; the
primitive's mount handler calls it with the handle it minted, so the
slot is `None` between `Ref::new()` and mount and `Some` after —
matching `useRef`'s lifecycle in React.

Each primitive's handle type is built by a `make_*_handle` capability
method. Backends that don't implement a given imperative API inherit the
default no-op handle (`runtime_vocabulary::caps::noop`), so calling
`handle.focus()` on a backend without `TextInputOps::focus` is a silent
no-op rather than a build error — useful when filling in a new backend
incrementally.

**Lifetime caveat.** The ref slot's lifetime was tied to an old-core
`Scope`, and no such scope is active in a runtime-v2 build, so a
`Ref::new()` slot is not freed until the thread exits. Refs are
per-component, so this is bounded — but a `Ref` created inside a
frequently-remounted subtree accumulates slots. See
[`reactivity.md` § `Ref<H>`](./reactivity.md#refh--the-imperative-handle-slot).

User components declared with `#[component]` + `#[method]` fns get a
parallel mechanism: the macro generates a handle struct and a
`bind_to` prop the body fills, driven through `Ref<MyHandle>` exactly
like a primitive's.

### Mount-time scoping

Signals and effects created inside a component body are collected into
the component's ownership scope, and the scope rides on the
`Element::Owned` boundary the `#[component]` macro emits. When the
component unmounts — its enclosing reactive `if` / `match` flips, the
parent's hole rebuilds, the root `Realized` drops — dropping that scope
runs the effects' cleanups and frees the slots. There's no manual
cleanup.

The `Ref<H>` *slot* is the exception, because it lives in the shared
substrate's arena rather than the world (see the caveat above); the
handle it holds is dropped when the ref is overwritten or the thread
ends. Backends' handle types are responsible for any platform-specific
teardown they need (most are zero-cost wrappers and need none).

---

## DSLs

The DSLs (`ui!`, `jsx!`) are parsers that emit calls into the
primitive constructors and, for user components, a `BuildElement`
struct-literal dispatch. They do **not** know about reactivity, the
backend, or the rendering model.

```text
ui! { Counter(label = "Score", value = score) }

  ↓ parsed by runtime_macros::ui, then split (see below)

let __ui_s0; __ui_s0 = score;                    // the one dynamic slot
BuildElement::build(Counter {                    // `Counter` is the tag alias
    label: ("Score").into(),                     // a literal: descriptor data
    value: (__ui_s0).into(),
    ..<Counter as BuildElement>::defaults()      // defaults for omitted props
})

  ↓ the `#[component]`-generated `build` calls the fn

counter(&CounterProps { label: "Score".into(), value: score, .. })

  ↓ runs the (rewritten) fn body, returns an Element
```

### The split: static descriptor + ordered slots

Before emitting anything, the macro splits each `ui!` site into a
**static** part and an ordered list of **dynamic slots**
(`crates/runtime/macros/src/ui_split.rs`). Static means: primitive and
component tags, attribute names, child order, string / integer / float /
bool literals (including `"lit".to_string()` / `"lit".into()`),
style-token accessors of the shape `t.a.b()` / `theme.x.y()`, enum-like
paths (`tone::Danger`, `StackAxis::Row`), and an f-string's literal
fragments. Everything else is a slot: closures, bare identifiers, method
calls, `rx!`, signal reads, control-flow conditions / scrutinees /
iterables, and bare expression children.

Slots have a **placement**:

- **`Prelude`** — bound to a `__ui_sN` local at the head of its scope,
  in source order, before anything is constructed. Emitted as a
  *deferred-init* pair (`let __ui_s0; __ui_s0 = …;`) so a `.into()` whose
  target type is pinned by the destination field still resolves.
- **`Construct`** — left where the construction splices it. Reserved for
  expressions whose *evaluation* has no observable effect: closure
  literals (constructing one only captures), macro invocations, the
  reactive-call shape `f(sig)` (rewritten inside a closure), and
  control-flow conditions / scrutinees / iterables (a reactive one is
  wrapped in `move || …`; a static `match` scrutinee must stay put or
  hoisting would force a move where match ergonomics borrow).

The hoist is why prop expressions now evaluate **in source order**. They
used not to: `style` lowers to a trailing `.with_style(f())`, so
`view(style = f()) { Badge(label = g()) }` ran `g()` before `f()`.

A **template scope** is one Rust scope's worth of nodes — the `ui!` body,
plus every body the emission puts in a fresh Rust scope (an `if`/`match`
branch, a `for` row builder, a `presence` child thunk). Each has its own
slot list, so a branch's expressions are evaluated when that branch
activates and a row's once per row, exactly as before. A `view`'s or
component's children are *not* a new scope: they are built inline in the
parent's block and share its prelude.

A primitive silently ignores props it doesn't recognise
(`view(gap = 4)` reaches nothing). Such a prop's slot is dropped from the
prelude rather than evaluated, so the split never starts running an
expression the emitter discards.

### Two lowerings

The split exists so a second lowering can consume it. `ui!` emits the
**direct** lowering (builder calls inline, reading slots out of their
locals). The **template** lowering turns the same split into a `static`
descriptor plus a runtime slot array:

```rust
{
    let __ui_s0; __ui_s0 = sheet();          // the SHARED prelude
    static __UI_DESC: Descriptor = …;        // the STATIC half: data
    runtime_core::__template::build(&__UI_DESC, &mut [
        SlotValue::style(__ui_s0),           // the DYNAMIC half
    ])
}
```

Both are reachable from the test-only `ui_lowered!(direct { … })` /
`ui_lowered!(template { … })` entry point in `runtime-macros`; `ui!`
stays on `direct`.

- `runtime-template` (`crates/runtime/template`) owns the descriptor
  types: `Descriptor`, `Node`, `PropEntry`, `SlotSig`, `SiteId`,
  `Registry`, `Patch`/`validate`, and the `TemplateSource` seam. It
  depends on `runtime-scene` and `serde` and nothing else, so a
  descriptor is a portable, serializable, validatable artifact.
- `runtime_vocabulary::template` owns the builder — it needs the glue
  wrappers, the coercion traits and `BuildElement`, which is why it
  cannot live in the data crate.

A node the descriptor cannot model becomes a `Node::Escape`: the direct
emitter builds it and the finished `Element` lands in a slot. Every
`ui!` construct is expressible that way, so the template lowering is
complete from day one and the descriptor-native set
(`view`/`text`/`button`/`image`/`activity_indicator`/`scroll_view`,
literal-prop components, reactive `if`) grows without ever being a
correctness precondition. An escaped node's *bodies* are still nested
templates.

A component node carries its literal props as descriptor data and
applies them through `__apply_literal(&mut Props, name, &LiteralValue)
-> bool`, which `#[component]` / `#[props]` generate as an **inherent**
method. Inherent-before-trait resolution is what makes a props type
*without* the macro fall back to a blanket `false` rather than fail to
compile — such a component is slot-only under the template lowering,
never an error.

### Selecting the lowering

There is deliberately **no cargo feature** for this. A proc-macro crate
is compiled once per build graph, so a feature on `runtime-macros` would
flip the lowering for every crate in the same cargo invocation — the
hazard that removed the `new-core` feature (see the NOTE in
`crates/runtime/macros/Cargo.toml`).

Two switches instead, at two granularities:

- **Per invocation** — `ui_lowered!(direct { … })` /
  `ui_lowered!(template { … })`. Test-only; it is how the parity suite
  expands one fixture both ways in one binary.
- **Per build graph** — `IDEALYST_UI_LOWERING=direct|template`, read by
  `runtime-macros` at expansion. `direct` is the default; an
  unrecognised value is a `compile_error!` naming the valid ones, never
  a silent fallback (building the wrong lowering would be invisible,
  since both produce working programs).

Set it through the CLI, not by hand:

```
idealyst dev   --web --ui-lowering template
idealyst build --web --ui-lowering template
```

**Why the flag and not the variable.** Cargo does not fingerprint a
proc macro's environment reads. Setting `IDEALYST_UI_LOWERING` alone
leaves every already-compiled crate looking fresh, so a rebuild happily
serves expansions from the *other* lowering — a mixed binary, with no
error. The CLI folds the value into `config_key`
(`crates/tools/build/web/src/lib.rs`), giving each lowering its own
target directory, which turns the flip into a directory switch and makes
a stale expansion impossible. That is a different reason from every
other field in that key: the others ride in `CARGO_ENCODED_RUSTFLAGS` or
the feature set, where keying the directory is a performance fix. Here
it is a correctness one.

The two must produce identical scenes. `crates/dev/ui-lowering-parity`
is the gate: every fixture is authored once, expanded through both
entry points in the same binary, mounted against `host-mock` in both
structural modes, and compared on three projections (structural ops,
every capability call, the final scene). It additionally pins BOTH
lowerings against goldens frozen *before* the slot rewrite, so "the
hoist changed nothing else" is falsifiable rather than self-asserted.

Why a descriptor is worth the machinery: most of a UI tree is not code.
Separating the data half makes a static edit (a changed literal, a
reordered child) a data change — the basis for hot reload without
recompiling, and, if it is ever built, for over-the-air UI patches.
Neither of those is built here; `TemplateSource`'s only implementation
is `CompiledIn`.

### Reactive `if`

`if` inside `ui!` / `jsx!` is reactive (rewritten to `when(...)`) in
**two** cases, and static (a plain Rust `if`, branch chosen once at
construction) otherwise:

1. **Visible signal read** — the condition tokens contain a `.get()`,
   e.g. `if items.get().len() > 1`. The macro sees the read syntactically
   and wraps the condition in a `when` closure. (Needed because such a
   condition's *type* is a plain `bool`, so the type-driven path below
   couldn't tell it apart from a static `if 3 < 4`.)
2. **Reactive-typed condition** — a bare `Signal<bool>` (what `memo(…)`
   returns) or `Derived<bool>`, e.g.
   `let visible = memo(move || items.get().len() > 1); … if visible { … }`.
   This is **type-driven**, mirroring the reactive `for` loop: the macro
   emits `(COND).__idealyst_if(then, else)` with `StaticCond` (for `bool`)
   and `ReactiveCond` (for `Signal<bool>`/`Derived<bool>`) in scope, and
   Rust method resolution picks the impl from the condition's *type*. Only
   bare `path`/`field` conditions route through this dispatch — every
   provably-`bool` condition (literal, `&&`/`!`, a function/method call,
   a comparison) stays a plain borrowing `if`.

Consequence — and the deliberate contract: an **opaque** `fn() -> bool`
call like `if del_visible()` (where `del_visible` is a plain closure) is
**static**. Reactivity must be carried by a reactive *type* (`memo` /
`Signal<bool>`) or a *visible* `.get()`; it is never inferred from an
opaque call's hidden body. This keeps a genuinely static `if helper()`
free of any reactive machinery, and is why `del_visible` is authored as a
`memo` rather than a bare `move || …` closure. (`if let PAT = EXPR { … }`
is always a plain static `if let` — a re-binding reactive `if let` is not
a construct; use `match sig.get() { … }`.)

This mirrors the `for` loop's `StaticForEach` / `ReactiveForEach`
type-dispatch: reactivity lives in the *type*, not in a guess about
syntax.

### Reactive `match`

The DSLs lower `match scrutinee { ... }` to `switch(...)` when the
scrutinee contains `.get()`. The arms then become the branches and
the framework's switch primitive handles the rebuild-on-key-change
logic.

### Text f-strings

A string literal in text position interpolates `{name}` placeholders,
the way Rust's own `format!` treats inline named arguments:

```rust
let count = signal(0);
let doubled = memo(move || count.get() * 2);

ui! {
    text { "count: {count}   doubled: {doubled:.1}" }
}
```

Each slot classifies by the interpolated value's **type** — the text
analog of `if is_high`'s `StaticCond`/`ReactiveCond` dispatch:

- a `Display` value bakes in **statically** (zero reactive machinery);
- a `Signal<T>` / `ReadSignal<T>` (memo output) becomes a **live
  slot** — no closure, no `.get()`. Signal slots build a pre-decomposed
  template binding (`TextSource::JsBinding`), so the web backend's
  JS-side fast path applies; other backends fall back to the Effect
  path;
- a `Reactive<T>` prop interpolates too: a static prop bakes in, a
  live one keeps the text live (via the Effect path — a `Dynamic`
  prop has no signal id for the template binding).

Format specs pass through (`{ratio:.2}`, widths, fill); `{{`/`}}`
escape literal braces. The rules are prose-first: a literal with **no**
valid `{ident}` placeholder never changes meaning (braces render
verbatim — `"use { here"` is fine), while a literal that *does*
interpolate treats malformed braces as compile errors. Positional
`{}`/`{0}` and Debug `{x:?}` are not supported in text f-strings —
reach for `text { move || format!(…) }` for those (the closure form is
the general escape hatch and remains fully supported).

### Why this matters for extensibility

The contract a UI macro needs to satisfy is small:

1. Emit calls to `runtime_core::{text, button, view, when, switch}`
   for built-in primitives.
2. Emit a `BuildElement` struct literal for user components (the tag
   alias `#[component]` generates).
3. For reactive conditionals, wrap dependency closures with
   `runtime_core::when` / `switch`.
4. Coerce the final expression via `IntoElement::into_element(...)`.

Anything that satisfies those four can serve as a front-end. The
shipped `jsx!` is the proof-of-concept: identical primitive output,
different surface grammar, fully interoperable in the same component.

The template lowering satisfies the same four, just indirectly — the
builder in `runtime_vocabulary::template` makes the calls on the
descriptor's behalf. That is the other half of the claim: the surface
DSL is a frontend, and so is the *lowering strategy*.

---

## The primitive builders

Primitive constructors don't return `Element` directly. They return a
small builder holding the in-progress payload and exposing a fluent
surface (`crates/runtime/vocabulary/src/glue.rs`):

```rust
pub fn button(label: impl TextContent, on_click: impl IntoAction) -> GlueButton { … }

button("Click", || …)
    .with_style(primary_button_style())
    .bind(my_ref)
    .disabled(move || disabled.get())
```

Each builder method fills one of the payload's optional slots and
returns `Self`. When the chain ends inside `ui!` children, the
`IntoElement` impl turns the builder into an `Element::Item` carrying
the finished payload.

This is what makes `style = ...` work uniformly on every primitive: the
DSL emits `.with_style(expr)` on the constructed builder, the builder
stuffs it into the payload's `style` slot, and the primitive's handler
attaches it at mount. The universal setters — `with_style`, `test_id`,
`accessibility`, `a11y_*`, `live_region` — are generated once for every
builder by a shared macro, so they exist on all of them by
construction.

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
into the surrounding `Vec<Element>`:

- `Element` → push as-is.
- `Option<Element>` → push if `Some`.
- `Vec<Element>` → extend.
- a primitive builder → convert and push.
- Iterators in `for` blocks → push each.

This is why `if let Some(x) = … { text { x } }` and `for item in items
{ text { item.name.clone() } }` work seamlessly inside `ui!` without
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

Architecturally, `Navigator` is "a `Element` that holds a route
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
| Add a new built-in primitive | A payload struct in `runtime_vocabulary::prims` + a mount handler in `runtime_vocabulary::handlers` (registered by `register_builtins`) + any new `caps::*Ops` method, with a default |
| Add a third-party primitive | A payload struct + a handler registered at the app's boot seam — no framework change ([`external-export.md`](./external-export.md)) |
| Add a new user-facing component | A `#[component] fn name(...) -> Element` in app code |
| Add imperative methods on a component | `#[method] fn foo(…) { … }` nested fns inside the `#[component]` body |
| Make a prop reactive | Pass a signal or a closure containing `.get()`; the constructor takes `impl IntoValue<T>`, which lowers to `Value::Dyn` |
| Add a new DSL | A new proc-macro that emits primitive / `name!` calls (see [`ui-layer.md` § DSLs](#dsls)) |
| Add a new style property | A field on `StyleRules` + the matching `stylesheet!` grammar + a backend branch in `StyleOps::apply_style` |
| Wire imperative platform features | A new method on the relevant handle `*Ops` trait + backend impl + handle method |

Each one is a localized change — none of the others has to know.

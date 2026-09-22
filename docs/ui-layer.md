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

### Why the split exists

`ui!` has ONE lowering. The split is not a step toward a second one — it
is the producer of a **descriptor**: the data half of a site, addressable
and patchable without recompiling.

Most of a UI tree is not code. Tags, attribute names, child order, string
and number and bool literals, enum-like paths, style-token accessors —
all of it is data that happens to be spelled in Rust. Separating that
half makes a static edit (a changed label, a reordered child) a DATA
change, and that is the basis for changing one without a rebuild.

`runtime-template` (`crates/runtime/template`) owns the types —
`Descriptor`, `Node`, `PropEntry`, `SlotSig`, `SiteId`, `Registry`,
`Patch`, `diff`. It depends on `serde` and nothing else, so a descriptor
serializes and diffs with no renderer in the graph.

`runtime-macros-parse` (`crates/runtime/macros-parse`) is the front end
that produces them: the `ui!` parser, the split pass, the node numbering
and the IDE-recovery shell, as a PLAIN library. `runtime-macros` is the
emission over it. The split matters because two callers need the same
answers — the proc macro expanding `ui!` inside rustc, and the CLI
reading a crate's sources at build time — and a proc-macro crate cannot
be depended on as a library. While the parser lived inside one, the
second caller had to reimplement it, and a reimplementation that
disagreed by a single node would mis-address every patch after that
node, silently.

What CONSUMES the descriptor is a dev-time overlay — see [The dev-time
overlay](#the-dev-time-overlay) below — not a second emission.

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

The DSL is a frontend. What it emits — one lowering, builder calls
inline — is the framework's only structural commitment.

---

## The dev-time overlay

Behind the `ui-overlay` cargo feature (off by default; `idealyst dev
--web` turns it on, `idealyst build --web` never does), the emission adds
one thing per node: an **origin tag**.

```rust
runtime_core::__overlay::tag(<the node's expression>, <site key>, <node index>)
```

Two integer literals and a call. `site key` is
`runtime_template::site_key` of the `ui!` invocation's package,
package-relative file, line and column; `node index` is the number the
split pass gave that node. A patch names the same pair, carries its new
values inline, and is applied inside `tag` — which is the point every
node passes through on **every** build of its site. That is what makes a
patch survive a rebuild: a site's `Element` is rebuilt whenever its
reactive scope re-runs, and a patch applied once to a tree that is then
rebuilt from the compiled code would silently revert.

A `#[component]`'s props are the exception. By the time its `Element`
exists they have been consumed and its body has run, so `ui!` brackets
the build expression with an ambient address instead:

```rust
runtime_core::__overlay::enter(SITE, NODE);
runtime_core::__overlay::exit(BuildElement::build(Badge { .. }))
```

and the component's own generated `BuildElement::build` reads it, applies
any staged literals through `__apply_literal`, and registers the
component's constructor.

Everything the call site emits is free functions taking integers —
nothing generic, nothing needing a trait in scope. That is a measurement,
arrived at in three steps: writing the work inline at each call site cost
+1.7 s on CrewForge's one-edit rebuild, moving the bodies into per-type
generated methods got it to +0.8 s, and the ambient pair removed the last
thing a site had to resolve (an inherent method whose fallback is a
blanket `impl<T>`, which pulls trait selection into every one of
thousands of sites).

Running a component again needs a `Clone` copy of its props, and the
autoref-specialization probe that decides whether one exists was first
emitted at the call site — which put trait selection back, at +1.2 s. It
belongs on the props TYPE: `#[component]` generates an
`__overlay_rebuilder` beside the other per-type items and the type's own
`build` hands the result to the open ambient frame. Re-measured on
CrewForge (arm64, 9 one-edit rebuilds per arm, interleaved): 6.18 s off
vs 6.22 s on at the minimum, and `macro_expand_crate` 1.72 s vs 1.82 s —
the overlay is back inside the noise of the feature being off.

Because registration now happens inside `build`, the contract for
inserting a component is "any component type this program has built
once", not "any component currently on screen". And because the ambient
frame is TAKEN rather than read, a component built by hand inside
another's children finds nothing rather than applying a patch addressed
to its neighbour — the patch is lost, which is a rebuild, instead of
being misapplied, which is a wrong screen with nothing to say so.

### Descriptors are a build artifact, not binary contents

They used to be compiled in — a `static Descriptor` per site, registered
at build time. Measured on a real app (CrewForge, ~256 codegen units)
that cost **+1.4 s on every one-edit rebuild**:

| one-edit rebuild | overlay off | tags only | tags + in-binary descriptor |
|---|---|---|---|
| a string literal in a screen | 5.04–5.30 s | 5.63–5.64 s | 6.83–6.97 s |
| a trailing comment | 4.96–5.02 s | 4.85–5.02 s | 6.47–6.64 s |
| `macro_expand_crate` | 1.64–1.65 s | 1.69–1.79 s | 2.51–2.60 s |
| `serialize_dep_graph` | 0.37–0.60 s | 0.39–0.60 s | 0.61–0.78 s |

A 27% tax on exactly the loop the overlay exists to shorten, all of it in
expansion and dep-graph serialization and none of it in codegen — while
keeping only the TAGS measured within noise of the feature being off.

So the descriptor is produced from SOURCE at build time, by the same
split pass, and written beside the build:
`target/idealyst/<app>/overlay/<build>.json`, where `<build>` is a digest
of every scanned file's content plus the split-pass version. The compiled
program carries only the tags and one `IDEALYST_UI_SPLIT_VERSION` marker,
so a tool can refuse a binary numbered by a walk it does not know.

Three things follow, and each is better than the old arrangement:

1. **Validation moves to where the evidence is.** "Does this edit disturb
   an expression the compiled code supplies?" needs BOTH source versions.
   The differ has them; a running app never did.
2. **An over-the-air path archives each build's descriptor set** keyed by
   build id, rather than reading it back out of a shipped binary.
3. **The dev loop pays nothing** for a feature that exists to make the
   dev loop faster.

### What patches, and what rebuilds

A save during `idealyst dev --web` takes one of two paths. This is the
whole table:

| the edit | what happens |
|---|---|
| a string, number or bool literal in a `ui!` body | **patched** |
| a `#[component]`'s literal prop | **patched**; live when its props are `Clone` and its root is a node, otherwise on that site's next render |
| …of a component whose root is a `switch`, `when` or keyed list | **patched**; live when the seam has a setter for the prop (a text's content, a button's label), otherwise on that site's next render — the region's contents carry the tag, so the node is reached and the refusal names it |
| a static child added, removed or reordered — where every old child is fully static | **patched** |
| a changed `if` condition, `for` iterable or `match` scrutinee | rebuild — it is compiled code |
| a literal becoming a closure, or the reverse | rebuild — the value moved between data and code |
| a changed style token or enum path (`t.card()`, `tone::Danger`) | rebuild — recorded as source TEXT, and no value can be rebuilt from a string |
| a structural change where any old child carries a style or other slot | rebuild — its prop bindings are owned by the enclosing scope, not the node |
| anything outside a `ui!` body, in a file that also has one | rebuild — the whole file's save rebuilds |
| a `ui!` body gaining or losing a LINE | **patched** — the sites below it re-key, but the save is matched to the build by ORDINAL and the patch is addressed to the key the binary carries |
| a `ui!` nested inside another macro's tokens (`vec![ui!{…}]`) | **patched** — every macro's token tree is walked for them |
| a site ADDED or REMOVED | rebuild — a new site has no compiled tag to address at all |
| a `jsx!` body | rebuild — `jsx!` has its own grammar, produces no descriptor, and carries no tags |

One practical consequence worth knowing before you reach for this:

**A heavily styled tree patches its text but not its shape.** Almost
every node in a real app carries `style = …`, which is a slot, so the
fully-static requirement for a structural change is rarely met. Text and
literal props are where the win is. (Letting a styled sibling be
inserted needs slot aliasing — knowing that the new node's `style`
expression is the same compiled code as its neighbour's. Not built.)

### Why a moved site still patches

A site is keyed by `(package, file, line, col)`, so a `ui!` body gaining
a line re-keys every site below it in that file — while the running
binary still carries the OLD keys in its tags. Keying alone would make
"add one line to a `ui!` body" cost a full compile.

So the build's descriptor set records, per site, its compiled **key** AND
its **ordinal** among that file's `ui!` invocations. Both the archive and
the on-save scan walk the file in document order, so the ordinals line
up; the save is matched by ordinal, the current source is described under
the ARCHIVED site id, and the patch is addressed to the key the binary
actually has. The key is never advanced by a patch — only by a rebuild,
which is the only thing that changes what the binary's tags say.

The ordinal set changing — a site added or removed — is the one case
that still rebuilds, and it has to: a new site has no compiled tag
anywhere to address.

Nested sites count in that ordering too. `syn` does not descend into a
macro's tokens, so `pressable(vec![ui! { … }], …)` was invisible to the
scanner even though the macro expands it and tags it. Every macro's
token tree is now walked for `ui !` groups, recursively, and the results
are merged and sorted by byte offset — because a nested site appended at
the end instead of slotted into document order would renumber every site
after it, and every later patch would address the wrong one.

### What an edit can and cannot change

`runtime_template::diff(old, new)` turns two descriptors into a `Patch`
or a `Rejection`. It refuses everything that would mean the compiled code
changed — a different slot signature, a prop that moved between data and
a slot, a changed `if` condition / `for` iterable / `match` scrutinee, a
changed style token (recorded as source text, because a value of an
arbitrary type is not reconstructible from a string), a new subtree
referencing a slot, or any structural change around a control-flow node
whose position is decided at runtime.

A refusal is not a failure. It is the differ saying "this one needs a
rebuild", which is the correct and available answer. The applier refuses
the same class of thing again at runtime — a reactive prop, a child list
holding a reactive region — checking the LIVE tree rather than trusting
the patch, and counts what it applied so a dev server can say so rather
than showing a tree that is neither version.

`crates/dev/ui-lowering-parity` closes the loop end to end: for each
edit pair it asserts that `Element(original)` plus the diff of the two
descriptors renders exactly like `Element(edited)`, on all three
projections of a real mount.

### Two application paths, and why both

A patch is applied twice, and neither half subsumes the other:

- **the `Element` path**, inside `__overlay::tag`, changes what the NEXT
  build of the site produces. Without it a patch evaporates the moment a
  signal fires and the site rebuilds itself from the compiled code.
- **the LIVE path**, `runtime_vocabulary::overlay::apply_live`, reaches
  the instances already mounted. Without it nothing visible happens
  until something re-renders.

The live path is generic over `H: AllCaps` and issues ordinary
capability calls — `update_text`, `update_button_label`, `set_disabled`,
`insert`, `remove_child`. There is no backend in it and no
`cfg(target)`: every backend gets it from one replay, which is the
standing rule about where platform differences are allowed to live.

**A node is reachable live only if it is TAGGED and mounted.**
`runtime_scene::live` registers every node `mount_item` builds, so a
subtree a handler realized into its own storage — every navigator screen
— is reachable, and a subtree that has unmounted is not. Registration is
at the one place every mounted node passes through, so a handler that
does not exist yet is covered too.

`with_tag` tags an `Item` root, recurses through an `Owned` to reach
one, and follows a REACTIVE REGION to its contents — idea-ui's `Button`
returns a `switch` the moment a structural prop is live, and a region
has no node of its own, so the contents are the only thing the call site
ever puts on screen. The tag is attached from INSIDE the region's build
closure, which a region runs again on every swap: the branch showing now
carries the tag, the branch that replaces it carries it too, and each
one registers on its way through `mount_item` while the outgoing one's
registration dies with its subtree. A `Fragment` root is still left
alone — it stands for several sibling nodes and none of them is *the*
node the call site's tag belongs to.

A live edit also needs a SETTER on the seam, or — for a component — a
way to run it again: a `#[component]`'s prop has no setter (its body
already ran, and re-running it would need every dynamic prop it was
given, which is compiled code the patch does not carry). Running it
again needs a `Clone` copy of its props AND a node whose place the
replacement can take, which a region's contents do not have: they stand
inside the region's anchor, which belongs to the region's own driver.
So a region-rooted component's prop applies live when the seam has a
setter for it and otherwise on the site's next render. The applier
COUNTS what it could not do rather than pretending, so a dev server can
say "showing on next render" instead of leaving the author wondering —
and, since the node is now reached, it says it about the node the author
edited rather than reporting nothing at all.

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

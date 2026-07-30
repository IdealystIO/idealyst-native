# Component patterns — signals, refs, memos, and friends

> **Status: review draft.** Written against the 0.3 surface (plain `signal`/
> `memo` fns, inline props, `ReadSignal`/`WriteSignal`). Once reviewed, this
> is intended as the hands-on companion to [ui-layer.md](ui-layer.md) (which
> explains the machinery; this shows the idioms).

Every pattern below follows the house rules: components wear `#[component]`
(CLAUDE.md §9.1), bodies compose with `ui!` (§9.2), children/conditions/loops
live inside the macro (§9.3–9.4), and signal props declare the narrowest
capability (§9.6a).

---

## 1. The smallest component: local state + reactive text

```rust
use runtime_core::{component, signal, ui, Element};

#[component]
pub fn Counter(#[prop(default = 0)] start: i32) -> Element {
    // `start` arrives as Reactive<i32> (reactive-by-default props) — read
    // with .get(). The signal is component-local: the surrounding scope
    // owns it and frees it on unmount.
    let count = signal(start.get());

    ui! {
        view() {
            // Reactive text: `{count}` is live because `count` is a
            // SIGNAL — f-string slots classify by type, like every other
            // reactivity decision. (A bare `text(count.get())` would be a
            // stale snapshot, and is a compile error by design; the
            // closure form `text(move || format!(…))` also works.)
            text { "Count: {count}" }
            button(label = "Increment", on_click = move || count.update(|n| *n += 1))
        }
    }
}
```

Things this demonstrates:

- **Inline props.** The macro generates `CounterProps` from the parameter
  list; `ui! { Counter() }` and `ui! { Counter(start = 5) }` both work, and
  `start = some_signal` passes a *live* value without any change here.
- **Signals are `Copy`.** `count` moves into both closures with no
  `.clone()` ceremony.
- **Reactivity lives at the leaves.** The component fn runs ONCE. Only the
  `text` binding re-renders when `count` changes — there is no re-invocation,
  no VDOM, no diff.

## 2. Derived state: `memo` vs `rx!` vs a plain closure

Three tools, one decision rule — *how expensive is the derivation, and how
many places read it?*

```rust
let items: Signal<Vec<Item>> = signal(Vec::new());

// (a) memo — cached, change-gated, read in several places. Returns
// ReadSignal<usize>: nobody can .set() a derivation.
let total = memo(move || items.get().iter().map(|i| i.price).sum::<usize>());

// (b) rx! — an inline live prop value; no caching, terse.
ui! {
    Badge(label = rx!(format!("{} items", items.get().len())))
}

// (c) a plain closure — reactive text/bindings that are trivial to recompute.
ui! {
    text(move || format!("total: {}", total.get()))
}
```

- `memo` recomputes **once per dependency change** (not per read) and only
  notifies subscribers when the value actually differs (`PartialEq`; use
  `memo_with(eq, f)` for types without it). The body must be pure — a
  `.set()` inside panics.
- Don't cache what's trivial: for a cheap one-off derivation, (b) or (c) is
  lighter than a memo's bookkeeping.

### The snapshot trap — the one rule to internalize

`.get()` subscribes **whoever is currently running** — an effect, a memo, a
`when` condition, a text-binding closure. The component body itself is NOT a
tracked context: it runs exactly once, at build. So a derivation hoisted
into a plain `let` is a **frozen snapshot**, and using it downstream looks
reactive while never updating:

```rust
#[component]
fn NameField(name: Signal<String>) -> Element {
    // ✗ THE TRAP: runs once at build. `too_short` is a plain bool, frozen
    // forever — the branch below will NEVER react to typing.
    let too_short = name.get().len() < 3;

    ui! {
        if too_short {                 // bool → static branch, silently
            text { "Name is too short" }
        }
    }
}
```

The fix is to keep the derivation **behind `move ||`** — then the *type* of
the binding says it's live, and the `if` dispatches reactively because of
that type:

```rust
    // ✓ ReadSignal<bool> — recomputes when `name` changes.
    let too_short = memo(move || name.get().len() < 3);
    // …or inline the read into the condition. Under the 0.4.0 inverted gate,
    // any condition that might read a signal (a `.get()` or any call) is
    // reactive; only a provably signal-free condition stays static:
    //   if name.get().len() < 3 { … }   // reactive
```

One sentence to remember: **a `let` freezes, a closure flows.** Every
fine-grained framework shares this rule (Solid and Leptos are identical) —
it's the price of run-once components, paid back by never diffing.

**The framework catches this trap twice:**

- **At runtime (debug builds):** a `.get()` during a component build with no
  tracked consumer logs a warning naming the component — *"this read is a
  one-time snapshot and will NEVER update"* — on first render, in the
  console you're already watching. Zero cost in release builds.
- **At edit time:** the `snapshot-condition` lint rule flags a hoisted
  `.get()` binding used as a `ui!` `if` condition, at the `let` where the
  fix goes. `idealyst dev` runs the linter ambiently on startup, so this
  appears without ever invoking `idealyst lint`.

Build-time snapshots are still a legitimate tool when they're *intentional*
(e.g. a structural choice that shouldn't rebuild — IconButton snapshots its
icon-vs-glyph choice this way). Declare the intent with
**`.peek()`** — it reads without subscribing, silences both diagnostics,
and tells every future reader "snapshot, on purpose." (On a `Reactive<T>`
prop the same intent is spelled `.get_untracked()`.) The distinction is
intent: snapshot with `.peek()`, derive with `memo(move || …)`, and treat
a bare `.get()` outside a closure as a smell.

## 3. Conditions and lists — literal Rust, inside `ui!`

```rust
let filter = signal(String::new());
let rows: Signal<Vec<Row>> = signal(load_rows());
let has_rows = memo(move || !rows.get().is_empty());

ui! {
    view() {
        // `if` over a reactive bool (Signal, memo output, or Derived) is a
        // live branch — it swaps subtrees when the condition flips, and the
        // hidden branch's state is DISPOSED (dispose-on-hide).
        if has_rows {
            // Reactive keyed iteration. `key =` is REQUIRED for a reactive
            // collection (compile error without it): rows reconcile by key,
            // so a surviving row keeps its component-local state.
            for row in rows, key = row.id {
                RowView(row = row)
            }
        } else {
            text { "No rows yet" }
        }
    }
}
```

- A plain `bool` condition is static (built once, no reactive machinery) —
  the *type* decides, not a syntax guess.
- `for` over a plain `Vec`/iterator is likewise static; `key =` is accepted
  but unused there.
- State that must survive a branch toggle belongs in the parent scope —
  anything created inside the hidden branch is gone when it hides.

## 4. Refs: imperative handles to rendered primitives

A `Ref<H>` is a `Copy` arena handle the backend fills at mount with a typed
platform-neutral handle (`TextInputHandle`, `ViewHandle`, …).

```rust
use runtime_core::{node_ref, text_input, Ref, TextInputHandle};

let query = signal(String::new());
let input_ref: Ref<TextInputHandle> = node_ref!(); // or Ref::new()

// Builder form: constructors return a primitive builder; `.bind(ref)` attaches.
let input = text_input(query, move |v: String| query.set(v))
    .bind(input_ref)
    .placeholder("Search…");

// Later — an event handler, an effect, wherever:
if let Some(h) = input_ref.get() {
    h.focus();
}
// Or without cloning the handle out:
input_ref.with(|h| h.focus());
```

Rules of thumb:

- `get()`/`with()` return `Option` — `None` before mount and after unmount.
- Do **not** call `.set()` on a signal inside a `Ref::with` closure — `with`
  holds the arena borrow and the write aborts ("RefCell already borrowed").
  Read out, close the closure, then write.
- Refs are scope-owned like signals: they die with the component.

### Giving a component a ref — two tiers

**Tier 1 — the forwarded ref (`bind_to` convention).** When the caller
wants the *underlying primitive's* handle (focus it, anchor an overlay to
it), the component exposes a prop and forwards it to its root primitive
(idea-ui's IconButton is the canonical example):

```rust
#[component]
fn IconButton(bind_to: Option<Ref<PressableHandle>>, /* … */) -> Element {
    let mut bound = pressable(children, move || on_click());
    if let Some(r) = bind_to {
        bound = bound.bind(r);   // caller's ref fills at mount
    }
    bound.into_element()
}
```

**Tier 2 — the component's own imperative surface (`#[method]`).** When
the component should expose its *own* API instead of leaking its root
primitive, mark nested fns with `#[method]`: the macro generates a typed
handle struct (fn name + `Handle`) carrying them. No `pub` (the handle
carries the public surface), no `&self` (state comes from the closure
capturing the component body — the attribute is what licenses a "nested
fn" to capture), and `()` returns only: **methods are commands, not
queries** — reads stay signals, so the handle can never become a
subscription-bypassing read side-channel.

```rust
#[component]
pub fn FlowChart() -> Element {
    let nodes: Signal<Vec<Node>> = signal(Vec::new());
    let next_id: Signal<u64> = signal(0);

    /// Append a node with a fresh id.
    #[method]
    fn add_node(label: String) {
        let id = next_id.get();
        next_id.set(id + 1);
        nodes.update(|n| n.push(Node { id, label }));
    }

    ui! {
        view() {
            for node in nodes, key = node.id { text { node.label } }
        }
    }
}
```

Binding uses the **ordinary tag form** — the macro auto-injects a
`bind_to: Option<Ref<FlowChartHandle>>` prop and fills it in-body:

```rust
let chart: Ref<FlowChartHandle> = Ref::new();

ui! {
    view() {
        FlowChart(bind_to = chart)
        button(label = "Add node", on_click = move || {
            if let Some(c) = chart.get() {    // get(), NOT with() — see below
                c.add_node("New step".into());
            }
        })
    }
}
```

So refs attach the same way at both tiers: `bind_to` is *the* prop for
handles — explicit on components forwarding a primitive handle (tier 1),
auto-injected on `#[method]` components (tier 2). (Only the legacy
explicit-props form still returns `Bindable<Handle>` for fn-call
`.bind()` — an injected prop can't be added to an author-written props
struct.)

**Invoking methods: `.get()`, not `.with()`.** `Ref::with` holds the
arena borrow across your closure; a method that writes signals (like
`add_node`) would `.set()` inside that borrow — the "RefCell already
borrowed" abort. `chart.get()` clones the handle out (handles are
`Clone`), releasing the borrow before the call. Rule of thumb:
**`.with()` for reads, `.get()` for method calls that write.**

Both tiers share the `Ref` ground rules from above: `Option` before fill,
non-reactive fill. Note the slot itself lives in the shared substrate's
arena and is not freed on unmount — see
[`reactivity.md` § `Ref<H>`](./reactivity.md#refh--the-imperative-handle-slot).

### Attaching refs per DSL

`.bind(r)` lives on the primitive builders (fn-call form); `jsx!` sugars
it as a `ref={r}` attribute. `ui!` has no `ref =` prop — inside `ui!`,
attach refs through a component's `bind_to` prop, or build that one child
in fn-call form.

## 5. Props: the capability ladder

Reactive-by-default wrapping means a plain data param accepts a literal, a
signal, or an `rx!` computation. For *signal-typed* props, declare the
narrowest half that's honest (§9.6a):

```rust
#[component]
pub fn ItemList(
    /// Observe-only: the signature PROVES this component never mutates
    /// the caller's list. Callers pass `items = list` (coerces) or
    /// `items = list.read_only()`. Every memo output already is one.
    items: ReadSignal<Vec<Item>>,

    /// Report-up only: this child can push a selection out but cannot
    /// read it back (so it can't accidentally subscribe itself).
    selection_out: WriteSignal<Option<ItemId>>,

    /// Plain data — arrives Reactive<String>, callers may pass any of
    /// "static str" / a Signal / rx!(…).
    title: String,

    /// Genuinely two-way state keeps the unified handle — the component
    /// both reads and writes it (the controlled-input pattern).
    query: Signal<String>,

    /// Optional callback: the §9.6 shape. Defaults to None; bind only
    /// when present — never wire a silent no-op closure.
    on_activate: Option<Rc<dyn Fn(ItemId)>>,

    /// Receives the call site's `{ … }` children block.
    children: Vec<Element>,
) -> Element {
    // …
}
```

Container components that receive children just splat them:

```rust
ui! {
    view() {
        text(move || title.get())
        children          // bare-identifier child splat
    }
}
```

### The handler paradigm

On **primitives**, handlers are plain `move` closures — as props in `ui!`
(`on_click = move || …`, `on_change = move |v: String| …`) or chained on
the builder form (`.on_key_down(move |e: &KeyEvent| …)`, `.on_hover`,
`.on_touch`). `Copy` signal handles make capture ceremony-free.

On **components**, handlers are `Rc<dyn Fn(Args)>` props, with three
rules:

1. Required handlers are bare `Rc<dyn Fn()>`; optional ones are
   `Option<Rc<dyn Fn()>>`, bound **only when present** (§9.6) — never a
   silent no-op default, which blocks hit-test fall-through on some
   backends.
2. The props machinery deliberately leaves handler types **unwrapped**
   (the `#[props]`/inline-props skip-list): a `Reactive<Rc<dyn Fn>>` is
   meaningless — handlers are capabilities, not data.
3. Forwarding is a move-clone into the primitive's closure:
   `pressable(children, move || (on_click)())`.

Handler *bodies* run outside any tracked context: a `.get()` there is a
one-shot imperative read (correct — no snapshot warning), writes are
born-batched (several `set()`s in one handler coalesce into one
fan-out), and a handler firing late — after its component unmounted — is
a safe generational no-op, so fire-and-forget async completions are
fine.

## 6. Effects, cleanup, and reactivity outside the tree

```rust
// INSIDE a component: scope-owned, no handle to manage. The block is the
// reactive unit — dependencies are tracked automatically, `move` implied.
effect!({
    let q = query.get();
    log::info!("query is now {q}");
});

// Teardown pairs with the effect: fires before every re-run AND on disposal.
effect!({
    let stream = subscribe(topic.get());
    on_cleanup(move || stream.close());
});

// OUTSIDE the tree (app init, async callback, service): `watch` returns a
// Subscription whose lifetime YOU own — store it or `.leak()`.
let _sub = watch(move || {
    persist_setting(theme.get());
});
```

The split matters: `effect!` creates into the ambient world and the
enclosing ownership scope frees it; `watch` puts the effect in a private
scope and hands you the handle. Effect creation needs `World::enter`, so
`effect!` belongs in a component body, another effect, or any
world-entered build scope — not in an event handler
(`crates/runtime/world/src/lib.rs`). `watch` has the same requirement.

Note the cleanup shape: `on_cleanup` is legal only inside a **running
effect**, so the paired teardown above works because it sits in the
effect body. `on_cleanup` directly in a component body panics.

Two write-side idioms worth knowing:

- **Every turn is one batch.** A `set()` stages; the driver's flush commits
  every write made during the handler as one logical update, so several
  `set()`s in one handler produce one fan-out. There is no `batch(…)` to
  call — see [`automatic-batching.md`](./automatic-batching.md).
- **Read-modify-write uses `update`.** Reads never see a staged value, so
  `set(count.get() + 1)` twice in one handler nets `+1`;
  `count.update(|n| n + 1)` composes on the staged value and nets `+2`.
- A write after the owning **world** is gone (a late async callback on an
  unmounted app) is a **safe no-op**, not a crash. A write through a stale
  handle in a live world still panics — that's a real use-after-unmount.

## 7. Context: dependency injection down the scope tree

```rust
#[derive(Clone)]
struct Theme { accent: Color }

// In an ancestor:
provide(Theme { accent: color::parse("#7c3aed").unwrap() });

// In any descendant (keyed by TYPE — newtype to disambiguate duplicates):
let theme = inject::<Theme>();
```

Provisions live on the scope, so a `when`/`switch` branch that re-provides
shadows its parent for its own subtree and unwinds on dispose.

## 8. A worked example: search-filtered list

Everything above, composed — the shape most real components take:

```rust
use std::rc::Rc;
use runtime_core::{component, effect, memo, signal, ui, Element, ReadSignal, Signal};

#[component]
pub fn ContactPicker(
    /// Observe-only source of truth, owned by the caller.
    contacts: ReadSignal<Vec<Contact>>,
    /// Fires with the chosen id. Optional per §9.6.
    on_pick: Option<Rc<dyn Fn(ContactId)>>,
) -> Element {
    // Local UI state.
    let query = signal(String::new());

    // Derived, cached, read-only: recomputes only when the query or the
    // source list changes; notifies only when the RESULT differs.
    let visible: ReadSignal<Vec<Contact>> = memo(move || {
        let q = query.get().to_lowercase();
        contacts.get().into_iter()
            .filter(|c| c.name.to_lowercase().contains(&q))
            .collect()
    });
    let none_match = memo(move || visible.get().is_empty());

    ui! {
        view() {
            text_input(
                value = query,
                on_change = move |v: String| query.set(v),
                placeholder = "Search contacts…",
            )

            if none_match {
                text { "No matches" }
            } else {
                for contact in visible, key = contact.id {
                    // Conditional binding, not a silent no-op closure.
                    ContactRow(
                        name = contact.name,
                        on_press = on_pick.clone().map(|cb| {
                            let id = contact.id;
                            Rc::new(move || cb(id)) as Rc<dyn Fn()>
                        }),
                    )
                }
            }
        }
    }
}
```

What to notice, reading top to bottom: the caller keeps ownership of the
data (`ReadSignal` prop — this component provably can't mutate it); the
query is genuinely local so it's a local `signal`; the filtered view is a
`memo` because it's read by both the emptiness check and the loop; the
branch and the keyed loop are literal Rust inside `ui!`; and rows reconcile
by `contact.id`, so typing in the search box never tears down a row that
survives the filter.

## Anti-patterns (the linter catches most of these)

| Don't | Do | Why |
| --- | --- | --- |
| `signal!(0)` / `memo!(e)` | `signal(0)` / `memo(move || e)` | the macros are removed (0.3) |
| `Signal::new(0)` | `signal(0)` | redundant spelling (`prefer-signal-fn`) |
| `text(count.get())` | `text(move || count.get())` | bare read is a stale snapshot — compile error by design |
| `let ok = x.get()… ;` then `if ok { … }` in `ui!` | `let ok = memo(move || x.get()…)` or inline the `.get()` in the condition | hoisted snapshot — the branch silently never updates (see "The snapshot trap") |
| keyless `for x in sig { … }` | `for x in sig, key = x.id { … }` | reactive lists must reconcile — compile error by design |
| `for x in sig.get() { … }` in `ui!` | `for x in sig, key = x.id { … }` | `.get()` in the header freezes a build-time snapshot — the list renders once and never updates (`snapshot-loop`; see the `keyed_list_add_remove` recipe) |
| building children in a `Vec::push` loop outside `ui!`, or `.map(\|x\| ui!{…}).collect()` | write them inside `ui!` with `for … , key = …` (§9.3) | defeats keyed reconciliation + reactive-scope inference (`prefer-keyed-list`) |
| `.set()` inside a memo body | derive from inputs | memos are pure — panics |
| `.set()` inside `Ref::with` | read out, then set | `with` holds the arena borrow — aborts |
| a no-op default closure for an optional callback | `Option<Rc<dyn Fn()>>` + bind-when-present (§9.6) | silent handlers block hit-test fall-through |
| `Signal<T>` prop on an observe-only component | `ReadSignal<T>` (§9.6a) | the signature should prove what the component can do |

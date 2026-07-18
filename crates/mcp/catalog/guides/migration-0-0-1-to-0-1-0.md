+++
title = "Migrating 0.0.1 → 0.1.0"
order = 900
tags = ["migration", "0.1.0", "breaking", "reactivity"]
+++

# Migrating 0.0.1 → 0.1.0

> **Status: 0.1.0 is in development.** This is the living record of its breaking
> changes — each section carries a `Status:` line and fills in with concrete
> before/after as the change lands. See [[migrations]] for the versioning
> policy: 0.1.0 is a **clean, in-place break with no legacy shims**.

0.1.0 exists to make the framework *one* coherent experience before the surface
ossifies. The headline is the reactivity model: liveness becomes **type-driven
everywhere**, and the syntactic `.get()` heuristic that silently froze some UI
is removed. Update the git tag, then work the sections below.

```toml
idealyst = { git = "https://github.com/.../idealyst-native", tag = "0.1.0" }
```

---

## 1. Reactive control flow: the `use_focus()` freeze is fixed

**What changed.** In 0.0.1, `ui!` decided whether an `if`/`match` *condition* was
reactive purely by **scanning its tokens for the substring `.get()`**. A reactive
read spelled any other way — a hook returning `impl Fn() -> bool` (`use_focus()`,
`use_can_go_back()`) whose read is a *call*, not `.get()` — was treated as static
and **silently frozen**. This is the `use_focus()` "KNOWN ISSUE" some screens hit.

0.1.0 keeps the `.get()` bridge (a `.get()` anywhere in the condition is still
reactive) and **adds the missing case**: any **top-level call** condition is now
reactive too. `if use_focus()()`, `if state.is_active()`, `if is_active(state)`
lower to `when(move || cond, …)` — the framework's Effect tracks whatever signals
the call reads while it evaluates, so it's live. A call that reads nothing yields
an inert effect (fires once). No more silent freeze.

What stays **static** is a condition that is structural by construction — a
comparison, `&&`/`||`/`!`, a literal (`if kind == Kind::Scope`,
`if !name.is_empty()`, `if a && b`). Those read no signal, so making them reactive
would only impose `'static`/`Clone` capture ceremony for zero benefit. 0.1.0
deliberately does **not** force "every conditional reactive"; instead the
universal, *type-driven* escape for any reactive read the sugar can't see is the
**closure child** (§3): `{ move || … }` is a first-class reactive child, the
boundary is visible, and it composes `if`/`match`/`for`/helpers freely.

**Why not pure type-driven for `if`?** A condition's *type* is `bool` whether it's
static or reads a signal — the type can't distinguish them, so `if` needs a
syntactic bridge. 0.1.0 makes that bridge sound (catches calls, not just `.get()`)
and pairs it with the genuinely type-driven closure child for everything else.

**Migrate.**

Most call sites need **no change** — an `if`/`match` reading a signal via `.get()`
is identical, and a structural `if` is identical:

```rust
// unchanged across 0.0.1 → 0.1.0 — the `.get()` condition is reactive both ways
ui! {
    if count.get() > 0 { text("has items") }
    else { text("empty") }
}
```

Conditions that were **frozen** in 0.0.1 (a reactive read via a call, no `.get()`)
now just work — no edit needed:

```rust
// 0.0.1 — SILENTLY STATIC (bug); 0.1.0 — reactive, the top-level call is tracked
ui! {
    if use_focus()() { ActiveBadge() } else { view {} }
}
```

The one residual gap — a reactive read *buried inside* a structural comparison
(`if some_field && helper_reading_a_signal_without_get()`) — is authored with the
visible closure boundary:

```rust
ui! {
    { move || if cond_reading_a_hidden_signal() { ui!{ A() } } else { ui!{ B() } } }
}
```

For a computed child that reads signals through helpers, indexing, or a
`Ref::with`, the rule is uniform: **wrap it in `move ||` to be live, leave it bare
to be static** (§3).

**Landed lowering.** The `if` rule targets the *actual* footgun — a reactive read
the old scan couldn't see — without taxing conditions that are static by
construction:

- `if cond { … }` is reactive (`when(move || cond, …)`) when the condition can
  carry a signal read: a `.get()` **anywhere** in the condition
  (`if sig.get() > 0`, `if !flag.get()`), **or** a **top-level call**
  (`if use_focus()()`, `if state.is_active()`, `if is_active(state)`). A call may
  read a signal that isn't spelled `.get()` at the call site — the 0.0.1
  `use_focus()` freeze — so any `Call`/`MethodCall` condition is now live (the
  framework's Effect tracks whatever the call reads; a call that reads nothing is
  an inert effect that fires once). `when` dedups on the bool = Leptos `<Show>`.
- `if cond { … }` stays a **plain static `if`** (borrowed captures, no ceremony)
  when the condition is purely structural — a comparison, `&&`/`||`/`!`, a literal
  (`if kind == Kind::Scope`, `if !name.is_empty()`, `if a && b`). These read no
  signal by construction, so lowering them to `move ||` would only impose
  `'static`/`Clone` capture ceremony (cloning an `Rc` per branch, threading owned
  strings) for zero reactive benefit. This deliberately does **not** chase the
  literal "every conditional is reactive": the one residual gap — a non-`.get()`
  reactive read *buried inside* a structural comparison — is authored with the
  visible closure boundary `{ move || if cond { … } else { … } }` (an
  `Element::Dynamic`, §3), the same escape hatch every reactive child uses.
- `if <Signal<bool>>` / `if <Derived<bool>>` bare-path dispatch (`__idealyst_if`)
  is unchanged — a bare `bool` stays static, a reactive bool value is live, by
  type.
- `match scrut { … }` is **reactive-when-possible**: a `switch(move || scrut, …)`
  when the scrutinee reads a signal (`.get()`) or is a signal-driven call
  (`match key(state)`, args rewritten to `.get()`); a plain static `match`
  otherwise. Unlike `if`, `match` is *not* broadened to all top-level calls:
  `switch` requires `S: PartialEq + 'static` and a re-evaluatable scrutinee, so
  forcing e.g. `match path.extension()` (a borrow, not `'static`) reactive would
  break far more than it fixes. `switch` dedups on the key.

**Breaking edges (all compile-time-loud, never silent freezes):**

- **Clone/`'static` tightening.** A now-reactive `if`/`match` branch (call or
  `.get()` condition) sits in a `move ||` closure, so a branch that renders a
  borrowed/non-`Clone` value needs a `.clone()` / `'static` capture. Structural
  `if`s are untouched (they stay borrowed). In practice this bit only a handful
  of call-condition sites workspace-wide.
- **Multi-node reactive child branches gain a wrapper.** In child position, a
  reactive `if cond { A B C }` wraps the multi-node branch in one node (via
  `emit_block_as_primitive`) — a reactive anchor needs a single root per branch.
  A static `if` still flat-splats. Split into separate `if`s if the flat siblings
  matter.

Status: **landed** (branch `feat/0.1.0-reactivity`) — `emit_if` + regression
tests (`tests/if_reactive_lowering.rs`). `match` was already reactive-when-possible
in 0.0.1 and is unchanged.

---

## 2. Reactive text is a closure — the `.get()` text scan is gone

**What changed.** In 0.0.1 `ui!` decided whether a `text { … }` content was
reactive by scanning it for `.get()` and auto-wrapping the match in a closure.
That scan is removed. Reactive text is now decided by TYPE, exactly like a
reactive child (§3):

- `text { move || … }` — a **closure** content is reactive; re-evaluates and
  patches the text in place when a signal it reads changes.
- `text { "literal" }`, `text { some_value }`, `text { format!("…") }` (no live
  read) — a **value** content is static, built once.
- `text { rx!(…) }`, `text { my_reactive_string }` —
  reactive **by type**: the value's type (`Reactive<String>` / a `Signal<String>`
  handle / a `TextSource::JsBinding` value) carries the liveness; `ui!`
  passes it straight through to `IntoTextSource`.

A **bare signal read** in text (`text { count.get() }`, `text { format!("{}",
x.get()) }`) is no longer auto-wrapped — it would be a *silent freeze* (rendered
once, never updated). `ui!` rejects it with a **compile error** pointing at the
closure form, so the footgun can't happen quietly. (The rare false positive — a
no-arg `.get()` that isn't a signal, like `Cell`/`OnceCell::get()`, in bare text —
is resolved by binding to a `let` first.)

**Why.** Same reason as §1: a syntactic `.get()` proxy has false negatives (a
reactive read via a helper with no `.get()` froze). Deciding by type makes the
reactive boundary visible and correct however the read is spelled.

**Migrate.**

```rust
// 0.0.1 — auto-wrapped by the scan
ui! { text { count.get() } }
ui! { text { format!("Count: {}", count.get()) } }

// 0.1.0 — the closure is the visible reactive boundary
ui! { text { move || count.get() } }
ui! { text { move || format!("Count: {}", count.get()) } }
```

If a `ui!` text closure reads a **borrowed prop** (`props.x`) and hits a
`'static` error, hoist a local first: `let x = props.x.clone(); … text { move || x.get() }`.
A **direct** `text(…)`/`button(…)` call inside a `#[component]` (outside `ui!`)
still has its parameter-rooted paths auto-cloned into the closure — write
`text(move || props.x.get())` and the macro makes the closure `'static`.

**`text_fmt!` / `bind!` were RETAINED at 0.1.0** (removed later in 0.3, where
typed f-string text literals — `text { "count: {count}" }` — produce the same
`TextSource::JsBinding` with no sentinel; see the 0.2→0.3 migration guide).
The 0.1.0-era rationale, kept for the record: they are not vestigial
sugar: `text_fmt!` compiles a `TextSource::JsBinding` (template parts + signal ids
+ per-signal stringifiers) that the **web backend updates on the JS side without a
wasm round-trip** — a real hot-text performance path the benchmarks use — and
`bind!` is its load-bearing signal marker (distinguishing a subscribed `Signal`
from a baked-in captured value, which a `.get()` scan alone can't do). This
coexists cleanly with the type-driven model: a `text_fmt!(…)` value is reactive
**by type**, so `text { text_fmt!(…) }` is passed straight through. Use the closure
form for general reactive text; reach for `text_fmt!` when you want the JS-binding
fast path for frequently-updated text on web. `rx!` likewise stays as the inline
reactive-prop form (`Typography(content = rx!(format!("…", count.get())))`).

Status: **landed** (branch `feat/0.1.0-reactivity`) — `emit_text` +
`reactivity.rs` closure-driven, footgun guard, `jsx!` already type-driven,
regression tests (`tests/text_reactive_lowering.rs`). Sweep confirmed the tree was
already almost entirely in the `text(move || …)` form.

---

## 3. `Element::Dynamic` + closures as reactive children — **LANDED**

**What changed.** A new `Element::Dynamic` primitive is the generic reactive
single-subtree (the whole-element dual of a reactive `text(move || …)` leaf); a
`dynamic(build)` constructor and blanket `impl IntoElement`/`ChildList for F:
Fn() -> E` make *any* closure returning an element a first-class reactive child.
Additive — every existing static child keeps working unchanged.

**Why.** It closes the "support any Rust expression as a child" goal for the
reactive case too — `for`, `if`, `match`, helper calls, iterator chains all
compose as children, static when bare and live when wrapped in `move ||`. Same
first-class-value story as SolidJS/Leptos, in plain Rust. The walker tracks the
build closure's *eager* reads and rebuilds on change (dispose-on-hide); inner
reactive constructs defer to their own effects (tracked-build/untracked-construct
split, proven in `tests/dynamic_reactive.rs`).

**Migrate.** Nothing to do — additive. Reach for it when you want a reactive
child without the `if`/`match` sugar:

```rust
ui! {
    view {
        // reactive: rebuilds when `filter` changes
        move || rows.get().iter().filter(|r| r.matches(filter.get())).map(row).collect::<Vec<_>>()
    }
}
```

Status: **landed** (branch `feat/0.1.0-reactivity`).

---

## Migration checklist

- [ ] Bump the git `tag` (or `rev`) to `0.1.0` across `Cargo.toml`.
- [ ] Build. `.get()`-based `if`/`match` conditions and `text { move || … }`
      closures compile unchanged — no action.
- [ ] Fix any **frozen** `if`: a reactive read via a call the `.get()` scan
      couldn't see (`use_focus()`, `use_can_go_back()`) now reacts automatically —
      no action, it just works. A reactive read buried in a structural comparison
      → author with a closure child `{ move || if … }`.
- [ ] Convert any **bare reactive text** the compiler flags (`text { count.get() }`
      → `text { move || count.get() }`). The footgun guard makes every such site a
      loud compile error — no silent freezes to hunt. Value-typed reactive forms
      are unchanged (reactive by type).
- [ ] Watch for **compile errors** where a now-reactive `if`/`text` closure
      captures a non-`Clone`/borrowed value and needs `Clone`/`'static` — loud,
      points at the site.
- [ ] Run `cargo test` and `idealyst lint`; re-run robot/parity checks on
      reactive screens.

See [[reactivity-in-depth]] for the model these changes are built on, and
[[idiomatic-components]] for the component shape they slot into.

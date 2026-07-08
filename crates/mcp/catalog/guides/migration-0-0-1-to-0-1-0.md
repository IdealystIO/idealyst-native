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

## 1. Reactive children & control flow: closures, not `.get()`-scanning

**What changed.** In 0.0.1, `ui!` decided whether an `if`/`match` branch or a
`text(...)`/`button(...)` argument was reactive by **scanning the tokens for
the substring `.get()`**. In 0.1.0 that heuristic is gone. Reactivity is decided
by **type**, matching how `Reactive<T>` props already worked:

- A child that is a **value** (`{ expr }`, any Rust expression returning
  `IntoElement`) is **static** — built once.
- A child that is a **closure** (`{ move || expr }`, `Fn() -> impl IntoElement`)
  is **reactive** — the walker installs an effect and rebuilds when the signals
  the closure reads change.

The `ui!` `if` / `match` sugar still works and reads the same, but it now lowers
through this closure boundary instead of a token scan — so it is correct
regardless of *how* you spell the reactive read.

**Why.** The `.get()` scan was a syntactic proxy for a semantic property and
leaked in both directions. Benignly on false positives (`HashMap::get()` built a
needless effect), and **dangerously on false negatives**: any reactive read not
literally spelled `.get()` was treated as static and **silently frozen**. The
worst offenders were reactive hooks returning `impl Fn() -> bool` —
`use_focus()`, `use_can_go_back()` — whose read is a *call*, not `.get()`
(this is the `use_focus()` "KNOWN ISSUE" some screens hit in 0.0.1). Type-driven
dispatch has no false negatives: if you want it live, it's a closure, and you
can *see* the reactive boundary.

**Migrate.**

Most call sites need **no change** — an `if`/`match` that already reads a
signal via `.get()` compiles and behaves identically:

```rust
// unchanged across 0.0.1 → 0.1.0
ui! {
    if count.get() > 0 {
        text(text_fmt!("{} items", bind!(count)))
    } else {
        text("empty")
    }
}
```

The change bites where a reactive read was **not** spelled `.get()`. Those were
frozen in 0.0.1; in 0.1.0 wrap them in a closure to make the boundary explicit:

```rust
// 0.0.1 — SILENTLY STATIC (bug): use_focus()'s read is a call, no `.get()`
ui! {
    if use_focus()() { ActiveBadge() } else { view {} }
}

// 0.1.0 — reactive: the closure is the boundary
ui! {
    { move || if use_focus()() { ui!{ ActiveBadge() } } else { ui!{ view {} } } }
}
```

For a computed child that reads signals through helpers, indexing, or a
`Ref::with` — anything the old scan would have missed — the rule is uniform:
**wrap it in `move ||` to be live, leave it bare to be static.**

**Decided lowering (for the macro rewrite):**

- `if cond { … }` → **always** `when(move || cond, …)`. A static condition
  (reads no signals) yields an inert effect that runs once and never re-fires —
  so "always reactive" costs ~nothing for static conditions, while reactive ones
  rebuild only on a real flip (`when` dedups on the bool = Leptos `<Show>`). The
  `.get()`-scan gate is deleted; the type-driven `if <Signal<bool>>` bare-path
  dispatch (`__idealyst_if`) stays.
- `match scrut { … }` → **reactive-when-possible**: `switch(move || scrut, …)`
  whenever the scrutinee is re-evaluatable (a `Copy` value, a signal read, a
  call); otherwise it fails to compile and is migrated. `switch` dedups on the
  key.

**Breaking edges to migrate (all compile-time-loud, never silent freezes):**

- **Clone/`'static` tightening.** A previously-static `if`/`match` branch that
  moved or borrowed a non-`Clone` value now sits in a `move ||` closure and needs
  `.clone()` / `'static` captures. (The documented "B5" nested-`else if` move
  constraint becomes uniform; the flat-`match` workaround still applies.)
- **`match` scrutinee requirements.** Reactive `switch` needs `S: PartialEq`, and
  a non-`Copy`-non-`Clone` owned scrutinee (`move || x` = `FnOnce`) won't
  compile. Migrate: derive `PartialEq`/`Copy`, or clone the scrutinee.
- **Multi-node child branches gain a wrapper.** In child position, a static
  `if cond { A B C }` used to flat-splat A/B/C as siblings; the reactive `when`
  wraps a multi-node branch in one node (via `emit_block_as_primitive`), matching
  how reactive branches already behave. Split into separate `if`s or accept the
  wrapper.

Status: **planned** (macro rewrite — the closure boundary in §3 it builds on has
landed).

---

## 2. `bind!` retires in favor of `rx!` / closures

**What changed.** `bind!` — the `text_fmt!`-only sentinel that depended on the
same `.get()` scan — is removed. Its two jobs are subsumed by the type-driven
model: a bare `Signal` prop is already live, and a computed live value is a
closure or `rx!`.

**Why.** With reactivity decided by type, a sentinel that marks "track this
substring" no longer has a job. One fewer special form, one fewer thing to learn.

**Migrate.**

```rust
// 0.0.1
Typography(content = rx!(format!("clicked {}×", count.get())))   // rx! unchanged
Text(text_fmt!("Count: {}", bind!(count)))                       // bind! sentinel

// 0.1.0
Typography(content = rx!(format!("clicked {}×", count.get())))   // still the way
text(move || format!("Count: {}", count.get()))                  // closure is the live form
```

`rx!` stays — it's the ergonomic inline form for a live prop value. Only `bind!`
goes away.

Status: planned.

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
- [ ] Build. `.get()`-based `if`/`match`/`text`/`button` sites compile
      unchanged — no action.
- [ ] Fix any **frozen** UI: a reactive read not spelled `.get()`
      (`use_focus()`, `use_can_go_back()`, helper-wrapped reads) → wrap the
      child in `move ||`.
- [ ] Replace `bind!(sig)` inside `text_fmt!` with a `move ||` closure (or a
      bare live prop). `rx!` is unchanged.
- [ ] Watch for **compile errors** where a previously-static child captured a
      non-`Clone`/borrowed value and now needs `Clone`/`'static` — loud, points
      at the site.
- [ ] Run `cargo test` and `idealyst lint`; re-run robot/parity checks on
      reactive screens.

See [[reactivity-in-depth]] for the model these changes are built on, and
[[idiomatic-components]] for the component shape they slot into.

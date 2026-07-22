+++
title = "Migrating 0.3 → 0.4"
order = 903
tags = ["migration", "0.4.0", "breaking", "reactivity", "macros", "lazy", "code-splitting"]
+++

# Migrating 0.3 → 0.4

> **Status: 0.4.0 is in development.** This is the living record of its breaking
> changes — each section carries a `Status:` line and fills in with concrete
> before/after as the change lands. See [[migrations]] for the versioning
> policy: 0.4.0 is a **clean, in-place break with no legacy shims**.

0.4.0 makes `ui!` / `jsx!` control-flow reactivity **correct by default**. The
pre-0.4.0 gate *guessed* whether an `if` / `match` condition was reactive by
scanning for a top-level call or a literal `.get()` in the tokens. That guess
had a silent failure mode: a signal read buried inside a comparison or negation
(`if items.len() > 0`, `if a.get() < b.get()`, `match model.current()`) was
classified **static** and **silently froze** — the branch built once and never
re-rendered. 0.4.0 replaces the guess with a sound rule: **reactive is the
default; static is the proven-safe optimization.**

Nothing that reads a signal can freeze anymore. The cost is moved onto the
compiler: a handful of genuinely-static conditions that happen to contain a
method call now lower reactively, and where their branch also *moves* a
non-`Copy` value you get a **loud compile error** (never a silent freeze). The
migration is mechanical — the first-party sweep across `idea-ui`, the website,
and every example touched exactly **two conditions in one file**.

## The inverted gate

**What changed.** `emit_if` / `emit_match` no longer assume a condition is
static unless they spot a top-level call or a literal `.get()`. The new
predicate `condition_may_read_signal` asks the opposite question — *can this
condition be proven signal-free?* Any function or method call **anywhere** in
the condition tree (`.get()`, `.len()`, `foo(x)`), or an unrecognized shape,
lowers reactively via `when` / `switch`. Only a provably signal-free condition —
a bare path/field (routed through the type-driven `StaticCond` / `ReactiveCond`
dispatch), or a literal / `&&` / `||` / `!` / comparison of **call-free**
operands — stays a plain static `if` / `match` with borrowed captures.

**Why.** The pre-0.4.0 gate was a *correctness* heuristic: guess wrong and the
UI silently freezes. A `.get()` spelled at the condition surface (`if x.get() >
0`) was caught, but the same read behind a method (`if v.get().len() > 0` is
fine; `if v.len() > 0` where `v` is derived from a signal, or `match
model.current()` where `current()` reads a signal) fell through to static and
froze. It also produced an `if`/`match` asymmetry — `if x.method()` was reactive
(top-level call) but `match x.method()` was not (method-call scrutinees weren't
recognized). Reactivity as the default eliminates the entire class of silent
freezes and erases the asymmetry: the framework's `Effect` subscribes to
whatever the closure actually reads, so a real read is *never* dropped. A
genuinely-static condition treated reactive costs only an inert effect that runs
once and never re-fires — correct and ~free.

**Migrate.** Most code needs no change: conditions with a visible `.get()`, over
`Copy` signals, or as bare paths/fields behave identically. The break surfaces
only where a condition **contains a call** *and* a branch **moves a non-`Copy`
value** the condition also references — the condition and branch now each capture
by `move`, so the value can't go to both. Two fixes:

*If the condition is genuinely static* (a presence/length check over a plain,
non-signal value), hoist the predicate to a `let bool` above the `ui!` block. A
bare-path condition dispatches to the static `StaticCond` path with borrowed
captures:

```rust
// before — 0.3: static plain `if`, borrowed capture of `eyebrow`
ui! {
    view {
        if !eyebrow.is_empty() {          // 0.4: now reactive → moves `eyebrow`
            text { eyebrow }              //      into BOTH the cond and branch closures
        }
    }
}

// after — hoist the predicate; `if has_eyebrow` is a bare-path bool → static
let has_eyebrow = !eyebrow.is_empty();
ui! {
    view {
        if has_eyebrow {
            text { eyebrow }              // borrowed capture, compiles
        }
    }
}
```

*If the condition is genuinely reactive* (the call reads a signal), keep it
reactive and `.clone()` the value the branch needs, or restructure to `if let`:

```rust
// reactive condition, non-Copy value needed in the branch → clone it
if list.get().contains(&needle) {
    text { needle.clone() }
}
```

To force a call-containing condition back to a borrowed-capture static `if`
against your intent, read the signal **inline** via `.get_untracked()` — the
framework's intentional-static marker is honored directly in the condition
(`if sig.get_untracked() < 3` stays static, no `move` capture). The `let bool`
hoist is the other option and reads clearer for a plain presence check.

**Status:** landed. `emit_if` / `emit_match` route through
`condition_may_read_signal`; regression coverage in
`crates/runtime/core/tests/if_reactive_lowering.rs`
(`comparison_with_buried_signal_read_is_reactive`,
`compound_get_comparison_is_reactive`) and
`match_reactive_call_regression.rs` (`match_method_call_scrutinee_is_reactive`),
plus the dedup-preserved and static-stays-static assertions in the same files.

## What did NOT change

- **`when` / `switch` dedup is intact.** A reactive `if` still rebuilds only when
  the bool *value* changes; a reactive `match` only when the discriminant *key*
  changes. 0.4.0 broadens *which* conditions are reactive — it does **not** swap
  `when`/`switch` for a coarser rebuild-on-any-change node, so there is no
  over-rebuild regression.
- **Conditions with a visible `.get()`** (`if x.get() > 0`, `match screen.get()`)
  are reactive exactly as before.
- **Bare-path/field conditions** (`if flag`, `if state.open`) still dispatch by
  type: `bool` → static, `Signal<bool>` / `Derived<bool>` (e.g. `memo(...)`) →
  reactive.
- **`if let PAT = EXPR`** is unchanged — always static (author a reactive form as
  `match sig.get() { … }`).

## Lazy loading: component surface + a fallible loader

0.4.0 promotes code-splitting from an inline-block-only tool to a
**component-level** one, and gives a lazy load real, observable states. See the
[[lazy-loading]] guide for the full surface; the migration-relevant deltas:

**New (additive).** A component can be split into its own wasm chunk with
`#[component(lazy)]` (or the `#[lazy_component]` shorthand). Its **props become
the args** that cross the split, so runtime input no longer forces you off the
blessed API into the `#[wasm_split]` internals. The generated props gain
`loading` / `error` config fields, and the load exposes three states —
Loading → Ready | Error — with a `LazyError` (`.message()` + `.retry()`). Retry
opts in via `#[component(lazy, retryable)]` (which derives `Clone` on the
props). Eager state a chunk builds in a constructor is now **owned by the chunk
scope**, not leaked — the pre-0.4.0 "signal created outside any reactive scope"
trap on lazy bodies is closed.

**Breaking (narrow).** The loader's output type changed:

```rust
// 0.3
pub type LazyFuture = Pin<Box<dyn Future<Output = Element>>>;
// 0.4 — the load can fail (web fetch / dynamic-link), surfaced to `.on_error(..)`
pub type LazyFuture = Pin<Box<dyn Future<Output = Result<Element, String>>>>;
```

The `lazy! { … }` macro handles this for you (it wraps the block's `Element` in
`Ok`). You only touch it if you **hand-roll a loader** — a bare `lazy_split(||
Box::pin(async { … }))`, an `install_dynlink_loader` bridge, or a direct
`Element::Lazy { … }` construction:

```rust
// before
lazy_split(|| Box::pin(async { my_chunk().await.into_element() }))
// after — wrap the success value; return `Err(msg)` to drive the error UI
lazy_split(|| Box::pin(async { Ok(my_chunk().await.into_element()) }))
```

`Element::Lazy` also gained an `error` field; an exhaustive `match` on `Element`
that names `Lazy { … }` needs the new field (or `..`).

**Status:** landed. Coverage in `crates/runtime/core/tests/walker/lazy.rs`
(`error_ui_renders_on_load_failure`, `retry_reloads_after_error`, and the
`lazy_component_*` macro tests) and `primitives::lazy` / `walker::lazy` unit
tests.

## Migration checklist

- [ ] Build. Fix any `E0507` ("cannot move out of value, a captured variable in
      an `Fn` closure") / `E0382` ("use of moved value") at an `if` / `match`
      whose condition contains a call.
- [ ] If you **hand-roll a lazy loader** (not the `lazy!` macro), wrap its
      success value in `Ok(...)`; the loader now returns `Result<Element,
      String>`. Add `..` to any exhaustive `match` on `Element::Lazy { … }`.
- [ ] For each: is the condition **static**? → hoist it to a `let bool` above the
      `ui!` block. Is it **reactive**? → `.clone()` the non-`Copy` value the
      branch needs, or restructure to `if let`.
- [ ] Spot-check any control flow that *should* be reactive but looked frozen
      before (`match x.method()`, `if a.len() < b.len()`) — it now updates.
- [ ] Update the git tag in your `Cargo.toml` dependency lines to `0.4.0`.

---
name: idealyst-reactivity
description: The deeper reactivity surface in THIS repo — memo/rx!, batch/untrack, reducer/resource, provide/inject context, when/switch/fragment, watch/Subscription, on_cleanup, and the sharp edges (stale-set no-op, Ref::with borrow abort, no set-in-memo, dispose-on-hide). Use when the plain signal!/effect! pair isn't enough, when debugging "my signal stopped updating" / "RefCell already borrowed" / a frozen wasm tab, or when reaching for derived state, context, or reactive control flow. Complements the idealyst-components skill.
---

# Reactivity in depth

The everyday surface — `signal!`, `effect!`, `watch` — is covered by the
[idealyst-components](../idealyst-components/SKILL.md) skill and the
[[reactivity]] MCP guide. **This** skill is the layer beneath: derived state,
tracking control, reactive control flow, and the edges you hit only sometimes.

Authoritative long-form reference (kept in sync, also served by the MCP):
[`crates/mcp/catalog/guides/reactivity-in-depth.md`](../../../crates/mcp/catalog/guides/reactivity-in-depth.md).
Ground truth is the source below — read it before guessing at semantics.

## Ground-truth source files

| Surface | File |
| --- | --- |
| `memo`/`memo_with`, `reducer`, `provide`/`inject`, `on_cleanup`, `untrack`, `batch`, `watch`/`Subscription`, `Signal` methods | `crates/runtime/core/src/reactive.rs` |
| `when`, `switch`, `fragment` constructors | `crates/runtime/core/src/builder.rs` |
| `rx!`, `signal!`, `effect!`, `memo!`, `bind!`, `children!`, `node_ref!` macros | `crates/runtime/core/src/lib.rs` |
| `derived(...)` style-variant source (NOT the reactive one) | `crates/runtime/core/src/style.rs` |
| `resource(...)` async state | `crates/runtime/core/src/resource.rs` |
| `Ref` generational-slot semantics + the `with` borrow rule | `crates/runtime/reactive/refs/src/lib.rs` |

## Reach-for table

| You need… | Use | Notes |
| --- | --- | --- |
| Derived value read in many places / expensive | `memo!(expr)` → `Signal<T>` | change-gated (`PartialEq`); `memo_with(eq,f)` for no-`PartialEq`. Body must be pure — **no `.set()` inside** (panics). |
| Inline live prop value from a computed expr | `rx!(expr)` | = `Reactive::derive(move \|\| expr)`. Bare `Signal` prop is already live. |
| Read a signal without subscribing | `untrack(\|\| s.get())` | |
| Coalesce several writes into one fan-out | `batch(\|\| { … })` | Handlers are **born batched** already — rarely needed by hand. |
| Action-dispatched state | `reducer(init, \|&s,a\| next)` → `(Signal, dispatch)` | dispatch = one cycle, untracked read. |
| Async data keyed on signals | `resource(deps, fetcher)` → `Resource<T,E>` | `Loading/Error/Success/Idle`. |
| Pass a value down the scope tree | `provide(v)` / `inject::<T>()` / `inject_or` / `with_inject` | keyed by TYPE — newtype to disambiguate; panics outside a scope. |
| Two-way reactive subtree | `when(cond, then, otherwise)` | dispose-on-hide. |
| Multi-way reactive subtree | `switch(scrutinee, \|&k\| …)` | `ui!` `match` lowers to this; key is `PartialEq`. |
| Several siblings from one `#[component]` | `fragment(children)` | layout-transparent, built once, not reconciled. |
| Reactivity OUTSIDE the tree | `watch(f)` → `Subscription` | `#[must_use]`; store it or `.leak()`. |
| Teardown | `on_cleanup(f)` | effect: before re-run + on disposal; scope: on drop. |

## Model: dispose-on-hide

`when` / `switch` / a reactive `for` rebuild the active branch from scratch and
**drop** the hidden branch's effects and subscriptions. State in a branch that
goes away is **lost** — hoist state that must survive a toggle into the parent
scope.

## Sharp edges (recognize these in the wild)

- **"My signal stopped updating"** → usually a raw `Signal::new`/`Effect::new`
  where the `signal!`/`effect!` macro (scope-anchored) was needed, or a stale
  handle. `idealyst lint` (`prefer-signal-macro`/`prefer-effect-macro`) flags it.
- **Stale `set` is a safe no-op** — generational handles mean a `.set()` after
  the owning scope tore down does nothing (not a panic). Deferred/async
  callbacks after unmount are safe.
- **"RefCell already borrowed" abort** → you called `.set()` inside a
  `Ref::with` / `handle.with` closure, which holds the arena borrow. Read the
  value out, close the `with`, then `set`. The `is_reactive_busy` guard does NOT
  catch this.
- **Panic inside a `memo!`** → you wrote `.set()`/`.update()` in the compute
  body. Memos are pure derivations.
- **A mutual write-loop panics (depth 256), not hangs** — look for A→B→A.
- **Frozen wasm tab** → a future self-rewaking (`wake_by_ref` + `Pending`) spins
  the single-thread event loop. Park the task; wake from the event.
- **`bind!` only works inside `text_fmt!`** — anywhere else is a compile error;
  use `rx!` for a live value elsewhere.
- **`.get()` outside a reactive context** is a one-shot read that subscribes
  nothing.

## Keep docs aligned (CLAUDE.md §2)

If you change reactive semantics, update
`crates/mcp/catalog/guides/reactivity-in-depth.md` (and `reactivity.md` for the
surface) in the same change, and add/extend tests under
`crates/runtime/core/tests/reactive/` (CLAUDE.md §1, §8).

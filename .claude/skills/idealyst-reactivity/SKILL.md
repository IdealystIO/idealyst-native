---
name: idealyst-reactivity
description: The deeper reactivity surface in THIS repo — memo/rx!, untrack, staged writes + flush, reducer/resource, provide/inject context, when/switch/fragment, watch/Subscription, on_cleanup, and the sharp edges (stale-handle write panic, Ref::with borrow abort, no set-in-memo, dispose-on-hide). Use when the plain signal()/effect! pair isn't enough, when debugging "my signal stopped updating" / "RefCell already borrowed" / a frozen wasm tab, or when reaching for derived state, context, or reactive control flow. Complements the idealyst-components skill.
---

# Reactivity in depth

The everyday surface — the `signal()` function, `effect!`, `watch` — is covered by the
[idealyst-components](../idealyst-components/SKILL.md) skill and the
[[reactivity]] MCP guide. **This** skill is the layer beneath: derived state,
tracking control, reactive control flow, and the edges you hit only sometimes.

Authoritative long-form reference (kept in sync, also served by the MCP):
[`crates/mcp/catalog/guides/reactivity-in-depth.md`](../../../crates/mcp/catalog/guides/reactivity-in-depth.md).
Ground truth is the source below — read it before guessing at semantics.

## Ground-truth source files

Everything is re-exported under `runtime_core::…`, but `runtime-core` is now a
77-line root over `runtime_vocabulary::glue` — the implementations live here:

| Surface | File |
| --- | --- |
| The kernel: `signal()`, `effect`, `memo`, `untrack`, `on_cleanup`, `provide`/`inject`, `Signal` methods, staged writes + `World::flush` | `crates/runtime/world/src/lib.rs` |
| `memo_with`, `reducer`, `watch`/`Subscription`, `when`/`switch` constructors, the whole author surface | `crates/runtime/vocabulary/src/glue.rs` |
| `resource(...)` / `mutation(...)` async state | `crates/runtime/vocabulary/src/async_reactive.rs` |
| `fragment(children)` | `crates/runtime/scene/src/element.rs` |
| `derived(...)` style-variant source (NOT the reactive one) | `crates/runtime/shared/src/style.rs` |
| `Ref` generational-slot semantics + the `with` borrow rule (a shared-substrate arena, deliberately outside the world kernel) | `crates/runtime/shared/src/reactive.rs` |
| `rx!`, `effect!`, `node_ref!` macros (the `signal!` and `memo!` macros were REMOVED — plain fns now) | `crates/runtime/core/src/lib.rs` (re-export), bodies in `runtime-vocabulary` / `runtime-shared` |

## Reach-for table

| You need… | Use | Notes |
| --- | --- | --- |
| Derived value read in many places / expensive | `memo(move \|\| expr)` → `ReadSignal<T>` | change-gated (`PartialEq`); `memo_with(eq,f)` for no-`PartialEq`. Body must be pure — **no `.set()` inside** (panics); output is read-only by type. |
| Prove a prop/handle only reads (or only writes) | `.read_only()` / `.write_only()` / `.split()` | zero-cost newtypes over the same slot; `ReadSignal` props for observe-only, `WriteSignal` for report-up. Unified `Signal` stays right for two-way (`TextInput.value`). |
| Inline live prop value from a computed expr | `rx!(expr)` | = `Reactive::derive(move \|\| expr)`. Bare `Signal` prop is already live. |
| Read a signal without subscribing | `untrack(\|\| s.get())` | |
| Coalesce several writes into one fan-out | nothing — it's automatic | Writes **stage** and commit in one `World::flush` at the end of the turn. There is no `batch(…)` on the author surface. |
| Action-dispatched state | `reducer(init, \|&s,a\| next)` → `(Signal, dispatch)` | folds on the staged value (dispatches compose); always notifies; never subscribes the caller. |
| Async data keyed on signals | `resource(deps, fetcher)` → `Resource<T,E>` | `Loading/Error/Success/Idle`. |
| Pass a value down the scope tree | `provide(v)` / `inject::<T>()` / `inject_or` / `with_inject` | keyed by TYPE — newtype to disambiguate; panics outside a scope. The entry is **owned by the providing scope** (retracted on its drop), like a signal. World-lifetime service ⇒ `unscoped(\|\| provide(v))`; bounded region ⇒ wrap the `provide` in its own `collect_owned`. |
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

- **"My signal stopped updating"** → usually a raw `Effect::new` where the
  scope-owned `effect!` was needed, or a stale handle. (Signal creation is the
  plain `signal(value)` fn; scope anchoring happens inside the constructor, so
  `Signal::new` is merely redundant, not broken.) `idealyst lint`
  (`prefer-signal-fn`/`prefer-effect-macro`) flags both spellings.
- **`set` is equality-guarded** — `.set(v)` (`T: PartialEq`) skips the fan-out
  when the value is unchanged. A same-value write used as a retrigger must be
  explicit: `touch()` (notify, no write), `set_always(v)` (write + always
  notify; also the only setter for non-`PartialEq` `T`), `set_untracked(v)`
  (write, never notify). `update(|&cur| next)` is guarded the same way and
  composes on the staged value. Diverges from Leptos.
- **Writes are staged; reads see the committed value** — `s.set(v)` then
  `s.get()` in the same handler still returns the old value. The world commits
  and fans out once, at the flush that closes the turn. `update` is the
  exception: its closure sees the staged value. Debug builds warn on the
  stale read (`idealyst[staged-read]`, naming the read site, the signal's
  creation site and the `update` fix; once per call site, never a panic).
  Reads that subscribe the running effect are exempt — they re-run when the
  write commits — so what it reports is handlers, component bodies, `peek` /
  `with_untracked` and cross-world reads.
- **A write after the world dropped is a no-op; a stale handle in a LIVE world
  panics** — an async callback completing after unmount of its world is
  harmless, but writing through a recycled slot's handle while the world is
  alive is a use-after-unmount error and says so.
- **"RefCell already borrowed" abort** → you called `.set()` inside a
  `Ref::with` / `handle.with` closure, which holds the arena borrow. Read the
  value out, close the `with`, then `set`. The `is_reactive_busy` guard does NOT
  catch this.
- **Panic inside a `memo`** → you wrote `.set()`/`.update()` in the compute
  body. Memos are pure derivations.
- **A mutual write-loop panics (flush round limit 100), not hangs** — look for
  A→B→A. Re-entering `flush` on the same world from inside its own effect also
  panics; writes staged during a flush land in that flush's next round.
- **Frozen wasm tab** → a future self-rewaking (`wake_by_ref` + `Pending`) spins
  the single-thread event loop. Park the task; wake from the event.
- **`text_fmt!` and `bind!` were REMOVED in 0.3** — reactive text is an
  f-string literal: `text { "count: {count}" }` — a signal slot is live by
  TYPE (a `Display` value bakes in), no closure/`.get()`/sentinel needed, and
  signal slots get the web JS-binding fast path automatically. Positional or
  Debug formatting → `text { move || format!(…) }`; live prop values → `rx!`.
  The `prefer-text-fstring` lint flags leftovers.
- **`.get()` outside a reactive context** is a one-shot read that subscribes
  nothing. Commonest form is the **hoisted-snapshot trap**: `let ok =
  x.get()…;` at component-body level then `if ok { … }` in `ui!` — `ok` is a
  frozen `bool`, the branch is static, silently. Keep derivations behind
  `move ||` (`memo(move || …)`) or inline the read into the condition — under
  the 0.4.0 inverted gate any `if`/`match` condition that might read a signal (a
  `.get()` or any call anywhere) lowers to `when`/`switch`; only a provably
  signal-free condition stays static. "A `let` freezes, a closure — or a call
  in the condition — flows." GUARDED: the `snapshot-condition` lint flags it
  (ambient in `idealyst dev`), and on a `Reactive<T>` prop `.get_untracked()`
  declares an intentional snapshot (which the lint accepts by name).

## Keep docs aligned (CLAUDE.md §2)

If you change reactive semantics, update
`crates/mcp/catalog/guides/reactivity-in-depth.md` (and `reactivity.md` for the
surface) in the same change, and add/extend tests in
`crates/runtime/world/src/tests.rs` (kernel semantics) or
`crates/runtime/vocabulary/tests/glue_reactive_surface.rs` /
`async_reactive.rs` (author surface) (CLAUDE.md §1, §8).

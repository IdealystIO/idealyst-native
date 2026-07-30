# Migrating from 0.5.x to 1.0

This is the front door for upgrading an app from **0.5.2** to **1.0.0**.

1.0 replaces the rendering core. The reactive system, the element model,
the backend seam and the crate layout all changed. Most app code does
**not** change — `ui!`, `#[component]`, `stylesheet!`, the primitives and
the component library keep their spelling — but the reactive write
semantics changed in a way that can alter behaviour *silently*, so read
[Reactive semantics](#reactive-semantics-read-this-one) even if
everything compiles.

## How to use this document

- **[Breaking changes](#breaking-changes)** — the full inventory, ordered
  by how likely you are to hit it.
- **[Upgrade procedure](#upgrade-procedure)** — the order to work in.
- **[`migrating-to-runtime-v2.md`](migrating-to-runtime-v2.md)** — the
  deep chapter. Same content, more detail: staged writes, boot, handler
  context, teardown, crate layout, testing. This page links into it
  rather than repeating it.

Earlier upgrades: [0.1 → 0.2](migration-0.1-to-0.2.md) (navigation moved
to the outlet model), [0.2 → 0.3](migration-0.2-to-0.3.md) (reactive
surface unification). If you are further back than 0.5, work through
those first — this guide assumes a 0.5.x starting point.

## What did NOT change

Worth stating, because the diff is large and it is easy to assume the
worst:

- **The wire protocol.** `PROTOCOL_VERSION` is `17` on both 0.5.2 and
  1.0 — 742 wire declarations, byte-identical. Dev clients and apps do
  not need a lockstep upgrade.
- **Navigation.** The tab/drawer → outlet migration happened in 0.1 →
  0.2. `stack-navigator` and `swap-navigator` in 0.5.2 carry forward.
- **Styled text runs**, `TextRun` / `TextRunStyle` — present in 0.5.2,
  unchanged.
- **`ui!` / `jsx!` / `#[component]` / `stylesheet!`** — same syntax, same
  primitive names, same props.
- **`runtime_core::…` import paths.** 377 of the 396 names exported from
  the 0.5.2 author surface still resolve unchanged. The 19 that do not
  are listed below in full.

## Reactive semantics (read this one)

The single change most likely to alter behaviour without a compile
error: **`set` stages the write; reads see the previous value until the
driver flushes.**

```rust
// 0.5.x: count is 2 afterwards.
// 1.0:   count is 1 — the second get() still sees the committed value.
count.set(count.get() + 1);
count.set(count.get() + 1);
```

Use `update` for read-modify-write:

```rust
count.update(|v: &i32| v + 1);   // composes on the staged value
```

Note `update`'s closure shape also changed: it takes `&T` and *returns*
the new value, where 0.5.x took `&mut T`. That one is a compile error,
so the compiler will find it for you.

Full treatment — including why batching disappeared and what the flush
boundary guarantees — is in
[`migrating-to-runtime-v2.md`](migrating-to-runtime-v2.md#reactive-semantics-writes-are-staged).

## Breaking changes

### 1. Reactive surface

| 0.5.x | 1.0 | How it fails |
| --- | --- | --- |
| `sig.set(v)` visible to the next `get()` | write **stages** until flush | **silent** — stale read-back |
| `sig.set(get()+1)` twice → +2 | → **+1** | **silent** — lost increment |
| `sig.update(\|v: &mut T\| …)` | `sig.update(\|v: &T\| -> T)` | compile error |
| `batch(\|\| …)` | removed — each handler turn is one batch | compile error |
| `sig.get_untracked()` | `sig.peek()` | compile error |
| `sig.update_if_changed(…)` | removed (guarded `set` subsumes it) | compile error |
| `Signal::new(v)` | the free `signal(v)` | compile error |
| `signal(v)` for any `T: Clone` | **`T: PartialEq`** bounds the whole handle | compile error |
| `on_cleanup(f)` in a component body | return the cleanup from an effect | **runtime panic** at build |
| `signal`/`effect`/`memo` inside an event handler | handlers run outside the world | **runtime panic** on the event |

The `PartialEq` bound is the one that surprises people: it applies to
creation and to `get`, not just to `set`. For a payload with no
meaningful value equality (a connection handle, an `Rc<dyn Any>` slot),
give it a pointer-identity impl comparing `Rc::ptr_eq` — in-tree
examples are `idea-theme`'s `ThemeSlot` and `server::SocketSender`.

### 2. Removed from the `runtime_core` author surface

Nineteen names. This list is complete — it was derived mechanically by
diffing the exported surface at the `0.5.2` tag against the current
tree, not written from memory.

**External-SDK authoring.** The per-backend External table became the
scene `Registry`, so an SDK registers a typed payload + handler instead
of implementing a backend-bound trait.

| Removed | Replacement |
| --- | --- |
| `ExternalRegistry<B: Backend>` | `runtime_scene::Registry<H>` |
| `RegisterExternal` (trait) | a plain `fn register<H: Host + …caps>(registry: &mut Registry<H>)` |
| `ExternalHandle<P>` | the SDK's own bound type over its payload (e.g. `canvas_core::CanvasBound`) |
| `ErasedHandler` | internal to `Registry` |
| `drain_external_registrations`, `has_pending_external_registrations`, `defer_external_registration` | `registry.defer::<P>()` + `registry.register_deferred::<P, _>(h)` + `runtime_scene::defer_registration` |

**Payload wire-serde — moved, not removed.** Same names, same
semantics, new home: `wire::payload_serde`.

| Removed from `runtime_core` | Now at |
| --- | --- |
| `register_external_serde` | `wire::register_external_serde` |
| `serialize_external_payload` | `wire::serialize_external_payload` |
| `deserialize_external_payload` | `wire::deserialize_external_payload` |

**Custom navigator authoring.** Not the 0.1→0.2 tab/drawer change — this
is the lower-level seam for writing your *own* navigator. Navigation is
now a vocabulary builtin; use `swap-navigator` / `stack-navigator`.

`NavigatorConfig` · `NavigatorHandler` · `NavigatorRegistry`

**Scopes and detached building.**

| Removed | Replacement |
| --- | --- |
| `Owner` | the world's `Owned` collector; a realized subtree owns its reactive state and frees it on drop |
| `DetachedScope`, `build_detached` | `runtime_scene::realize_detached` |

**Miscellaneous.**

| Removed | Note |
| --- | --- |
| `EachSnapshot` (`Box<dyn Fn() -> Vec<(EachKey, EachRowBuild)>>`) | old keyed-list plumbing; `for … , key = …` in `ui!` is unchanged for authors |
| `ReactiveListKeyed` | was a compile-time diagnostic marker, never a callable API |
| `IntoDisabledSource` | `.disabled(…)` now takes `impl IntoValue<bool>` — `true`, a signal, or a closure all still work |

### 3. CLI and build

| 0.5.x | 1.0 |
| --- | --- |
| `idealyst build --web --primitives=<list>` | removed — hard error naming the migration guide. The `prim-*` feature families are gone; per-primitive registration in `handlers::register_builtins` replaces them |
| `prim-*` cargo features on `runtime-core` / backends | removed |

### 4. Crate layout

`runtime-core` is now a thin author facade re-exporting
`runtime_vocabulary::glue`. The substrate it used to contain lives in
four crates: `runtime-shared` (style, assets, animation, scheduling),
`runtime-world` (reactive kernel), `runtime-scene` (element model,
realize, `Host`, `Registry`), `runtime-vocabulary` (capability traits,
builtin handlers).

**Apps do not need to change their dependency line** — keep depending on
`runtime-core` and keep writing `runtime_core::…`. This matters only if
you write a backend or an SDK, which should depend on the specific crate
it needs. See
[Crate layout](migrating-to-runtime-v2.md#crate-layout-runtime-shared-the-split-substrate).

## Upgrade procedure

1. **Bump the dependency** to 1.0 and build. Do not fix anything yet —
   collect the whole error list first.
2. **Work the compile errors**, which cluster in a predictable order:
   `Signal::new` → `signal()`, `get_untracked` → `peek`, `update`'s
   closure shape, `batch` removal, then `PartialEq` on your signal
   payload types (derive it; use pointer identity where value equality
   is meaningless).
3. **Fix the panics the compiler cannot see.** Grep your own code for
   two shapes:
   - `on_cleanup(` inside a component body → return the cleanup from an
     effect instead;
   - `signal(` / `effect!` / `memo(` / free theme functions inside an
     event handler → capture what you need at build time.
   These are runtime panics on first interaction, not build failures.
4. **Audit read-after-write.** The staged-write change is silent. Search
   for a `.set(` followed by a `.get()` of the same signal in one
   handler — that is the shape that changes meaning.
5. **If you ship an External SDK**, port registration to the scene
   `Registry` (section 2 above).
6. **Run it.** The staged-write and handler-context changes surface
   through behaviour, not tests. Exercise the interactive paths.

## A note on animation

If you drive animation from a custom raf loop and see the wire flooding
with updates while nothing moves, check that a monotonic clock is
installed for your host — `now_micros()` reads `0` with no time source
registered, so every tween resolves against t=0 and re-emits its start
value forever. Every shipped backend boot installs one; a custom host
must too.

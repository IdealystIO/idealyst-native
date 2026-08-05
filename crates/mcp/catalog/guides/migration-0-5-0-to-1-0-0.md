+++
title = "Migrating 0.5 → 1.0"
order = 905
tags = ["migration", "1.0.0", "breaking", "reactivity", "staged-writes", "registry", "crate-layout"]
+++

# Migrating 0.5 → 1.0

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
- **`docs/migrating-to-runtime-v2.md`** in the repository — the deep
  chapter. Same content, more detail: staged writes, boot, handler
  context, teardown, crate layout, testing. This page links into it
  rather than repeating it.

Previous jump: [[migration-0-4-0-to-0-5-0]]. Further back than 0.5, work
through the earlier guides in sequence first — see [[migrations]] for the
chain and the versioning policy. This guide assumes a 0.5.x starting
point.

**1.0.1** is the current patch. It restores the `runtime_core::log_*`
macro re-exports, gives `MediaStream` a pointer-identity `PartialEq` so a
stream can live in a signal, exposes `wrap` / `code_mode` on
`GlueTextArea`, and adds `canvas_vello::register_from_chunk`. Nothing in
it requires migration steps of its own.

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
boundary guarantees — is in `docs/migrating-to-runtime-v2.md`, section
"Reactive semantics: writes are staged". [[reactivity-in-depth]] covers
the resulting model as it stands today.

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
creation and to `get`, not just to `set`. Three answers, in order of
preference:

1. **Derive it** — the normal case for enums, DTOs, view models.
2. **Write a pointer-identity impl** when the payload has no meaningful
   value equality (a connection handle, an `Rc<dyn Any>` slot): compare
   `Rc::ptr_eq` on an `Rc` the type already holds. "Is this the same
   instance?" is exactly the question the guarded `set` asks. In-tree
   examples are `idea-theme`'s `ThemeSlot` and `server::SocketSender`.
3. **Wrap it in `runtime_core::ByIdentity<T>`** when the type is not
   yours — the orphan rule blocks the impl from your crate just as it
   would from ours. It is an `Rc<T>` comparing by `Rc::ptr_eq`,
   `Clone`ing by sharing, and `Deref`ing to `T`, so it disappears at use
   sites:

   ```rust
   let session = signal(ByIdentity::new(third_party::Session::open()?));
   session.with(|s| s.ping()); // Deref — no unwrapping
   ```

   `ByIdentityArc<T>` is the `Arc` sibling for a pointer you were handed
   rather than allocated (`storage::platform_storage()`); wrapping an
   existing `Arc` in `ByIdentity` would compare the new `Rc` instead and
   lose the identity you wanted. Both are `?Sized`-tolerant, so
   `ByIdentityArc<dyn Storage>` works.

Note `set_always` is **not** an escape hatch here: a type with no
`PartialEq` cannot be put in a signal at all, so there is nothing to call
`set_always` on. Fix the payload type, don't change the setter.

Framework types already carry the impl — `MediaStream`, `AudioStream`,
`net::Client` / `CancelHandle` / `CancelToken`, `NavHandle` (and
`SwapHandle` / `StackHandle`), `sync::SyncEngine` / `SyncHandle`,
`graphql::GraphqlClient`, `offload::Handle`, and the SDK node handles
(`FormHandle`, `SvgHandle`, `VideoHandle`, `WebViewHandle`,
`ToolbarHandle`, `MarkdownHandle`). You should
never need `ByIdentity` for something the framework ships; if you do,
that is a framework bug, since only the framework can supply the impl.

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
now a vocabulary builtin; use `swap-navigator` / `stack-navigator` (see
[[navigation]]).

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

[[sdks]] carries the current statement of this ("There are no `prim-*`
features"), and [[migration-0-4-0-to-0-5-0]] keeps the historical
contract those features came from.

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
it needs. See `docs/migrating-to-runtime-v2.md`, section "Crate layout:
runtime-shared, the split substrate".

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

## Migration checklist

- [ ] Dependency tag bumped to `1.0.1`; full error list collected before
      fixing anything.
- [ ] `Signal::new` → `signal()`, `get_untracked` → `peek`, `batch`
      removed, `update_if_changed` removed.
- [ ] `update` closures take `&T` and return the new value.
- [ ] Every signal payload type implements `PartialEq` (pointer identity
      where value equality is meaningless; `ByIdentity<T>` /
      `ByIdentityArc<T>` for a type you do not own).
- [ ] No `on_cleanup(` in a component body.
- [ ] No `signal` / `effect!` / `memo` / free theme calls inside an event
      handler.
- [ ] Read-after-write audited: no `.set(` followed by `.get()` of the
      same signal in one handler.
- [ ] External SDKs registered through `runtime_scene::Registry`; wire
      serde imported from `wire::payload_serde`.
- [ ] `--primitives=` / `prim-*` features removed from build scripts and
      manifests.
- [ ] Interactive paths exercised on a real backend.

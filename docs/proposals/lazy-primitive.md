# Proposal: the `lazy` primitive — wasm code splitting for idealyst web

Web idealyst apps ship as one wasm bundle. Every capability impl, every
primitive's mount handler, every SDK's handler is reachable through
`call_indirect` and impossible for `wasm-opt` to strip. A heavy feature used
in one corner — a GPU preview, a canvas editor, a PDF viewer, a map —
inflates the bundle every user downloads, even the ones who never open it.

The `lazy` primitive lets authors declare boundaries in the UI tree that
compile into separate wasm chunks, loaded on demand. On native platforms it's
a no-op: the body compiles inline and mounts synchronously.

> **Status: implemented, by a different mechanism than this proposal
> sketched.** The proposal made each chunk a separate *crate* — a second
> idealyst app with its own `WebBackend`, its own reactive arena, a
> `chunks!()`-generated `ChunkId` table, and props serialized across a
> module boundary. What shipped splits **one** compiled module after the
> fact with the framework's `wasm-split` pass
> (`crates/tools/wasm-split/`): a chunk is a slice of the same program, so it
> shares the main module's heap, statics, reactive world, and backend, and
> props cross as ordinary Rust values — none of the serialization or
> cross-boundary-state machinery the proposal designed was needed.
>
> - **Component-level laziness is the primary surface.** `#[component(lazy)]`
>   / `#[lazy_component]` split a component's body into a chunk and make its
>   props the data that crosses the boundary. `lazy! { … }` splits an
>   anonymous, input-free block.
> - **Three states.** The loader yields `Result<Element, String>`; the mount
>   handler drives a Loading → Ready | Error machine. The `loading` / `error`
>   props render the first and last; `LazyError` carries `.message()` and a
>   `.retry()` that re-drives the load (`#[component(lazy, retryable)]`).
> - **Eager construction is owned.** The chunk's loader runs under the
>   chunk's reactive scope, so state a component or SDK allocates while
>   building is owned by the chunk and torn down with it.
>
> The primitive's types live in `crates/runtime/shared/src/primitives/lazy.rs`;
> the mount handler in `crates/runtime/vocabulary/src/handlers/lazy.rs`. The
> author-facing reference is the `lazy-loading` MCP guide
> (`crates/mcp/catalog/guides/lazy-loading.md`). The regression apps under
> `tests/lazy-*` guard the split (see [`tests/README.md`](../../tests/README.md)).

---

## Motivation

A marketing site with one embedded GPU preview (the device simulator on
`/`) was ~13 MB uncompressed, and the preview's dependencies — the GPU
engine, two device painters, the embedded app — accounted for ~10 MB of it.
The preview is a visual asset shown on one route. With code splitting, the
main bundle carries the site and the preview loads in the background without
blocking initial paint.

This generalizes:

- an admin route loaded only when the user navigates to `/admin`,
- a rich text editor loaded when the user opens "compose",
- a map view loaded when the "map" tab is selected,
- heavy data viz loaded when a dashboard widget mounts.

---

## Lazy registration of third-party handlers

Splitting the *render* isn't enough when the heavy code is a third-party
primitive. A handler registered on the scene `Registry` at boot is statically
referenced from the main module, so wasm-split's reachability analysis keeps
the whole SDK in `main.wasm`; wrapping the usage site in a lazy boundary does
nothing, because registration, not rendering, is the anchor.

The fix is to register the handler from **inside** the chunk:

```rust
// 1. Boot seam (`register_scene_extensions` / the boot entry's `register`):
//    declare the payload kind late-bound. Costs main a TypeId, nothing else.
registry.defer::<HeavyProps>();

// 2. Inside the `#[component(lazy)]` body — no registry in hand, so queue:
runtime_scene::defer_registration::<backend_web::WebBackend, _>(|registry| {
    // 3. …which lands here, installing the handler and realizing every
    //    item of this kind that realize already parked.
    registry.register_deferred::<HeavyProps, _>(mount_heavy);
});
```

Realize drains the mailbox at the top of every realization, so the chunk's
own element realizing is what applies the registration.

- **Parking is opt-in per kind.** An unregistered payload panics at realize —
  and keeps panicking for any kind the app did not declare with
  `defer::<T>()`. Panicking on a genuine missing registration is the
  feature; deferral must not weaken it into a silent blank.
- **A payload that realizes before its handler arrives is completed, not
  abandoned.** A parked scene item holds a layout-transparent placeholder at
  a known parent + index, and the drain splices the real node in and removes
  the placeholder — one node out, one node in, so no sibling index moves and
  nothing else remounts.
- **A deferred payload may not be a subtree root** (a `when` / `switch`
  branch, a keyed row, a navigator screen, portal content). The drain needs a
  parent and an index; a root has neither, and the enclosing region caches
  its root node handles. Wrap it in a view — the same rule `Element::Many`
  has, for the same reason.
- **Keyed by scene `Host`.** A process hosting two backends at once never
  cross-applies a registration.

`tests/lazy-payload-split` measures the bundle win (an eager/lazy pair that
differ only in where the handler registers; the lazy `main.wasm` must be at
least 400 KiB smaller), and `crates/runtime/scene/src/tests.rs` pins the
semantics.

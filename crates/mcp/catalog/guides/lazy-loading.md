+++
title = "Lazy loading a heavy component or SDK"
order = 87
tags = ["performance", "bundle-size", "lazy", "code-splitting", "external", "wasm", "chunks", "allocator", "memory"]
+++

# Lazy loading a heavy component or SDK

A wasm app ships as one module, so a heavy feature used in one corner — a canvas
editor, a PDF viewer, a map, a video compositor — inflates the bundle every user
downloads, even the ones who never open it. **Lazy loading** moves that feature
into a separate wasm *chunk* fetched on first use. The main bundle shrinks; the
chunk loads on demand.

The unit of splitting is a **component**. You mark a component lazy; the
framework compiles its body into a chunk, ships a placeholder in its place, and
loads the real thing when it first mounts. Its props are the data that crosses
the split, so there's nothing new to learn at the call site.

## Make a component lazy

Add `lazy` to `#[component]`, or use the `#[lazy_component]` shorthand:

```rust
use runtime_core::{lazy_component, ui, Element};

/// A heavy editor — its whole subtree ships in a separate chunk.
#[lazy_component]
fn Editor(document_id: u32) -> Element {
    // ... a large tree pulling in a heavy dependency ...
    ui! { view { /* editor UI */ } }
}
```

`#[lazy_component]` is exactly `#[component(lazy)]`; use whichever reads better.
At the call site nothing changes — `Editor` is a normal component tag:

```rust
ui! {
    Editor(document_id = 42)
}
```

- **Props are the args.** `document_id` is captured and passed across the chunk
  boundary. Any runtime input the component needs is just a prop.
- **The component's name identifies its chunk** — the chunk files themselves
  are emitted as `module_<n>___lazy_body.wasm`, and `Editor`'s readable name
  lands in the loader symbol inside `__wasm_split.js`
  (`__wasm_split_load___idealyst_lazy_Editor_…`), so searching that file maps
  any chunk in the network tab back to its component.
- **On native** there's no bundle to split, so the body is compiled inline and
  mounts synchronously — the placeholder never shows.
- **Premint composes.** The `--premint` build-time CSS dump resolves lazy
  boundaries while crawling routes (it pumps its executor per route until the
  bodies have mounted), so styles constructed inside a lazy body mint into the
  shipped CSS like any other mount-time sheet — `--premint-only` works with
  lazily-split screens. Interaction-gated styles inside a lazy body follow the
  usual crawl contract (see the [[styling]] guide).

> `#[component(lazy)]` currently requires **inline props** (declare the props as
> `fn` parameters, as above; zero parameters is fine — the generated props
> struct then carries just the `loading`/`error` config fields). A component you
> need both eager *and* lazy is best expressed by extracting the body into a
> shared `fn` that a plain `#[component]` and a `#[lazy_component]` both call.

## Handle the three states

A lazy load is asynchronous, so it has three observable states: **loading**,
**error**, and the loaded component itself. Configure the first two with the
`loading` and `error` props — both take an ordinary `ui!`-returning closure:

```rust
ui! {
    Editor(
        document_id = 42,
        loading = || ui! { Skeleton() },
        error = |e| ui! {
            view {
                text { format!("Couldn't load the editor: {}", e.message()) }
                Button(label = "Retry", on_press = e.retry())
            }
        },
    )
}
```

- `loading` renders while the chunk is fetching (default: an empty view).
- `error` renders if the fetch or dynamic-link fails. The `LazyError` it
  receives carries `.message()` and `.retry()`.
- The loaded component renders itself on success — there's no third prop for it.

### Retry needs `retryable`

`retry()` re-invokes the loader, which re-passes the props to the component — so
it needs to reproduce them. By default a lazy component moves its props into the
loader **once** (no `Clone` bound, the common case). Opt into retry explicitly:

```rust
#[component(lazy, retryable)]     // derives Clone on the props so retry() works
fn Editor(document_id: u32) -> Element { /* ... */ }
```

Without `retryable` the `error` UI can still show `e.message()`; calling
`e.retry()` will panic (it tells you to add `retryable`). With it, the props
must be `Clone` — a clear compile error names any field that isn't.

## Build a split bundle

Splitting happens during a web build; there's nothing to configure per chunk:

```bash
idealyst build --web --release
```

The release pipeline runs the wasm-split pass; the chunks land in
`dist/web/pkg/` next to the main bundle and load over the network on demand.

Dev builds split too. Outside release the splitter is also the only pass that
compacts the module, so it earns its place even on an app with nothing to
extract — on the `welcome` example, skipping it saves 0.2s of packaging and
grows the served wasm from 2.2 MB to 6.3 MB.

`--no-split` opts out, on both `idealyst build --web` and `idealyst dev`:

```bash
idealyst dev --web --local --no-split
```

It does not remove your lazy boundaries — it declines to extract them. The
bodies ship inside the main bundle and their loaders resolve on a microtask
instead of a network round trip, so a `#[component(lazy)]` still mounts and its
`loading` state simply flashes by. What you give up is the smaller main bundle:
skipping the pass also skips the only compaction a dev build gets, and on a
large app the browser pays for that on every reload.
Chunk-only **code** leaves `main.wasm` automatically. Chunk-only **data** (large
`&'static` tables, an SDK's embedded payload) stays in `main.wasm` by default —
dropping it requires the **experimental, opt-in** `--data-prune`:

```bash
idealyst build --web --release --data-prune   # verify your app still renders!
```

Every pruned symbol is shipped by exactly one artifact: the owning chunk
re-materializes it (from any active data segment — `.rodata`, `.data`, `.bss`)
when it instantiates, and symbols no chunk could restore are never pruned.
`--data-prune` is still off by default because its chunk-only classification
under-approximates what `main` reaches (it can't trace data reached via
data→data pointers or `call_indirect`), so it can silently zero a
main-reachable static that
`main` reads *before* the owning chunk loads — corrupting `main.wasm` with no
error (fonts stop registering, a lazy route renders nothing). Only enable it
after confirming the built app renders correctly, and re-check when your
static data changes.

## Lazy-loading a heavy SDK (extension primitives)

Wrapping a heavy SDK's *usage* in a lazy component splits that corner's
**rendering** code into a chunk. What it does not automatically split is the
SDK's **handler**: a third-party primitive is a payload handler on the scene
`Registry`, and the ordinary place to install one is at boot, from your crate's
`register_scene_extensions`. A handler named there — and everything it
statically reaches — is reachable from `main.wasm` no matter where the
primitive renders.

Registration is the anchor, so moving the anchor is what moves the weight. The
registry has a **late-registration seam** for exactly that, the runtime-v2
successor to the pre-v2 core's `defer_external_registration`:

- **At boot, declare the kind instead of the handler.**
  `registry.defer::<HeavyProps>()` costs `main.wasm` a compile-time `TypeId`
  and nothing else. It is what licenses `realize` to **park** an item of that
  kind behind a layout-transparent placeholder rather than panicking on it. A
  payload kind that was never declared still panics — that distinction is
  deliberate, so a genuinely forgotten registration stays loud.
- **From inside the chunk, install the handler.**
  `runtime_scene::defer_registration::<MyBackend, _>(|registry| {
  registry.register_deferred::<HeavyProps, _>(handler); })` queues it on the
  scene's late mailbox, keyed by host type. The next `realize` drains the
  queue and completes every parked item **in place** — same node, same
  position, no remount.

`main` only ever names the type-erased closure in the drain path, so the
handler and whatever it reaches are constructed only inside the chunk:
wasm-split confines them there and `--data-prune` can then evict their statics
from main. `tests/lazy-payload-split` measures precisely this — the same app
built two ways, differing only in that one line, with the gate requiring the
deferred variant's `main.wasm` to be at least 400 KiB smaller.

The registry also replaced `inventory::submit!` self-registration — every
handler is installed explicitly now, which means the main bundle contains
exactly the handlers you named and nothing else.

The practical recipe:

1. **Split the body.** `#[component(lazy)]` on the component that renders the
   heavy primitive keeps its construction code, its data plumbing, and its
   private helpers out of `main.wasm`.

2. **Defer the handler when it's the heavy part.** `registry.defer::<T>()` at
   boot plus `register_deferred` from the chunk, as above.

3. **Otherwise keep the handler thin.** If you register at boot, structure the
   SDK so the expensive machinery (a rasterizer, a font stack, an embedded
   table) sits behind a function the *chunk's* code path calls, not behind one
   the handler reaches at registration time. Reachability is computed from the
   handler; what the handler can't reach can live in the chunk.

4. **Register only what you render.** Every `register` line in
   `register_scene_extensions` pulls that handler's whole reachable graph into
   main. Since registration is explicit on every target now, that list is
   entirely under your control — an unused line is pure main-bundle cost.

5. **Data needs `--data-prune`.** Even a chunk-only static stays in
   `main.wasm` unless you opt into the experimental pass above.

```rust
// Boot: declare the payload kind late-bound. `main.wasm` never names the
// handler. (Swap this line for `heavy_sdk::register(registry)` if you'd
// rather pay for the handler up front.)
pub fn register_scene_extensions<H: runtime_scene::Host>(
    registry: &mut runtime_scene::Registry<H>,
) {
    registry.defer::<heavy_sdk::HeavyProps>();
}

// The USAGE is what splits — and the chunk installs the handler on its
// way in, via `defer_registration` → `Registry::register_deferred`.
#[component(lazy)]
fn HeavyCorner(document_id: u32) -> Element {
    heavy_sdk::register_from_chunk();
    heavy_sdk::EditorView(document_id)
}
```

### Eager state in a lazy chunk is safe

An SDK or component that allocates reactive state in its **constructor** — a
canvas that creates signals in `new()`, a component that calls `signal()` as it
builds — is safe inside a lazy chunk. The chunk's construction runs under the
chunk's own reactive scope, so that state is owned and torn down with the chunk
(it does **not** leak). You don't need to defer eager-state widgets to
walk-time; build them where they read best.

## Choosing the wasm allocator

Splitting moves code out of the main bundle. The **allocator** is a smaller,
blunter lever on the same number, and it is chosen in the app's own manifest:

```toml
[package.metadata.idealyst.app]
allocator = "small"      # "default" (the default) | "small"
```

`entry!` reads the key and emits the matching `#[global_allocator]` into your
`main.rs`. The two values:

| value | allocator | what you get |
| --- | --- | --- |
| `"default"` — or leaving the key out | `std`'s wasm32 default, `dlmalloc` | size-binned, roughly O(1) per allocation |
| `"small"` | `lol_alloc`'s free list, via `idealyst::alloc::Small` | ~8.7 KB less wasm (measured on the `baseline` example, pre-`wasm-opt`), but **every allocation walks a free list** |

**Default is right for almost every app.** `"small"` was unconditional up to
1.3.7, on the reasoning that a free list costs "a few cycles per allocation".
It doesn't: a free list is a linear scan, so its cost grows with fragmentation
— which is exactly what a UI runtime produces as it mounts a subtree, tears it
down, and mounts a differently-shaped one. In a real app (a schedule grid
re-slicing ~1400 cells per scroll step, debug wasm) it was **62% of a scroll
frame**, and still 44% after two rounds of cutting the app's own hot path.

So reach for `"small"` when the bundle is genuinely size-bound and the app is
mostly static — a landing page, a docs site, an embedded widget — and never for
anything that allocates on a frame (lists, grids, animation, virtualized
scrolling). Profile before and after; ~8.7 KB is a poor trade for a dropped
frame.

Two things worth knowing:

- The key is **web-only**. The emitted static is `cfg`'d to `wasm32`, so an app
  that also builds for iOS/Android/macOS is unaffected there — native shells
  use the system allocator.
- It is manifest metadata rather than a cargo feature **on purpose**. A
  `#[global_allocator]` is process-wide, and cargo features unify across a
  workspace, so a feature would let one crate silently pick the allocator for
  every app built alongside it. Metadata is read from the crate that owns
  `main`, which is the granularity the allocator actually has.

An app with a hand-written `main` (no `entry!`) declares the static itself:

```rust
#[global_allocator]
static ALLOCATOR: idealyst::alloc::Small = idealyst::alloc::small();
```

## See also

- [[external-export]] — the outbound counterpart (shipping components *out* to
  other frameworks) versus splitting them *within* an app.
- [[sdks]] — the SDK crates (canvas, pdf, maps, video) that render through a
  scene-registry payload handler and are the usual lazy-loading candidates.
- [[sdk-components]] — the authoring side: how to build an SDK component so
  consumers can defer it, including the `register` / `defer` /
  `register_from_chunk` seam convention.
- [[backends]] — why the split is a web concern (native compiles the body
  inline).

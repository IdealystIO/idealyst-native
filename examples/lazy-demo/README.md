# lazy-demo — Primitive::Lazy demo

A minimal app showing the `Primitive::Lazy` code-splitting primitive
in action. The parent crate (`lazy-demo`) lazily mounts a chunk
crate (`lazy-demo-chunk`) and renders its lifecycle state.

## What you'll see

### Native (terminal / iOS / macOS / Android)

```
Lazy Primitive Demo
The status line below reflects the chunk's lifecycle:
status: Rendered ✓
[chunk says] Hello from the lazy chunk!  (multiplier = 42)
(rendered by lazy-demo-chunk::app)
```

The chunk is a normal cargo dep on these targets — the framework's
walker dispatches synchronously through a registered thunk and the
chunk's UI is mounted inline. `on_state` fires `Loaded → Rendered`
during the first walk.

### Web

```
Lazy Primitive Demo
The status line below reflects the chunk's lifecycle:
status: Loading chunk...
(loading chunk...)
```

The dynamic-import handler is still pending (PR 6 of the
lazy-primitive series). For now the placeholder renders, `on_state`
fires `Loading` once, and a one-shot stderr warning explains the
state. The chunk doesn't actually load yet — but the API surface is
final, so this code will work end-to-end once PR 6 ships, with no
changes here.

## Run it

Terminal is the fastest path:

```bash
cd examples/lazy-demo
idealyst dev --terminal
```

iOS simulator:

```bash
cd examples/lazy-demo
idealyst dev --ios
```

Web (renders placeholder; full lazy load lands in PR 6):

```bash
cd examples/lazy-demo
idealyst dev --web
```

## How the wiring works today

1. **Parent declares the chunk** in its `Cargo.toml`:

   ```toml
   [package.metadata.idealyst.chunks]
   demo = { path = "../lazy-demo-chunk", crate = "lazy_demo_chunk" }
   ```

   (This is forward-looking — read by PR 2's `chunks!()` macro and
   PR 4's build pipeline. Today nothing consumes it.)

2. **Parent registers the chunk's native thunk** at boot. Without
   the `chunks!()` macro this is manual:

   ```rust
   runtime_core::primitives::lazy::register(DEMO, |payload| {
       let props = payload.downcast::<ChunkProps>().unwrap();
       lazy_demo_chunk::app((*props).clone())
   });
   ```

   Once PR 2 lands, this collapses to `chunks::register(&mut backend)`.

3. **Parent mounts the chunk** via `Primitive::Lazy`:

   ```rust
   let chunk: Primitive = lazy::<ChunkProps>(DEMO, ChunkProps { … })
       .on_state(move |s| state.set(s))
       .placeholder(|| ui! { … }.into_primitive())
       .into_primitive();
   ```

4. **Framework's walker** sees `Primitive::Lazy`. On non-wasm it
   looks up the thunk, calls it, and mounts the returned subtree
   inline. On wasm it mounts the placeholder + fires `Loading`.

## What's not done yet

- **PR 2**: `chunks!()` proc macro — codegens typed `ChunkId`
  constants + `register()` from the manifest. Today the demo
  hand-rolls the equivalent.
- **PR 3**: lift the thread-local registry to per-backend
  registries (matches existing `register_external` shape).
- **PR 4**: build pipeline — multi-wasm-bundle web build.
- **PR 5**: content-hashed bundle filenames + `index.html` rewrite.
- **PR 6**: web backend's dynamic-import lazy handler.
- **PR 7**: migrate the website's Simulator to `Primitive::Lazy`.

See `docs/proposals/lazy-primitive.md` for the full design.

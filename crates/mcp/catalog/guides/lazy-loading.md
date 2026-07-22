+++
title = "Lazy loading a heavy component or SDK"
order = 87
tags = ["performance", "bundle-size", "lazy", "code-splitting", "external", "wasm", "chunks"]
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
- **The chunk is named after the component** — `Editor` produces a readable
  `…_lazy_Editor.wasm`, not a content hash, so you can spot it in the network
  tab.
- **On native** there's no bundle to split, so the body is compiled inline and
  mounts synchronously — the placeholder never shows.

> `#[component(lazy)]` currently requires **inline props** (declare the props as
> `fn` parameters, as above). A no-arg component should take a parameter; a
> component you need both eager *and* lazy is best expressed by extracting the
> body into a shared `fn` that a plain `#[component]` and a `#[lazy_component]`
> both call.

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

The release pipeline runs the wasm-split pass and (with `--release`) prunes
chunk-only data out of `main.wasm`. The chunks land in `dist/web/pkg/` next to
the main bundle and load over the network on demand.

## Lazy-loading a heavy SDK (External extensions)

The same win applies to a heavy **SDK** that renders via `Element::External`
(canvas, PDF, maps, video). But wrapping the *usage* in a lazy component isn't
enough on its own: an SDK's external handler is installed by **registration**,
and if that registration is anchored in the main module, wasm-split keeps the
whole SDK in `main.wasm` regardless of where it renders.

Registration is the anchor, so registration is what must move into the chunk.
Three parts, all required:

1. **Register from inside the chunk**, via `defer_external_registration`. The
   closure — and the SDK it closes over — is constructed only in chunk code, so
   `main.wasm` never reaches it:

   ```rust
   // In the SDK's web module:
   #[cfg(target_arch = "wasm32")]
   pub fn register_lazy() {
       runtime_core::defer_external_registration::<backend_web::WebBackend, _>(|b| {
           b.register_external::<EditorProps, _>(|props, backend| build_editor(props, backend));
       });
   }
   ```

2. **Do not `inventory::submit!` the web handler.** An inventory submission is
   itself a main-module anchor — it drags the SDK back in even if part 1 is
   correct. Keep inventory self-registration for **native** targets (where
   bundle size is a non-issue) and opt web into the lazy path.

3. **Call `register_lazy()` at the top of the lazy component's body**, so the
   handler is queued when the chunk loads. The web backend drains the queue
   right before it dispatches the chunk's own `Element::External`, so the
   freshly-registered handler is found:

   ```rust
   #[lazy_component]
   fn Editor(document_id: u32) -> Element {
       editor_sdk::register_lazy();   // web: defers; native: no-op
       editor_sdk::EditorView(document_id)
   }
   ```

Get any part wrong and the SDK silently stays in `main.wasm` (parts 1–2) or the
external renders a "not supported" placeholder (part 3). Verify the win by
checking `main.wasm` shrank and the chunk carries the SDK's bytes.

### Eager state in a lazy chunk is safe

An SDK or component that allocates reactive state in its **constructor** — a
canvas that creates signals in `new()`, a component that calls `signal()` as it
builds — is safe inside a lazy chunk. The chunk's construction runs under the
chunk's own reactive scope, so that state is owned and torn down with the chunk
(it does **not** leak). You don't need to defer eager-state widgets to
walk-time; build them where they read best.

## See also

- [[external-export]] — the outbound counterpart (shipping components *out* to
  other frameworks) versus splitting them *within* an app.
- [[sdks]] — the SDK crates (canvas, pdf, maps, video) that render via
  `Element::External` and are the usual lazy-loading candidates.
- [[backends]] — why the split is a web concern (native compiles the body
  inline).

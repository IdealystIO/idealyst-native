# framework/reactive

The pre-v2 reactivity substrate, split across two sibling crates so the arena
could be reused by other systems without dragging in the public `Ref<H>`
surface.

| Crate | Path | Role |
| --- | --- | --- |
| `reactive-arena` | [`arena/`](./arena) | Arena allocator backing the scope graph. Holds nodes for signals, effects, and scopes; reclaims them in bulk when a scope is dropped. |
| `reactive-refs` | [`refs/`](./refs) | The `Ref<H>` machinery: typed handles that let a parent component call methods on a child primitive or user component imperatively. |

**Status after the runtime-v2 deletion: nothing in the workspace depends
on either crate.** The reactive kernel authors reach through
`runtime_core::…` (`Signal<T>`, `Effect`, `Memo`, `ReadSignal`,
`WriteSignal`) is now `runtime-world`, re-exported via
`runtime_vocabulary::glue`; `Ref<H>` comes from `runtime-shared`. These
two crates remain as standalone workspace members with no consumers.

They were originally split into separate compilation units because:

- **`reactive-arena`** has no `runtime-core` dependency; it's pure data
  structure. That makes it cheap to depend on from helpers that don't want
  the rest of the framework in their dep graph.
- **`reactive-refs`** isolates the typed-handle layer from the arena so the
  arena can be exercised without `Ref<H>`'s type machinery.

For the reactive model itself (when effects re-run, how scopes nest, the
fine-grained-update contract), see `docs/reactivity.md`.

For where `Ref<H>` is used in author code (`bind(ref)`, the `#[method]`
block inside `#[component]`, imperative method dispatch through `RefOps`),
see `docs/ui-layer.md`.

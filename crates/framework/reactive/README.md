# framework/reactive

The reactivity substrate, split across two sibling crates so the arena can be
reused by other systems without dragging in the public `Ref<H>` surface.

| Crate | Path | Role |
| --- | --- | --- |
| `reactive-arena` | [`arena/`](./arena) | Arena allocator backing the scope graph. Holds nodes for signals, effects, and scopes; reclaims them in bulk when a scope is dropped. |
| `reactive-refs` | [`refs/`](./refs) | The `Ref<H>` machinery: typed handles that let a parent component call methods on a child primitive or user component imperatively. |

Application code rarely depends on these directly — `framework-core`
re-exports the bits authors need (`Signal<T>`, `Effect`, `Scope`, `Ref<H>`).
These crates exist as separate compilation units because:

- **`reactive-arena`** has no `framework-core` dependency; it's pure data
  structure. That makes it cheap to depend on from helpers that don't want
  the rest of the framework in their dep graph.
- **`reactive-refs`** isolates the typed-handle layer from the arena so the
  arena can be exercised without `Ref<H>`'s type machinery.

For the reactive model itself — when effects re-run, how scopes nest, the
fine-grained-update contract — see `docs/reactivity.md`.

For where `Ref<H>` is used in author code — `bind(ref)`, the `methods!` block
inside `#[component]`, imperative method dispatch through `RefOps` — see
`docs/ui-layer.md`.

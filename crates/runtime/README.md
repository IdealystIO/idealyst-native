# `runtime/` — the platform-agnostic upper half

The **Runtime** is the half of Idealyst that runs above the [`Backend`](../../crates/runtime/core/src/backend.rs)
trait. It compiles once and is the same compiled artifact regardless
of which backend the build target picks. It knows nothing about
UIKit, the DOM, Android views, or wgpu pipelines — those live below
the Backend Interface.

| Crate | Path | Role |
| --- | --- | --- |
| `runtime-core` | [`core/`](./core) | Primitives, reactivity (signals/effects/scopes), the render walker, the `Backend` trait itself. Every other crate in the framework depends on this. |
| `runtime-macros` | [`macros/`](./macros) | `ui!`, `jsx!`, `#[component]`, `stylesheet!`, `methods!`. Compile-time DSLs that lower into plain `runtime-core` calls. No runtime cost. |
| `reactive-arena` / `reactive-refs` | [`reactive/`](./reactive) | The reactive substrate split into a pure-data arena + a typed-handle layer. `runtime-core` re-exports what authors use. |
| `runtime-layout` | [`layout/`](./layout) | Taffy wrapper (flex + grid). Used by backends that don't have a native layout engine — currently iOS, Android, and Roku. Web inherits the browser's layout. |

The Runtime's job is to turn app code (components, signals,
stylesheets, navigators) into a primitive tree, then drive that tree
into a `Backend` through the trait. Cross-cutting concerns — hot
reload, dev-server replay, MCP introspection, animation — all hook
into this layer's reactive graph.

## Why "Runtime" and not "framework"

This dir was previously `crates/framework/`. The rename matches the
public-facing concept used in docs: the framework is the Runtime
plus the Backend Interface; everything below the seam is a Backend.
The crate names follow the same convention now (`runtime-core`,
`runtime-macros`, `runtime-layout`).

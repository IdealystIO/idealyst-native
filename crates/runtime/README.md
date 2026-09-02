# `runtime/` — the platform-agnostic upper half

The **Runtime** is the half of Idealyst that runs above the
[`Host`](./scene/src/host.rs) seam and the capability (`*Ops`) traits
defined over it in [`vocabulary/src/caps/`](./vocabulary/src/caps). It
compiles once and is the same compiled artifact regardless of which
backend the build target picks. It knows nothing about UIKit, the DOM,
Android views, or wgpu pipelines — those live below the Backend
Interface.

| Crate | Path | Role |
| --- | --- | --- |
| `runtime-core` | [`core/`](./core) | The author surface — the `runtime_core::…` spelling every app uses. A paper-thin re-export of `runtime_vocabulary::glue` plus the macro set; nothing is implemented here. See its own README. |
| `runtime-world` | [`world/`](./world) | The reactive kernel: worlds, signals, staged commits, derivation-class flush. |
| `runtime-scene` | [`scene/`](./scene) | The scene model: `Element`, `realize`, the `Host` seam, the `Registry` of primitive mount handlers, the Dyn/Keyed structural drivers. |
| `runtime-vocabulary` | [`vocabulary/`](./vocabulary) | The ~30 capability (`*Ops`) traits over `Host`, the builtin primitive handlers, the style-attach engine, navigation, and `glue`. |
| `runtime-shared` | [`shared/`](./shared) | The substrate: style engine + tokens, assets/typefaces, animation, input channels, scheduling, identity, introspection, the robot registry, and every primitive's prop/handle types. |
| `runtime-macros` | [`macros/`](./macros) | `ui!`, `jsx!`, `#[component]`, `stylesheet!`, `#[method]`. Compile-time DSLs whose expansions target `runtime_vocabulary::glue`. No runtime cost. |
| `runtime-layout` | [`layout/`](./layout) | Taffy wrapper (flex + grid). Used by backends that don't have a native layout engine — iOS, Android, macOS, Linux, Windows, terminal, CPU, and the GPU engine. Web inherits the browser's layout. |

The Runtime's job is to turn app code (components, signals,
stylesheets, navigators) into a scene `Element` tree, then realize that
tree into a backend through `Host` + the capability traits.
Cross-cutting concerns — hot reload, dev-server replay, MCP
introspection, animation — all hook into this layer's reactive graph.

## Why "Runtime" and not "framework"

This dir was previously `crates/framework/`. The rename matches the
public-facing concept used in docs: the framework is the Runtime
plus the Backend Interface; everything below the seam is a Backend.
The crate names follow the same convention now (`runtime-core`,
`runtime-macros`, `runtime-layout`).

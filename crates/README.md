# `crates/` — workspace layout

Idealyst is two concepts with a contract between them: the
**Runtime** above and a **Backend** below, glued by the
`runtime_core::Backend` trait (the Backend Interface). Every
top-level directory here maps to one piece of that architecture.

```
crates/
├── runtime/        ← The Runtime: primitives, reactivity, the
│                     Walker, macros. Platform-agnostic.
│
├── backend/        ← Backend Interface impls against native UI
│                     toolkits (UIKit, AppKit, DOM, Android Views,
│                     Roku SceneGraph, terminal).
│
├── gpu-backend/    ← The composed wgpu Backend: Host + Painter +
│                     Engine. Draws its own pixels — no native
│                     toolkit underneath.
│
├── dev/            ← Runtime-locality: wire protocol, hot reload,
│                     dev server, app-side replayer. The seam that
│                     lets the Runtime live on one machine and the
│                     Backend on another.
│
├── mcp/            ← Project-aware introspection: catalog inventory,
│                     stdio MCP server, robot proxy. External tools
│                     query the running project through here.
│
├── ui/             ← Optional component library (idea-ui) + icons.
│                     Pure composition over Runtime primitives.
│
├── sdk/            ← Third-party extension primitives. Each crate
│                     defines a new primitive plus per-Backend impls,
│                     wired through `Primitive::External`.
│
└── tools/          ← User-facing orchestration: the CLI, per-platform
                      build/run, and the source-language porters.
                      Not part of the runtime.
```

## Reading order

If you're new to the codebase:

1. [`runtime/`](./runtime) — start here. The Backend trait lives in
   `runtime/core/src/backend.rs`; everything else hangs off it.
2. [`backend/`](./backend) — pick one (web, ios, android, macos) and
   read it as a complete implementation of the trait.
3. [`gpu-backend/`](./gpu-backend) — the structurally different
   Backend. Lives in three layers because it can't inherit from a
   native toolkit.
4. The rest as needed.

## Architectural overview

The introduction page in the docs site has the full architectural
treatment:
[`examples/docs/src/pages/introduction.rs`](../examples/docs/src/pages/introduction.rs).

Each top-level dir has its own README explaining what lives there
and why.

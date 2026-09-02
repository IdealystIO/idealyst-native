# `backend/` — native-SDK Backend implementations

Each crate here is a concrete implementation of the
[`Host`](../runtime/scene/src/host.rs) seam plus the ~30 capability
(`*Ops`) traits in [`runtime-vocabulary`](../runtime/vocabulary/src/caps),
driving a particular platform's existing UI toolkit. The Runtime calls
capability methods (`create_view`, `apply_style`, `update_text`, …); a
Backend translates each call into operations on its substrate.

These Backends inherit the platform substrate for free — the OS
already owns the event loop, the accessibility tree, scroll physics,
the soft keyboard. A Backend's job is *translation*, not
reimplementation.

| Group | Platform | Substrate |
| --- | --- | --- |
| [`ios/`](./ios) | iOS / iPadOS / tvOS | UIKit via `objc2`. Sub-crates: `core` (shared), `mobile` (iPhone/iPad), `tv` (Apple TV). |
| [`macos/`](./macos) | macOS | AppKit via `objc2`. |
| [`apple/`](./apple) | (shared) | Code shared between `ios/` and `macos/`. |
| [`android/`](./android) | Android | Native `View` hierarchy via JNI. Sub-crates mirror iOS: `core` / `mobile` / `tv`. |
| [`web/`](./web) | Web | DOM nodes, compiled to WebAssembly. |
| [`roku/`](./roku) | Roku | SceneGraph component tree. Demonstrates that the trait isn't tied to traditional GUI toolkits. |
| [`terminal/`](./terminal) | Terminal | TTY cell grid (ANSI). |

The [GPU Backend](../gpu-backend/) is structurally different — it
draws everything itself rather than inheriting from a toolkit, so it
ships as a composition of Host + Painter + Engine instead of a
single crate. See its own README.

## Adding a Backend

Write a new crate that implements `runtime_scene::Host` plus the
`runtime_vocabulary::caps` traits for a target substrate. Wire it into
the workspace and add a `tools/run/` companion if you want a one-shot
launcher. The contract is
documented in [`docs/backend.md`](../../docs/backend.md) and in the
`writing-a-backend` page of the docs site.

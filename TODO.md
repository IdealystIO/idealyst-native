This is a temporary list of todos so I don't forget.

1. Review size of files in the framework - particularily "walker.rs" has become really large
2. Decide what needs to be explicitly defined in the Backend vs optionally
3. revisit install theme requirements
4. Text component macro confusion: text! renders a primitive, not string

## Carried over from a11y workstream

### Bigger feedback items still pending
- **Testing parity** across backends — backend impls have uneven test coverage; need a shared per-backend conformance harness
- **Stylesheet cache** — design / implementation pending
- **macOS gap fill-in** — backend has stub-shaped `create_button` and friends; a11y wiring is in place but the underlying NSButton/NSImageView/etc. impls are still placeholders

### A11y polish (not blocking; intentional gaps documented)
- Per-backend a11y integration tests for web / iOS / Android / macOS (today only framework-core + wgpu + wire have dedicated tests; widget backends rely on wire-roundtrip + build-as-smoke-test)
- Wire the `crates/host/wgpu-accesskit` bridge into an example wgpu app so it gets exercised in practice
- `default_role` exhaustiveness test that asserts every `PrimitiveKind` arm exists
- AccessibilityAction handlers cross the wire as `HandlerId` trampolines — works, but no example app demonstrates AT-triggered actions yet

### A11y intentional gaps (forward-compat by design — leave alone unless requirements change)
- Future `Role` variants surface as `WireRole::Unknown` on older replayers → `None`
- No standalone iOS-shell / AT-SPI bridges (AccessKit covers UIA/AT-SPI/macOS-AX through one API, so wgpu+winit is end-to-end via `crates/host/wgpu-accesskit`)

### Pre-existing breakage noticed in passing (not from a11y work)
- `port-preview` — `discover_framework_core` import unresolved
- `idea-ui-docs` — missing `icon_button!` macro
- `docs` (websites/docs lib test) — 47 compile errors
- `website` SSR example targets (`websites/website/examples/{serve,ssr}.rs`, need `--features ssr`) — API drift: `ServeConfig` gained `extra_head`, `render_document` gained a third arg, `codeblock::register` expects the macOS backend under the sidecar cfg
- `idealyst dev --macos` (remote/wire mode, non-`--local`) — native shell renders BLANK on every page: `[runtime-server-shell] replay error after N cmds: UnknownNode(NodeId(..))` aborts each batch, starting with the very first home-page replay (before any navigation, no `flat_list` involved — observed 2026-07-19 on the website app while verifying the macOS virtualizer fix). Robot tree + navigator state are fine over the bridge; only replay-to-native-views dies. Likely fallout of the swap-navigator wire work in `crates/dev/client/src/navigators.rs`. `--local` mode is unaffected.
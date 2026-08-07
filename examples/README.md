# `examples/`

Curated demo apps. Each is an ordinary app crate: it depends on
`runtime-core`, builds its tree with `ui!`, and exposes
`app()`, `register_scene_extensions(&mut runtime_scene::Registry<H>)`, and
`scene_app()` for the Android wrapper. Build any of them with
`idealyst build --<target> examples/<name>` or run with
`idealyst dev --<target> examples/<name>`.

## Every app owns its entry point

An app crate carries a `src/main.rs` holding one line:

```rust
idealyst::entry!(welcome);
```

plus `idealyst = { workspace = true }` in `[dependencies]`. That macro
reads `[package.metadata.idealyst.app]`, lifts the app's
`register_scene_extensions` into the `SceneExtensions` impl the boot
seam takes, and emits a `main` calling `idealyst::boot::run`. Which
shell that resolves to is **config, not code** — the target triple
settles web/iOS/Android, and a feature on the `idealyst` dep picks
between native shells that share a triple.

There is no CLI-generated *web* wrapper any more: `idealyst build --web`
builds this crate's own binary, and a project without `src/main.rs`
fails with a message naming the fix. iOS and Android still get a
generated wrapper that depends on the app as an `rlib`.

**`register_scene_extensions` must stay registry-GENERIC** —
`fn register_scene_extensions<H>(&mut Registry<H>) where H: …`. The boot
seam is generic over the backend (`SceneExtensions::register<H:
SceneHost>`), so a `cfg`-split seam whose web half takes a concrete
`Registry<WebBackend>` cannot cross it. SDK `register` fns type-dispatch
on the registry internally, so one generic seam serves every target;
`whiteboard-demo` is the worked example.

| Example | What it demonstrates | Targets |
| --- | --- | --- |
| `welcome` | The animation system (springs + tweens + a raf-driven pulse). **Load-bearing**: `idealyst new` copies this project verbatim, so its Cargo.toml + `src/lib.rs` shape is the scaffold's shape (`crates/tools/cli/src/cmd/scaffold_template.rs` mirrors it). | web, ios, android |
| `nav-showcase` | Nested navigators (swap + stack) with `idea-ui-nav` chrome. | web, ios, android |
| `login-demo` | Full-stack auth: `#[server]` login, httpOnly session cookie (BFF), bearer token in the OS keystore on native. | web, ios, android |
| `whiteboard-demo` | Drawable canvas + camera + recording. See its module docs for the platform coverage: the canvas handler is real on web and an External placeholder on native. | web, macos, ios, android |
| `inspector` | A robot-bridge client — a live debugging dashboard over the newline-JSON transport. | macos |
| `baseline` | One text node. The framework's web bundle-size floor. | web |
| `fiddle` | The online playground's compile server (not an idealyst app itself; `template/` is the project it compiles user snippets into). | — |

## Removed in the runtime-v2 one-core wave

`eager-canvas`, `lazy-canvas`, `probe-canvas`, and `prune-repro` were deleted
when the old core was removed. All four were one-shot **bundle-size
experiments** whose subject was the old `Element::External` registry.

### They were scratch fixtures, not a gate

None of the four asserted anything. They had no `tests/`, no size budget, no
CI step, and nothing in the repo read their output — an engineer built each
one, ran `idealyst build --web --release`, and eyeballed `ls -l` on
`main.wasm`. The only automation that ever touched them was workspace
membership (so `cargo check --workspace` compiled them) and a hard-coded
crate-name list in the splitter's **diagnostic** `trace` mode
(`crates/tools/wasm-split/wasm-split-cli/src/lib.rs`, the `CRATES`
attribution table), which prints a per-crate main/shared/chunk breakdown and
asserts nothing. Deleting them therefore removes zero coverage.

`prune-repro` is the one with a durable successor: it reproduced the
`--data-prune` chunk-corruption bug (a chunk-only mutable static in a
non-`.rodata` segment was zeroed in main and never re-materialized in the
chunk), and that bug is now pinned by a unit regression test —
`wasm-split-cli/src/lib.rs::regression_rematerializes_non_rodata_segments` —
which does not need an app to reproduce it.

### Their subject is gone — and so is the capability it measured

The independent variable was *where* the External registry's `Rc<dyn Fn>`
handler landed in `main.wasm`: registered eagerly at boot (`eager-canvas`),
deferred from inside a lazy chunk via `defer_external_registration`
(`lazy-canvas`), or bypassed entirely by calling `canvas_vello::build_canvas`
on the ambient backend (`probe-canvas`).

Runtime v2 has exactly one point on that axis. `runtime_scene::realize` takes
an `Rc<Registry<H>>` with **no interior mutability**
(`crates/runtime/scene/src/realize.rs`, `MountCx.registry`), and
`Registry::register` needs `&mut self`
(`crates/runtime/scene/src/registry.rs`), so a handler can only be installed
at the boot seam — `runtime_vocabulary::register_builtins` plus the app's
`register_scene_extensions` — before the tree goes live. An unregistered
payload panics at realize rather than degrading, which is the same fact seen
from the other side.

Be precise about what that costs, because it is **not** only a lost
experiment. On the old core an SDK could keep a heavy renderer out of the main
bundle by deferring its registration into the chunk; on runtime v2 it cannot.
`canvas_vello::register(&mut Registry<H>)` (`canvas/vello/src/render.rs`) is a
boot-seam registration, so any app that calls it links vello + wgpu into
`main.wasm` whether or not the canvas is behind `#[component(lazy)]`. The same
applies to every handler-backed SDK (pdf, maps, video). **Registration is now
the bundle floor**, and there is no lazy escape hatch to measure.

Two consequences worth carrying forward:

- `tests/lazy-external-split/` (the `eager`/`lazy` pair plus the 512 KiB fake
  `heavy` SDK) and the `measure_registration_split` step in
  `tests/prune-regression` were the **real** automated gate on this — the one
  the deleted examples were hand-built cousins of. It asserted the lazy
  variant's `main.wasm` was ≥ 400 KiB smaller. Those three fixture crates are
  still workspace members, still written against `defer_external_registration`
  / `RegisterExternal`, and **no longer compile** (`#[component]` now emits
  `runtime_vocabulary::` paths). They measure a capability runtime v2 does not
  have, so they cannot be ported as-is: either retarget them to the axis that
  *does* still exist (heavy code reachable only from a `#[component(lazy)]`
  body vs. reachable from `app()`, which still measures wasm-split +
  `--data-prune` eviction) or delete them with the old core. They must not be
  left as a dead gate.
- The published guidance has not caught up. `websites/website/src/pages/
  code_splitting.rs` ("Heavy SDKs") and `docs/proposals/lazy-primitive.md`
  still teach `register_lazy()` built on `defer_external_registration` as the
  supported way to split a heavy SDK. On runtime v2 that pattern does not
  exist.

### What replaced the coverage

Chunk-splitting of ordinary *code* is unaffected: `#[component(lazy)]` lowers
to the vocabulary `lazy` prim and emits the same `#[wasm_split]` chunks, and
`baseline` still measures the framework's floor for one text node.

What now determines that floor is the always-resident handler set, so that is
what is gated:
`crates/runtime/vocabulary/tests/builtin_surface.rs` pins the exact set
`register_builtins` installs (21 single-node + 1 multi-node handler) and the
boot-only registration mechanism itself. Adding a primitive there puts its
handler and everything it reaches into every app on every target; the test
makes that a conscious edit instead of a silent one. It runs offline in
milliseconds, which is why it is a `#[test]` rather than a two-artifact wasm
byte diff.

The other link-time anchors that could pin code into every binary are all
behind off-by-default features and carry nothing in shipped graphs:
`mcp-catalog`'s `inventory` ctors (`runtime-vocabulary/catalog`),
`runtime-shared`'s `linkme` premint registry (`style-dump`), and
`runtime-core`'s `LegacyBridge` (`legacy-bridge`).

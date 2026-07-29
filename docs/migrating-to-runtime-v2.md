# Migrating to runtime v2 (`new-core`)

Runtime v2 replaces the reactive arena, the render walker, and the
`Element` enum with three new crates: `runtime-world` (per-world signal
arenas with staged-commit flushes), `runtime-scene` (a five-variant
structural `Element` + mount drivers), and `runtime-vocabulary` (the
primitives, as registry handlers over per-capability backend traits).
`ui!`, `jsx!`, `#[component]`, and `stylesheet!` lower to the new core
when the `new-core` cargo feature is on; with the feature off they emit
byte-identical old-core code.

The migration's contract is **same source, same behavior**: the op
sequences the new core sends to a backend are pinned byte-for-byte
against goldens recorded from the old walker, modulo a closed list of
sanctioned divergences (almost all confined to teardown ordering — see
["What is guaranteed"](#what-is-guaranteed) below). This guide covers
the parts that are *not* invisible: the reactive-timing semantics that
leak into authored code, the API surfaces that changed or are not yet
ported, and the new boot mechanics.

Verification notes: every behavioral claim in this guide is pinned by a
test or source comment; the pinning path is cited inline. Sections
marked **[in flight]** describe work that was mid-landing when this
guide was written — re-check the cited source before relying on them.

## Breaking changes at a glance

| Old core | Runtime v2 | Failure mode if unmigrated |
| --- | --- | --- |
| `sig.set(v)` — value visible to the next `get()` | write **stages** until the driver's flush; reads see the previous value until then | logic that reads back a just-written signal in the same handler computes with the stale value (silent) |
| `sig.set(get()+1)` twice in one handler → +2 | second `get()` still sees the committed value → net **+1** | lost increments; use `update` (see below) |
| `sig.update(\|v: &mut T\| …)` | `sig.update(\|v: &T\| -> T)` — takes `&T`, returns the new value | compile error: closure shape mismatch |
| `batch(\|\| …)` | removed — every handler/turn is implicitly one batch | compile error: cannot find function `batch` |
| `sig.get_untracked()` | `sig.peek()` | compile error: no method `get_untracked` |
| `sig.update_if_changed(…)` | removed (guarded `set` subsumes it) | compile error |
| `on_cleanup(f)` in a component body | panics `"on_cleanup called outside an effect"` — return the cleanup from an effect instead | runtime panic at build |
| creating `signal`/`effect`/`memo` inside an event handler | panics (handlers run outside the world) | runtime panic on the event |
| free theme fns (`install_tokens(…)`, …) in a handler | panic outside `World::enter` — capture `ThemeCtx` at build, call its methods | runtime panic on the event |
| `presence` re-shown mid-exit reuses the exiting child | builds a **fresh** child (crossfade); child-local state does not survive | visual/state difference, see below |
| omitted required two-way signal prop → detached-sentinel panic | a fresh default-valued signal is minted | old bug becomes silently-working UI |

Everything else — `signal(v)`, `memo(move || …)`, `effect!`, `rx!`,
`ReadSignal`/`WriteSignal`/`.split()`, `Reactive<T>` props, inline-props
`#[component]`, `stylesheet!` with tokens/variants/state overlays,
`provide`/`inject`, `watch`/`Subscription`, `test_id=`, keyed `for`,
reactive `if`/`match` — compiles and behaves the same. The proof crate
is `crates/dev/newcore-app`: one source tree, compiled and e2e-tested
under both cores.

---

## The defaults flip: runtime v2 is the default

As of the wave-2b defaults flip, the NEW core is what you get with no
flags anywhere an author touches the framework:

- **CLI**: `idealyst dev` and `idealyst build` (all targets with a
  new-core leg: web, ssr/ssg, runtime-server/aas, macos, ios, android,
  terminal) resolve the core per project: a project declaring the
  dual-core convention's `new-core` cargo feature builds and runs on
  runtime v2; `--old-core` opts back onto the old walker; `--new-core`
  is accepted as a no-op alias. Legacy projects without the feature
  keep building the old core with a one-line note. Roku and
  `--serverless-lambda` have no new-core CLI leg yet and always build
  old-core (the wrappers pin `old-core` for dual-core apps).
  Old-core builds of a dual-core app compile it
  `default-features = false, features = ["old-core"]` — see
  `build_ios::old_core_user_dep`; resolution lives in
  `crates/tools/cli/src/core_mode.rs`.
- **Fresh projects**: the `idealyst new` scaffold ships
  `default = ["new-core"]`, an empty `old-core` marker feature, and the
  new-core registration seams (a registry-generic
  `register_scene_extensions`, `scene_app()` for the Android wrapper)
  — mirrored verbatim from `examples/welcome`.
- **Dual-core apps in-tree** (welcome, websites/website,
  websites/idea-ui-docs, conformance, newcore-app, nav-showcase):
  `default = [... , "new-core"]`; the old-core leg is
  `--no-default-features --features old-core[,…]` (each crate's
  Cargo.toml documents its exact invocations).
- **Dual-core UI/SDK library crates** (idea-ui, idea-theme,
  idea-ui-nav, the navigator SDKs, and every External SDK below):
  `default = ["new-core"]` — a plain `idea-ui = "…"` dep from a fresh
  app is the alias build. In-repo consumers ride the workspace dep
  specs, which pin `default-features = false` so every graph (old-core
  parity harnesses included) selects a core explicitly.

What deliberately did NOT flip:

- **`runtime-macros/new-core`** stays off by default at the crate level
  — the emission flip is graph-wide (see the hazard below), so it is
  only ever enabled by a consumer's own `new-core` feature.
- **Backend crates' `new-core` features** (backend-web, backend-macos,
  …) stay opt-in: the generated wrappers enable them explicitly per
  build, and the backends' default test suites remain the old-core
  legs.
- **Parity/test graphs that exist to build the old walker**
  (scene-parity, mock-backend, the SDK old-core suites, conformance's
  old leg, the byte-parity corpora) keep doing so via the pinned
  workspace specs plus their documented `--no-default-features`
  invocations.

## The `new-core` feature (the dual-core convention)

Your app declares its own forwarding feature; the canonical block is
`crates/dev/newcore-app/Cargo.toml`:

```toml
[features]
default = ["new-core"]
new-core = [
    "runtime-macros/new-core",
    "dep:runtime-vocabulary",
    "dep:runtime-scene",
    "dep:runtime-world",
]
old-core = ["dep:runtime-core"]
```

**One build graph, one core.** `runtime-macros` is a proc-macro crate,
and proc-macro crates compile once per cargo invocation — any crate in
the graph enabling `runtime-macros/new-core` flips `ui!` emission for
*every* crate in that build (documented in
`crates/runtime/macros/Cargo.toml`). You cannot mix an old-core crate
and a new-core crate in one binary. Choose per build:

```bash
cargo build --features new-core          # everything on the new core
cargo build --features old-core          # everything on the old core
```

### idea-ui

idea-ui and idea-theme compile the same component sources on the new
core through an alias crate: under `new-core`, `extern crate
runtime_facade as runtime_core;` swaps the old root for
`runtime-facade`, a thin re-export of `runtime_vocabulary::glue`
(`crates/runtime/facade/src/lib.rs`, `crates/ui/idea-ui/src/lib.rs`).
Since the defaults flip a plain dep is the new-core build with the full
prim set (`default = [prim families, table, new-core]`); the old-core
leg is the opt-out:

```toml
idea-ui = "…"                       # new core, full component set

idea-ui = { version = "…", default-features = false, features = [
    "old-core",                     # old core, historical default prim set
] }
```

The `table` feature is dual-core: the table SDK carries its own
`new-core` leg (scene-registry handlers — real `<table>` DOM on web,
CSS-grid on native), and idea-ui's `new-core` feature weak-forwards
`table?/new-core`, so `features = ["new-core", "table", …]` just works
(the old `compile_error!` pin is gone).

The `docs` feature (the `DocControls` derive + doc-controls runtime)
is dual-core too: the derive emits the free `::runtime_core::signal(…)`
(glue-mirrored) and the control helpers satisfy the world kernel's
`T: PartialEq` signal bound — `RefBuiltins` now has a `PartialEq`
supertrait with pointer-identity impls on the `*Ref` handle types
(`crates/ui/idea-theme/src/extensible/mod.rs`; identity, not key
equality, so the guarded `set` can never swallow a change between two
distinct modifiers sharing a key). An app-defined `*Ref` implementing
`RefBuiltins` must add the same shape of `PartialEq` impl.

---

## Booting

The old single `run`/`start` entry points are old-core-only. Each
backend gained a `newcore` module with a mirror entry; the shape is the
same everywhere: *(register, build)* — `register` runs after
`runtime_vocabulary::register_builtins` and lets you add your own
primitive handlers to the registry; `build` is your root component call.
Variants without `register` exist for the common case.

| Platform | Entry | Source |
| --- | --- | --- |
| Web (client render) | `backend_web::newcore::start(build)` / `start_in(selector, register, build)` | `crates/backend/web/src/newcore.rs` |
| Web (hydrate SSR output) | `backend_web::newcore::hydrate(build)` / `hydrate_in(selector, register, build)` — an empty mount falls through to a fresh `start_in` | `crates/backend/web/src/newcore_hydrate.rs` |
| macOS | `host_appkit::newcore::run(build, opts)` / `run_with(build, opts, register)` | `crates/gpu-backend/host/appkit/src/newcore.rs` |
| iOS | `backend_ios::newcore::run_in_view(root_view, register, build)` — called from the generated Swift shell | `crates/backend/ios/mobile/src/newcore.rs` |
| Android | generated wrapper's `new-core` feature — `attach` mounts your lib's `scene_app()` via `backend_android::newcore::start` | `crates/tools/build/android`, `crates/backend/android/mobile/src/newcore.rs` |
| SSR | `backend_ssr::newcore::render_to_string(build)` / `render_path(path, build)` / `render_path_with(path, register, build)` — fresh `World` per request, dropped after serialize | `crates/backend/ssr/src/newcore.rs` |
| SSG crawl | `backend_ssr::newcore::render_all(register, build)` — same hierarchy-driven crawl as the old `render_all` (the vocabulary navigator mounts publish their routes into the shared collector); drives `idealyst build --ssg` (the default core since the flip; `--old-core` opts out) | `crates/backend/ssr/src/newcore.rs` |
| SSR server | `backend_ssr::newcore::serve(addr, config, register, build)` (feature `serve`) — the old server's HTTP loop with the new-core renderer; drives `idealyst build --ssr` (the default core since the flip; app exposes `register_ssr_scene_handlers`) | `crates/backend/ssr/src/{newcore,serve}.rs`, `crates/tools/build/ssr` |
| GPU desktop | `host_winit::newcore::run(profile, skin, build)` / `run_with(profile, skin, register, build)` | `crates/gpu-backend/host/winit/src/newcore.rs` |
| Terminal | `host_terminal::newcore::run(build, opts, register)` — the crossterm loop over `backend_terminal::newcore::start(backend, register, build)` (world boot + dispatch-hook flush driver; `render_headless` twin for snapshots); grid parity with the old core pinned by `backend-terminal/tests/newcore_parity.rs` | `crates/backend/terminal/src/newcore.rs`, `crates/gpu-backend/host/terminal/src/newcore.rs` |
| Email | `backend_email::newcore::render_email(build)` / `render_email_with(setup, build)` — one-shot `World` per render (the SSR shape for "SSG for emails"), dropped after serialize; byte-parity with the old core pinned by `backend-email/tests/newcore_golden.rs` | `crates/backend/email/src/newcore.rs` |
| CPU rasterizer | `backend_cpu::newcore::start(backend, register, build)` — host-cadence backend (the host calls `render(surface)` and `dispatch_click`; the wrapped `ClickOutcome` handler schedules the flush, headless hosts settle via `flush_sync`); `set_viewport` forwards into the world's viewport ctx; **pixel** parity with the old core (byte-exact `MemSurface` framebuffers) pinned by `backend-cpu/tests/newcore_parity.rs` | `crates/backend/cpu/src/newcore.rs` |
| Roku (command stream) | `backend_roku::newcore::start(backend, register, build)` + the embedder contract `settle()` (drain microtasks + flush) after dispatching `HandlerTable` events, before draining the command queue — that's the "event → staged writes → flush → emitted commands" boundary; serialized command-stream **byte** parity pinned by `backend-roku/tests/newcore_parity.rs` (no BrightScript thin client exists — the stream is the observable). No viewport surface on this backend (documented in the module) | `crates/backend/roku/src/newcore.rs` |
| Linux (GTK4 scaffold) | `backend_linux::newcore::start(backend, register, build)` — target-gated with the rest of the crate (`#![cfg(target_os = "linux")]`); GTK signal handlers are the wrapped author callbacks. Type-checked on `x86_64-unknown-linux-gnu` both cores; no GTK host environment to run — the scaffold's real-widget build-out remains open | `crates/backend/linux/src/newcore.rs` |
| Windows (Win32 scaffold) | `backend_windows::newcore::start(backend, register, build)` — target-gated (`#![cfg(target_os = "windows")]`); Win32 control callbacks are the wrapped author callbacks. Type-checked on `x86_64-pc-windows-gnu` both cores; no Windows environment to run | `crates/backend/windows/src/newcore.rs` |
| GPU variants (phone/tablet/tv) | `variant_phone::newcore::run(skin, build)` / `run_at` (tablet/tv likewise) — same `DeviceProfile` as the old `run`, over `host_winit::newcore`. `run_runtime_server` stays old-core-only (native runtime-server shells are the dev-chain's named old-core seam) | `crates/gpu-backend/variant/{phone,tablet,tv}/src/newcore.rs` |
| Embedded sim (native hosts) | `host_macos_desktop::mount_newcore(...)` (+ `host_ios_mobile` / `host_android_mobile` mirrors) — the native twins of host-web's: `render_wgpu::newcore::start_in_world` into the page backend's world via the new `backend_{macos,ios,android}::newcore::mounted_world()` accessors, so the page's flush driver commits the embedded app's writes; routed cross-target by `host_wgpu::mount_newcore` (wasm32 → host-web, macOS/iOS/Android → these, others `Unsupported`). macOS live-verified (newcore-macos-smoke embedded-mount phase); iOS/Android compile-verified | `crates/gpu-backend/host/{macos-desktop,ios-mobile,android-mobile,wgpu}` |
| Dev session (runtime-server wire) | `idealyst dev --web` (new core is the default since the flip; `--old-core` opts out) — the sidecar mounts each session through `dev_server::sidecar::run_newcore` → `dev_server::newcore::SceneSession` (per-session `World` + `realize` against the recorder's caps adoption; wire `Command`s out are identical to the old core's, pinned by `mock-backend/tests/wire_behavior_newcore.rs`). Clients replay unchanged; a caps-only replay target exists via `dev_client::newcore::CapsReplay`. Saves apply by rebuild-and-respawn — in-place hot-PATCHING needs the `#[component]` hot-dispatch split, which the new-core emission doesn't have yet | `crates/dev/server/src/newcore.rs`, `crates/dev/server/src/sidecar.rs`, `crates/dev/client/src/newcore.rs`, `crates/tools/build/runtime-server` |

Every boot above wires a **live viewport source** where the platform
has one: the platform's
resize seam pushes into the world's `ViewportCtx`
(`runtime_vocabulary::viewport`), so breakpoint-reactive code —
`current_breakpoint()`, idea-ui `Breakpoint`, AppShell sidebar
pinning, responsive grids — re-fires on window resize, rotation, and
configuration change exactly as it did on the old core. Web listens to
window `resize`; macOS observes the content view (`setFrameSize:`);
iOS observes host bounds (`layoutSubviews`); Android mirrors the host
`ViewGroup` measure (survives Activity recreation); GPU follows
`Host::set_viewport` — the *logical* viewport, which the winit host
keeps fixed per `DeviceProfile` (window resizes letterbox-scale, so
bucket flips only follow logical reports); the terminal and CPU
backends forward `set_viewport` (cells / pixels respectively). SSR
seeds per-request from `SSR_VIEWPORT` (no live source — requests are
point-in-time renders); Roku has no viewport report surface at all,
and the Linux/Windows scaffolds have no resize seam yet (each module
documents the wiring rule for when one lands).
Per-backend seam details: `runtime-vocabulary/src/viewport.rs`,
"Seeding + platform sources".

A minimal web main:

```rust
fn main() {
    backend_web::newcore::start(|| app());   // app() -> Element via ui!
}
```

Web also exposes two interop seams (`crates/backend/web/src/newcore.rs`):
`with_world_entered(f)` — run `f` with the app world entered, for JS
entry points that need to create reactive state — and `flush_sync()` —
commit staged writes immediately (bench drivers, robot verbs).

Working examples for every platform live in `crates/dev/newcore-app`
and the `crates/dev/newcore-*-smoke` crates.

---

## Reactive semantics: writes are staged

This is the one semantic change you must internalize. On the old core a
write landed immediately: "`with_signal_mut` writes the new value
immediately; a `get()` on the next line sees it"
(`docs/automatic-batching.md`) — only the *notification* was batched.
On runtime v2 the **value itself is staged**: `set(v)` records the
pending value and nothing observable changes until the driver flushes
the world (`crates/runtime/world/src/lib.rs` module docs; pinned by
`crates/runtime/world/src/tests.rs::set_stages_until_flush`).

Reads *never* see a staged value. There is no context — handler, effect
body, component body, plain code — in which `get()` or `peek()` returns
an uncommitted write:

| Read | after `set(v)`, before flush | after flush |
| --- | --- | --- |
| `get()` (anywhere) | previous committed value | `v` |
| `peek()` / `with_untracked()` | previous committed value | `v` |
| `update(\|cur\| …)` closure argument | **staged** value (composes) | committed value |

`update` is the read-modify-write primitive — it composes on the staged
value, so increments never get lost
(`tests.rs::update_composes_on_the_staged_value`):

```rust
// WRONG on runtime v2: both get()s see the committed value → net +1.
count.set(count.get() + 1);
count.set(count.get() + 1);

// RIGHT: update composes on the staged value → net +2.
count.update(|n| n + 1);
count.update(|n| n + 1);
```

A *single* `set(get() + 1)` per turn is fine — that's the idiomatic
counter handler and it behaves identically on both cores.

### When does the flush happen?

You never call it. Each backend installs a flush driver that commits
after every author-code entry point returns: event handlers, timers,
animation frames, and async-task polls are wrapped at the dispatch site
(web: `crates/backend/web/src/newcore.rs`; the native backends mirror
the same design). From your point of view: your handler runs to
completion with a consistent pre-write snapshot, then the flush commits
every staged write as **one logical update** — still within the same
tick, before paint. SSR is the degenerate case: one flush per request,
after the tree realizes (`crates/backend/ssr/src/newcore.rs`).

### `batch` is gone

Staging makes every turn an implicit batch, so the explicit `batch(f)`
fn no longer exists — it is not re-exported by the new surface
(`crates/runtime/vocabulary/src/glue.rs` re-exports the reactive
surface; `batch` is absent by design). Delete the wrapper; the writes
inside it already coalesce.

### Guarded `set` (unchanged, but interacts)

The guarded-write family predates this migration and carries over with
the same author contract, now enforced at **commit time**
(`crates/runtime/world/src/tests.rs`, lines cited per test):

- `set(v)` — equality-guarded (`T: PartialEq`). If the staged value
  equals the committed value at commit (including an `A→B→A` round trip
  netting to zero), subscribers are not notified
  (`set_skips_when_value_unchanged`).
- `set_always(v)` — stages and forces notification even on equal value
  (`set_always_notifies_when_value_unchanged`).
- `touch()` — no value write, forces notification
  (`touch_notifies_without_writing`).
- `set_untracked(v)` — writes the committed value directly, bypassing
  staging and notification (`set_untracked_writes_without_notifying`).

### Effects run post-flush, glitch-free

- **Timing.** Effects never run synchronously inside `set()`; they run
  during the flush, once per flush no matter how many of their
  dependencies changed in the turn
  (`tests.rs::diamond_effect_runs_once_with_consistent_pair`).
- **Ordering.** Memos are a separate effect class (derivations) that
  settles *before* user effects run. The diamond case — an effect
  reading both `s` and `memo(|| s.get() * 10)` — can never observe a
  fresh `s` with a stale memo (`crates/runtime/world/src/lib.rs`
  `flush` docs; same test, plus
  `memo_chain_settles_in_one_flush_with_one_reaction_run`). On the old
  core this was the known diamond-glitch; runtime v2 fixes it.
- **Dependencies self-register.** Whatever your effect body actually
  reads on a given run is its dependency set for the next run —
  branches you didn't take this run don't subscribe you
  (`tests.rs::dependencies_recollect_each_run`).
- **Effect bodies are their own tracking context.** An effect *created*
  inside `untrack(…)` still tracks its own reads — the body resets the
  untrack window (`tests.rs::effect_created_inside_untrack_still_tracks`).
- **Memos are equality-guarded.** A recompute that produces an equal
  value does not wake consumers
  (`tests.rs::memo_equality_cut_stops_propagation`).

### `resource` / `mutation` (async reactive)

Both primitives exist on runtime v2 with the old API surface —
`data()` / `error()` / `loading()` / `state()` / `network_state()`,
`refetch()` on resources, `trigger()` / `run()` / `reset()` /
`state_signal()` on mutations, `ResourceCancel` with `on_cancel` — as
glue reimplementations (`crates/runtime/vocabulary/src/async_reactive.rs`,
behind the same `async-driver` gate as the old root; pinned by
`crates/runtime/vocabulary/tests/async_reactive.rs`). An async
completion is event-boundary work: its state write **stages** and
commits at the driver's flush (the executor fires the post-dispatch
hook after every future poll), and a completion that lands after its
owning scope tore down is dropped silently — the resource's cancel
token and the mutation's liveness sentinel guard the write, so
unmount-with-a-fetch-in-flight is safe on both cores.

| Old core | Runtime v2 | Failure mode if unmigrated |
| --- | --- | --- |
| `resource(...)`/`mutation(...)` anywhere; outside a scope they persist for the thread | require `World::enter` (component build / effect); with no collector they live for the world | runtime panic at creation outside the world |
| `refetch()`/`trigger()`/`reset()` after the owning scope dropped: silent no-op | panics with the kernel's stale-handle diagnostic (use-after-unmount surfaced) | runtime panic — restructure so the handle doesn't outlive its component |
| N same-turn `refetch()` calls issue N fetches (first N−1 discarded by the stale guard) | coalesce into ONE fetch per flush | none observable — fewer wasted requests |
| `NetworkState::from(&state)` ad-hoc conversion | use `.network_state()` on the handle (the `From` impls would be orphans in glue) | compile error under the alias |

---

## Component bodies, scopes, and teardown

### Bodies run once, untracked

A `#[component]` body runs exactly once, inside
`component_scope` = "collect everything created, run untracked"
(`crates/runtime/world/src/lib.rs::component_scope`;
`tests.rs::component_bodies_run_untracked`). A bare `sig.get()` in a
component body is a build-time snapshot — it can never subscribe
anything, even accidentally. This is the same authoring rule the 0.3
snapshot-trap warning taught (`docs/migration-0.2-to-0.3.md`), now
structural. Reactive reads belong in `ui!` slots, `rx!`, `move ||`
closures, memos, and effects — exactly as before.

### Teardown is drop

Everything you create in a component body — signals, effects, memos —
is collected into the component's scope and freed when the component
unmounts (effects' cleanups first, then the slots;
`tests.rs::dropping_the_owned_scope_retires_the_effect`,
`component_scope_collects_effects_into_owned`). There is no separate
dispose call.

### `on_cleanup` placement tightened

On the old core, `on_cleanup(f)` in a component body attached to the
surrounding scope, and outside any context it was silently dropped
(`crates/runtime/core/src/reactive.rs::on_cleanup` docs). On runtime v2
it requires a **running effect** and panics otherwise:
`"on_cleanup called outside an effect"`
(`crates/runtime/world/src/lib.rs::on_cleanup`). Two migration shapes:

```rust
// Old: component-body cleanup.
on_cleanup(move || timer.cancel());

// New — either register inside an effect…
effect!({
    on_cleanup(move || timer.cancel());
});

// …or return the cleanup from the effect body (runs before each
// re-run and at teardown; tests.rs::returned_cleanup_from_effect_body).
effect!({
    let t = start_timer();
    move || t.cancel()
});
```

The effect-owned shape also fixes a real bug class: a cleanup owned by
the scope cancels its timers when the component unmounts, so detached
callbacks can't outlive a remounted world.

---

## Event handlers run outside the world

Handlers (`on_click`, `on_change`, timers, async continuations) execute
*outside* `World::enter` — the world is only entered during builds and
flushes. Signal **handles are Copy and route to their own world**, so
the everyday surface just works in a handler: `get`, `peek`, `set`,
`set_always`, `touch`, `update` on any signal you captured at build
time. What does *not* work in a handler is anything that needs the
ambient world:

- **Creating state** — `signal(…)`, `effect(…)`, `memo(…)`,
  `provide`/`inject` panic outside `enter`. Create state at build time
  and capture the handles.
- **The free theme fns** — `install_tokens(…)` & co. resolve the
  ambient world's `ThemeCtx` and panic in a handler. Capture the ctx at
  build time and call its methods, which are documented handler-safe
  (`crates/runtime/vocabulary/src/theme.rs` — the comment records the
  live panic that motivated this):

  ```rust
  let theme = theme_ctx();                 // build time (world entered)
  ui! {
      button(label = "Dark", on_click = move || {
          theme.install_tokens(&dark_tokens());   // handler-safe method
      })
  }
  ```

- **Navigation is already handler-safe.** `on_select`, `pop`, and
  `NavHandle` dispatch never mount screens directly — they stage the
  command into a queue and a driver effect commits it on the flush
  ("one navigation = one logical update";
  `crates/runtime/vocabulary/src/handlers/navigator.rs` module docs).

If a foreign entry point (JS interop, a native callback you own) really
must create reactive state, web exposes
`backend_web::newcore::with_world_entered(f)`.

### After the world is gone

Per-world lifetime is strict (`crates/runtime/world/src/tests.rs`):
**writes** to a dropped world are silent no-ops
(`writes_to_a_dead_world_are_noops`) — an in-flight async task
completing after unmount is harmless — while **reads** panic
(`reads_from_a_dead_world_panic`), surfacing the handle leak instead of
returning garbage. SSR pins the same contract per request
(`crates/backend/ssr/tests/newcore_isolation.rs`).

---

## Authored surfaces not yet on the new core

These fail loudly at compile time with a message naming the deferral —
none degrade silently. Exact messages live in
`crates/runtime/macros/src/ui.rs` and `lib.rs`. Status as of this
guide (a `link(route=)` port was in flight when this was written —
trust the compiler over this table):

| Surface | Status | Workaround |
| --- | --- | --- |
| `link(route = …)` (in-app links) | compile error — the ambient link-activator seam retargets with the navigation SDK | `link(external = "…")` works; for in-app navigation dispatch through the injected `SwapNav`/`StackNav` handles |
| `#[component(lazy)]` / `#[lazy]` | **supported** — lowers to the vocabulary `lazy` prim (`glue::primitives::lazy` + `handlers/lazy.rs`): placeholder-first mount, chunk body swaps in on load, `loading`/`error` props, retry, SSR keeps the loading UI (byte-identical to old-core SSR). Same `#[wasm_split]` chunk naming, so `idealyst build --web` splits identically. One shape change under the hood: the chunk fn returns a body *thunk* (construction runs in the mount handler's swap effect, under the world) | — |
| `web_view` | **supported** — the snake_case tag special-case is gone from the macro entirely (it was never a first-party primitive and never resolved on the old core either); the WebView SDK ships the tag contract itself: `ui! { WebView(url = …) }` is ordinary `BuildElement` dispatch on BOTH cores (`crates/sdk/client/webview`, `type WebView = WebViewProps`) | — |
| virtualizer `for i in count(sig)` sugar | compile error — generator-backend metadata is post-migration | the `flat_list(data=…, key=…, size=…, render=…)` tag works |
| `CardTabs` ui! sugar | compile error — rides the old `cardtabs!` macro | — |
| `#[method]` legacy explicit-props / generic form | compile error — `Bindable<H>` rides the old `Element` | the inline-props form works (see Testing below) |

### External SDKs (the third-party primitive layer)

On the old core, peripheral SDKs render via `Element::External` plus a
per-backend `register_external` registry. The new core has no separate
External concept — the scene `Registry` treats primitives and externals
uniformly, so each SDK registers its payload handler exactly like
`register_builtins` registers the core primitives. Every External SDK
is dual-core (same public names both cores, default-off `new-core`
feature, `oldcore.rs`/`newcore.rs` split — the navigator/codeblock/table
house pattern). App-side changes on the new core:

- **Registration moves to the boot seam.** There is no inventory
  self-registration; compose the SDK registers into the boot entry's
  `register` argument: `backend_web::newcore::start_in("#app",
  |r| { svg::register(r); canvas_native::register(r); }, app)`. An
  UNREGISTERED payload panics at realize (the scene contract) — the
  old core rendered a placeholder box instead, so a missed `register`
  fails loud rather than soft.
- **Callbacks flush.** SDK glue that fires author callbacks from
  platform event sources outside the framework's wrapped dispatch
  sites (a `<form>` submit listener, an iframe `message` event, an
  NSToolbar button action) calls the backend's
  `newcore::schedule_flush()` after the callback returns — this closes
  the "External glue must call schedule_flush" residual named in each
  backend's `newcore.rs` module docs (web and macOS closed by this
  wave; the pattern is pinned per SDK).

| SDK | New-core status | Notes |
| --- | --- | --- |
| `table` | web `<table>` handlers + native CSS-grid (earlier wave) | SSR reuses the generic handlers |
| `codeblock` | caps-generic handler, web + SSR (earlier wave) | byte-identical `<pre>`/span DOM |
| `svg` | web `innerHTML` handler; placeholder elsewhere | native usvg walk old-core-only (seam documented) |
| `video` | web `<video>` handler; placeholder elsewhere | macOS/iOS/Android players old-core-only (seam documented) |
| `webview` | web `<iframe>` handler (author callbacks flush); placeholder elsewhere | `WebView` ui! tag works on BOTH cores now |
| `form` | web real `<form>` + children + submit-flush; placeholder+children elsewhere | `Form` tag = manual `BuildElement` on the new leg |
| `markdown` | ONE caps-generic semantic-DOM handler for ALL hosts | SSR now gets real markdown DOM (old SSR rendered the placeholder — an upgrade, documented) |
| `maps` | web leaf handler; placeholder elsewhere | `maps-ios` old-core-only |
| `toolbar` | REAL macOS `NSToolbar` leg (clicks flush via `backend_macos::newcore::schedule_flush`); placeholder elsewhere | registration-time type dispatch; Windows/Linux legs old-core-only |
| `screen-recorder` (`PrivateLayer`) | passthrough children handler (`register_scene`) | capture-EXCLUDED native windows old-core-only (seam documented) |
| `canvas` | web Canvas2D renderer (`canvas-native`) + SSR `<canvas>` host (`canvas_core::register_ssr_scene`); placeholder on native | GPU `canvas-vello` + native painters old-core-only; renderers register a `CanvasPrim` handler (the seam) |

Live evidence: `crates/dev/newcore-canvas-smoke` (repo-root
`newcore-canvas-smoke.png`) — an author draw closure repainting through
the SDK's world effect on a real new-core web boot.

Two adjacent nuances:

- **`Ref` / `AnchorTarget`.** `Ref` is old-arena machinery; on the new
  core only the detached sentinel `Ref::default()` is sanctioned —
  `get()` returns `None` and an `anchored_overlay(target = …)` built
  from it falls back to unresolved positioning
  (`crates/runtime/vocabulary/src/glue.rs`, the `AnchorTarget`
  re-export comment).
- **Omitted required signal props.** Leaving out a required two-way
  prop (`text_input.value`, `toggle.value`, `slider.value`, …) mints a
  fresh default-valued signal instead of the old core's detached
  sentinel (`crates/runtime/macros/src/ui.rs`, the `fresh_signal`
  emission sites; `crates/runtime/vocabulary/src/glue.rs::fresh_signal`).
  Code that accidentally relied on the sentinel's panic now silently
  gets a working, unshared signal.

---

## Behavior changes you can observe

### `presence`: re-present during exit is a crossfade

The one deliberate rendering-semantics change. When a `presence` child
is dismissed and re-presented *while the exit animation is still
running*:

- **Old core:** the exit was cancelled and the outgoing child reused —
  child-local state survived the flicker.
- **Runtime v2:** the outgoing child finishes its exit on its own timer
  while a **fresh** child enters — a crossfade. Child-local state does
  **not** survive a mid-exit flicker, and the exit timer is deliberately
  never cancelled (cancelling would orphan the detached nodes).

Pinned in `crates/runtime/vocabulary/src/handlers/presence.rs` module
docs and the presence-cycle goldens
(`crates/dev/scene-parity/goldens_full_newcore/full_presence_*.golden`).
If a modal/toast keeps state that must survive rapid dismiss/re-show,
lift that state above the `presence` boundary.

### Reactive `match` dedupes equal scrutinees

A reactive `match` (and `switch`) is keyed on the scrutinee **value**:
a dependency re-fire that produces an equal value keeps the mounted arm
instead of rebuilding it (`crates/runtime/vocabulary/src/glue.rs`,
`switch` docs). Consequence: `touch()` on a signal in a match scrutinee
is inert — if you need a forced rebuild, change the value (e.g. include
a generation counter in the scrutinee tuple).

### Teardown-window ordering

Within the release window of an unmounting subtree, cleanup ordering
differs (old: scope cleanups LIFO, then effects in creation order; new:
per-effect cleanups in creation order — divergence class #5 in
`crates/dev/scene-parity/README.md`). The release *set* is identical.
This only matters if sibling cleanups depend on each other's relative
order — they shouldn't.

---

## What is guaranteed

The parity harness (`crates/dev/scene-parity`) pins the exact backend
op sequences — creates, inserts, style applies, text updates, removals
— for 13 structural scenarios × 2 mount modes plus 27 full-op
scenarios, recorded from the **old** walker and matched byte-for-byte
by the new core. Divergence is a closed, enumerated list ("These are
the known, deliberate divergence points. Everything not listed here is
a hard invariant." — `crates/dev/scene-parity/README.md`). The six
classes, in author terms:

1. Node-naming shifts from lazy anchor creation — invisible.
2. A skipped no-op `clear_children` on a virgin anchor — invisible.
3. Cross-effect firing order after unsubscribes — invisible.
4. Final-unmount (owner-drop) mechanics — invisible.
5. Teardown release ordering within the unmount window — see above.
6. Presence mid-exit re-present — the crossfade change, see above.

Status at time of writing (all from checked-in gates):

- **Conformance:** the robot-driven cross-platform suite passes 8/8 on
  the new core (primitives, modal, stack-nav, `#[method]` suites) vs
  the old core's 8/9 — the one old-core failure is pre-existing and
  unrelated (`crates/dev/robot-e2e/examples/conformance`). The idea-ui
  suite's new-core leg is **[in flight]** with the idea-ui retarget.
- **Performance:** 9 of 11 js-framework-bench ops are within the ±5 %
  gate or better; `create_1k`/`create_10k` are +24 %/+23 % (≈0.3 µs per
  row of payload construction, mechanism identified; full table and
  residual profile in `benchmark/idealyst-native/README.md`). Every
  interactive-update path — granular bumps, shared/point restyles,
  signal-class flips, hierarchy updates, teardown — is at old-core
  parity.
- **SSR / SSG:** output is byte-identical to the old renderer across the
  8-scenario corpus (html and head CSS, including a `render_all` crawl
  pair — `crates/backend/ssr/tests/newcore_byte_identity.rs`) AND
  across the ENTIRE website: all 33 routes crawled through the
  production SSG drivers plus one route served over real HTTP, old vs
  new byte-identical (`websites/website/tests/ssg_parity.rs`; the one
  documented exception is `presence`'s Dyn-hole anchor, a layout-inert
  `display: contents` wrapper each core's own hydration expects). This
  is the hydration-compatibility proof.

---

## Crate layout: `runtime-shared` (the split substrate)

The migration split the old `runtime-core` in two:

- **`runtime-shared`** — the permanent substrate both cores compile
  against: the style engine (`StyleRules`, stylesheets, tokens, theme
  plumbing), colors, assets/fonts (`typeface!`/`face!`), animation
  types, touch/hover/wheel/file-drop event types, the legacy reactive
  arena (`Signal`/`effect`/`memo`, `Ref`/`node_ref!`), scheduling /
  time / session, viewport + breakpoints + safe-area, logging, debug
  counters (`debug-stats`), the robot registry + bridge, native
  introspection, page metadata, the host slots (platform / color
  scheme / URL opener / fullscreen / announcer), and every
  per-primitive prop/handle struct (`primitives::*`).
- **`runtime-core`** — ONLY the old walker half: `Element`, the
  `Backend` mega-trait, the render walker, and the `Bound`/builder
  authoring layer. It re-exports everything from `runtime-shared` at
  the old paths, so **old-core consumers compile unchanged** — keep
  writing `runtime_core::…`. The crate is deleted at the end of the
  migration; the shared substrate outlives it.

Every thread-local moved WITH its module: `runtime-shared` owns the
single authority and `runtime-core` only re-exports. (A second copy
would silently split state — signals set through one core invisible to
the other.)

What this means for downstream crates:

- **Old-core apps/SDKs**: nothing. `runtime_core::…` paths resolve to
  the same items via re-export.
- **New-core (`new-core` feature) builds**: `runtime-vocabulary` and
  `runtime-facade` now depend on `runtime-shared` directly — a default
  new-core dependency graph contains **no `runtime-core`**
  (`cargo tree -e normal -p runtime-vocabulary | grep runtime-core`
  is empty).
- **`legacy-bridge` feature (`runtime-vocabulary`)**: the one
  remaining old-core coupling, off by default. It gates
  `bridge::LegacyBridge` (mounting the new core through an old
  `Backend` impl) and the `NavigatorOps::create_navigator` cap (whose
  `NavigatorHost` closes over the old `Element`). Backends whose
  `newcore.rs` delegates `create_navigator` to their old Backend impl
  enable it inside their own `new-core` feature; so do the parity and
  test harnesses. It is deleted together with `runtime-core`.
- **`css` and `runtime-layout`** depend on `runtime-shared` (they only
  ever needed the style data model, not the walker).

---

## Testing and the robot bridge

The robot surface (`idealyst test`, the MCP verbs, `robot-test`)
adapts verb-for-verb. What changes for test authors:

- **`test_id=`** works identically on both cores (14 registering
  primitives; `crates/runtime/vocabulary/src/robot.rs` mirrors the old
  registry's model 1:1 — same query semantics, same last-wins duplicate
  policy).
- **`#[method]`** requires the inline-props component shape (props as
  fn parameters; zero params is fine) — the legacy explicit-props form
  is a compile error (see table above). Method bodies that must compile
  on both cores should write `value.set(value.get() + n)` rather than
  `update`, whose closure shape differs between cores
  (`crates/dev/newcore-app/src/app.rs::MethodTally`). In robot builds
  only, a methods-bearing component adds one anchor view on anchored
  hosts (navigator screens, keyed rows) — non-robot builds are
  structurally identical (`crates/runtime/vocabulary/src/robot_methods.rs`
  module docs).
- **`robot::watch_signal(name, sig)`** accepts `Signal`, `ReadSignal`,
  or `Memo`, and must run where effect creation is legal — a component
  body, an effect, or any world-entered build scope; the entry's
  lifetime is tied to that scope
  (`crates/runtime/vocabulary/src/robot_watch.rs`).
- **`assert_signal`** in `#[robot_test]` fns is unchanged — it needs
  the target registered via `watch_signal` first, and the wire shape is
  identical across cores.
- **MCP catalog + robot in new-core dev sessions** (wave 2b — this
  used to be a named gap). The catalog is core-agnostic now: the
  core-primitive recipes are STATIC data in `runtime_shared::recipes`
  (`include_str!` of `crates/runtime/shared/recipes/*.rs`,
  compile-gated on both cores by
  `crates/dev/newcore-app/tests/recipes_compile.rs`), and the
  `__mcp` inventory anchor exists per lowering: `runtime_core::__mcp`
  (old), `runtime_vocabulary::glue::__mcp` (retargeted emissions), and
  the facade root (alias-resolved derive/`recipe!`/`doc_scope!`
  emissions) — all three the same `mcp-catalog` instance. A new-core
  graph turns everything on with `runtime-facade/dev` (= robot +
  catalog); the generated new-core sidecar wrapper enables
  `runtime-core/dev` + `runtime-facade/dev`, and the sidecar session
  starts the bridge and installs
  `dev_server::newcore::install_robot_env` (vocabulary registry for
  element verbs, shared-bridge fallback for `get_catalog`/logs/
  customs via `runtime_shared::robot::bridge::install_verb_router`).
  Pinned by `dev-server/tests/newcore_robot_catalog.rs` and the
  build-runtime-server wrapper tests.

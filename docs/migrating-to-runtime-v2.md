# Migrating to runtime v2

> **Upgrading an app? Start at
> [`migration-0.5-to-1.0.md`](migration-0.5-to-1.0.md).** Runtime v2 ships
> as **1.0.0**, and that page is the front door: the complete breaking-change
> inventory for the 0.5.x → 1.0 range (including the removals this document
> does not list), plus the order to work in. It links back here for the
> detail — staged writes, boot, handler context, teardown, crate layout,
> testing — which is what this document is for. Read it there, come here for
> the why.

Runtime v2 replaced the reactive arena, the render walker, and the
`Element` enum with three crates: `runtime-world` (per-world signal
arenas with staged-commit flushes), `runtime-scene` (a six-variant
structural `Element` + mount drivers), and `runtime-vocabulary` (the
primitives, as registry handlers over per-capability backend traits).
`ui!`, `jsx!`, `#[component]`, and `stylesheet!` lower onto that stack.
It is now the only runtime: the pre-v2 walker has been deleted, and
there is no feature or flag that selects a core.

The migration's contract was **same source, same behavior**: the op
sequences the new core sends to a backend are pinned byte-for-byte
against goldens recorded from the old walker, modulo a closed list of
sanctioned divergences (almost all confined to teardown ordering — see
["What is guaranteed"](#what-is-guaranteed) below). This guide covers
the parts that are *not* invisible: the reactive-timing semantics that
leak into authored code, the API surfaces that changed or were never
ported, and the boot mechanics.

Verification notes: every behavioral claim in this guide is pinned by a
test or source comment; the pinning path is cited inline.

## Breaking changes at a glance

| Old core | Runtime v2 | Failure mode if unmigrated |
| --- | --- | --- |
| `sig.set(v)` — value visible to the next `get()` | write **stages** until the driver's flush; reads see the previous value until then | logic that reads back a just-written signal in the same handler computes with the stale value (silent) |
| `sig.set(get()+1)` twice in one handler → +2 | second `get()` still sees the committed value → net **+1** | lost increments; use `update` (see below) |
| `sig.update(\|v: &mut T\| …)` | `sig.update(\|v: &T\| -> T)` — takes `&T`, returns the new value | compile error: closure shape mismatch |
| `batch(\|\| …)` | removed — every handler/turn is implicitly one batch | compile error: cannot find function `batch` |
| `sig.get_untracked()` | `sig.peek()` | compile error: no method `get_untracked` |
| `sig.update_if_changed(…)` | removed (guarded `set` subsumes it) | compile error |
| `signal(v)` for any `T: Clone` | **`T: PartialEq`** is a bound on the whole `Signal<T>`/`ReadSignal<T>`/`WriteSignal<T>` surface — creation, `get`, `set_always`, `touch`, not just guarded `set` | compile error: `can't compare T with T` / "method exists but its trait bounds were not satisfied". Derive `PartialEq`; for a payload with no value equality, give it a pointer-identity impl (see below) |
| `Signal::new(v)` | removed — the free `signal(v)` is the constructor | compile error: no associated function `new` |
| `on_cleanup(f)` in a component body | panics `"on_cleanup called outside an effect"` — return the cleanup from an effect instead | runtime panic at build |
| creating `signal`/`effect`/`memo` inside an event handler | panics (handlers run outside the world) | runtime panic on the event |
| free theme fns (`install_tokens(…)`, …) in a handler | panic outside `World::enter` — capture `ThemeCtx` at build, call its methods | runtime panic on the event |
| `presence` re-shown mid-exit reuses the exiting child | builds a **fresh** child (crossfade); child-local state does not survive | visual/state difference, see below |
| omitted required two-way signal prop → detached-sentinel panic | a fresh default-valued signal is minted | old bug becomes silently-working UI |

### `PartialEq` is a bound on the signal, not just on `set`

The guarded-`set` family predates runtime v2, but on the old core only
the *write* methods carried `T: PartialEq`; the arena itself accepted
any `T: Clone`. The world kernel bounds the whole handle
(`crates/runtime/world/src/lib.rs` — `signal`, `impl<T: PartialEq +
'static> Signal<T>`), so a payload that cannot be compared cannot be
stored in a signal at all. Two migration shapes:

- **Derive it.** Message enums, DTOs, view models — add `PartialEq`
  alongside `Clone`/`Serialize`. This is the normal answer.
- **Give it a pointer-identity impl** when the payload genuinely has no
  value equality (a connection handle, an `Rc<dyn Any>` theme slot).
  Compare `Rc::ptr_eq` on an `Rc` the type already holds — "is this the
  same instance?" is exactly the question the guarded `set` asks. Two
  in-tree examples: `IdeaThemeRef`'s `ThemeSlot`
  (`crates/ui/idea-theme/src/theme_runtime.rs`) and
  `server::SocketSender` (`crates/api/server/src/socket.rs`). Pair it
  with `set_always` at the write site if every write must notify
  regardless.

SDK-visible consequence: `server::use_socket<In, Out>` and
`server::use_sse<T>` now require `PartialEq` on the *inbound* message
type (`In` / `T`) — the type that lands in a signal. `Out` is
unaffected.

Everything else — `memo(move || …)`, `effect!`, `rx!`,
`ReadSignal`/`WriteSignal`/`.split()`, `Reactive<T>` props, inline-props
`#[component]`, `stylesheet!` with tokens/variants/state overlays,
`provide`/`inject`, `watch`/`Subscription`, `test_id=`, keyed `for`,
reactive `if`/`match` — compiles and behaves the same. The end-to-end
proof crate is `crates/dev/newcore-app`, which drives every one of those
surfaces through the real macro → builder → handler → driver → kernel
path.

---

## One core, no flags

Runtime v2 is the only runtime. The pre-v2 walker — the `runtime-core`
crate's `Element` enum, `Backend` mega-trait, render walker, and
`Bound`/builder layer — has been deleted, so there is nothing to select:

- **CLI**: `idealyst dev`, `idealyst build`, and `idealyst run` build
  runtime v2 for **every** target — web, ssr/ssg, runtime-server/aas,
  macos, ios, android, terminal, roku, the embedded sim, and
  `--serverless-lambda`. There is no core to resolve: `--new-core` is
  accepted as a working no-op and `--old-core` is a hard error pointing
  here (`crates/tools/cli/src/core_mode.rs`). Every generated wrapper
  takes a **plain path dep** on the user crate — the app's own defaults
  select its feature set, and no wrapper pins a core feature or
  `default-features = false`.
- **Fresh projects**: the `idealyst new` scaffold declares no core
  feature at all. It takes plain deps on `runtime-core`,
  `runtime-vocabulary`, and `runtime-scene`, and ships the registration
  seams the generated
  wrappers call: a registry-generic `register_scene_extensions`, plus
  `scene_app()` for the Android wrapper. `examples/welcome` is the
  scaffold's source of truth — the CLI `include_str!`s it, so the two
  cannot drift.
- **Apps and library crates in-tree** carry no core feature either.
  There is nothing to select, so `idea-ui = "…"` (or
  `{ workspace = true }`) is the whole dep line, and the workspace specs
  no longer pin `default-features = false` to force a choice.

### There is no core feature any more

Earlier releases used a per-crate `new-core` / `old-core` feature pair
to pick a runtime. Both are gone. If you are porting a project that
declared them:

```toml
# BEFORE — the dual-core convention
[features]
default = ["new-core"]
new-core = [
    "runtime-macros/new-core",
    "dep:runtime-vocabulary",
    "dep:runtime-scene",
    "dep:runtime-world",
]
old-core = ["dep:runtime-core"]

[dependencies]
runtime-core = { version = "…", optional = true }
runtime-facade = { version = "…", optional = true }
runtime-vocabulary = { version = "…", optional = true }
runtime-scene = { version = "…", optional = true }
```

```toml
# AFTER — one core, unconditional deps
[dependencies]
runtime-core = "…"            # the author surface (`runtime_core::…`)
runtime-vocabulary = "…"      # the `ui!` emission's absolute paths land here
runtime-scene = "…"           # `Registry`, for the registration seam
```

If you were on an interim build that carried
`extern crate runtime_facade as runtime_core;` at your crate root, delete
that line: `runtime-core` is the real package again and the
`runtime-facade` package no longer exists.

Every `runtime_core::…` path in your sources keeps resolving unchanged.
See ["Crate layout"](#crate-layout-runtime-shared-the-split-substrate).

### idea-ui

idea-ui, idea-theme, and idea-ui-nav reach the framework through the
same `runtime_core::…` root every app does, so a plain dep is the whole
story:

```toml
idea-ui = "…"                       # the full component set
```

Two feature notes:

- The `table` feature pulls the table SDK (real `<table>` DOM on web,
  CSS-grid on native, generic handlers reused by SSR).
- The `docs` feature (the `DocControls` derive + doc-controls runtime)
  works because the derive emits the free `::runtime_core::signal(…)`
  and the control helpers satisfy the world kernel's `T: PartialEq`
  signal bound — `RefBuiltins` has a `PartialEq` supertrait with
  pointer-identity impls on the `*Ref` handle types
  (`crates/ui/idea-theme/src/extensible/mod.rs`; identity, not key
  equality, so the guarded `set` can never swallow a change between two
  distinct modifiers sharing a key). An app-defined `*Ref` implementing
  `RefBuiltins` must add the same shape of `PartialEq` impl.

### `prim-*` primitive-family gating is gone

**What was there.** Bundle-size gating had three layers, all removed:

| Layer | Surface | What turning a family OFF did |
| --- | --- | --- |
| `runtime-core` | twelve default-on features — `prim-virtualizer`, `prim-icon`, `prim-image`, `prim-text-input` (TextInput **and** TextArea), `prim-toggle`, `prim-slider`, `prim-activity`, `prim-portal` (portal / overlay / anchored_overlay), `prim-presence`, `prim-graphics`, `prim-navigator` (navigator + outlet + URL sync + `Link`'s nav dispatch), `prim-lazy` | deleted the walker's per-`Element`-variant dispatch arm, the authoring builder fn, and the `Backend` trait's methods for that family. Each backend crate (`backend-web`, …) mirrored all twelve and forwarded to `runtime-core` |
| `idea-ui` | **six** default-on features — `prim-icon`, `prim-image`, `prim-text-input`, `prim-activity`, `prim-portal`, `prim-presence` — each forwarding the same-named `runtime-core` feature | `#[cfg]`-deleted every component that (transitively) rendered the family, so naming one was a compile error rather than a runtime placeholder. `prim-icon` ⇒ Icon / IconButton / Breadcrumbs / Checkbox / Switch / Slider / Pagination; `prim-image` ⇒ Image / Avatar; `prim-text-input` ⇒ Textarea; `prim-activity` ⇒ Spinner; `prim-portal` ⇒ Menu / Popover / Tooltip; multi-family closures Button (icon+activity), Alert (icon+activity), Select (icon+portal), Modal (portal+presence), Field (icon+activity+text-input), Autocomplete (text-input+portal), Toast / ToastHost (all four) |
| CLI | `idealyst build --web --primitives=<list>` — comma-separated family names without the `prim-` prefix (`icon,text-input`, or `none` for a text/view-only bundle) | restricted the generated web wrapper's `runtime-core` / `backend-web` feature selection. Unknown names were a hard error; the build warned when the *app* crate's own dep line kept default features, since cargo unification would silently re-widen the set |

**All of it is removed**, along with the SDK-side forwards that fed it
(`virtualized` → `prim-virtualizer`, `swap-navigator` / `stack-navigator`
/ `idea-ui-nav` → `prim-navigator`).

**Why.** The features gated things that no longer exist. Runtime v2 has
no render walker and no `Backend` mega-trait:
`runtime_vocabulary::handlers::register_builtins` installs one handler
per primitive into a `runtime_scene::Registry`, and reachability from
that boot seam — plus LTO — decides what links. `runtime-vocabulary` has
no `prim-*` equivalent, so idea-ui's six features had nothing left to
forward to; keeping them would have kept deleting components from the
public API while shrinking nothing, which is strictly worse than not
having them. The size win *was* the whole point.

**What you must change.** Nothing in your *source*: no type, function,
macro, or component was lost, and the component set is now
unconditional — a build that previously selected a subset now gets
strictly more of idea-ui, not less. Only manifests and build commands
move:

```toml
# BEFORE — a size-restricted app
runtime-core = { version = "…", default-features = false }
idea-ui = { version = "…", default-features = false, features = [
    "prim-icon", "prim-portal",
] }
```

```toml
# AFTER — the whole component set, unconditionally
idea-ui = "…"
```

`runtime-core` itself is gone (see ["Crate
layout"](#crate-layout-runtime-shared-the-split-substrate)); replace it
with `runtime-core` + `runtime-vocabulary`, no feature list. If you
kept `default-features = false` on `idea-ui` only to drop the optional
`table` SDK, that still works — write `idea-ui = { version = "…",
default-features = false }` with no `prim-*` entries. The surviving
idea-ui features are exactly `table` (default), `docs`, and `robot`.

Both halves fail loudly rather than silently, by design:

- A stale `features = ["prim-…"]` entry is a cargo resolve error
  (*"does not have these features"*) — the manifest will not build until
  you delete it.
- `idealyst build --web --primitives=…` is a hard CLI error naming this
  guide (`crates/tools/cli/src/removed_flags.rs`), the same treatment
  `--old-core` gets. Accepting it as a no-op would let a size-tuned
  release pipeline keep "succeeding" while quietly producing the
  all-families bundle it was written to avoid.

**What is genuinely lost, with no replacement:** the ability to shrink a
web bundle by compiling out primitive families. There is no v2 flag for
it today. The structural successor is per-primitive **handler
registration** — `register_builtins` holds the only reference to each
primitive's module and its caps calls, so a per-family gate belongs
there (and in each backend's caps impls), with LTO dropping whatever
nothing registers. That is a narrower *registration* set, not a
compile-time amputation of the component library. The per-component
family map above is preserved verbatim in idea-ui's crate docs
(`crates/ui/idea-ui/src/lib.rs`, "Primitive families per component")
precisely so the author-facing half can be restored mechanically if that
split lands.

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
| macOS | `host_appkit::run(build, opts)` / `run_with(build, opts, register)` (also resolves at `host_appkit::newcore::run` — compat re-export) | `crates/gpu-backend/host/appkit/src/boot.rs` |
| iOS | `backend_ios::newcore::run_in_view(root_view, register, build)` — called from the generated Swift shell | `crates/backend/ios/mobile/src/newcore.rs` |
| Android | generated wrapper's `attach` mounts your lib's `scene_app()` via `backend_android::newcore::start`, passing your `register_scene_extensions` seam | `crates/tools/build/android`, `crates/backend/android/mobile/src/newcore.rs` |
| SSR | `backend_ssr::newcore::render_to_string(build)` / `render_path(path, build)` / `render_path_with(path, register, build)` — fresh `World` per request, dropped after serialize | `crates/backend/ssr/src/newcore.rs` |
| SSG crawl | `backend_ssr::newcore::render_all(register, build)` — same hierarchy-driven crawl as the old `render_all` (the vocabulary navigator mounts publish their routes into the shared collector); drives `idealyst build --ssg` | `crates/backend/ssr/src/newcore.rs` |
| SSR server | `backend_ssr::newcore::serve(addr, config, register, build)` (feature `serve`) — the old server's HTTP loop with the new-core renderer; drives `idealyst build --ssr` (app exposes `register_ssr_scene_handlers`) | `crates/backend/ssr/src/{newcore,serve}.rs`, `crates/tools/build/ssr` |
| GPU desktop | `host_winit::run(profile, skin, build)` / `run_with(profile, skin, register, build)` (also resolves at `host_winit::newcore::run` — compat re-export) | `crates/gpu-backend/host/winit/src/app.rs` |
| Terminal | `host_terminal::run(build, opts, register)` (also resolves at `host_terminal::newcore::run` — compat re-export) — the crossterm loop over `backend_terminal::newcore::start(backend, register, build)` (world boot + dispatch-hook flush driver; `render_headless` twin for snapshots); grid parity with the old core pinned by `backend-terminal/tests/newcore_parity.rs` against the frozen dumps in `tests/goldens/` | `crates/backend/terminal/src/newcore.rs`, `crates/gpu-backend/host/terminal/src/boot.rs` |
| Email | `backend_email::newcore::render_email(build)` / `render_email_with(setup, build)` — one-shot `World` per render (the SSR shape for "SSG for emails"), dropped after serialize; byte-parity with the old core pinned by `backend-email/tests/newcore_golden.rs` against the frozen output in `tests/goldens/` | `crates/backend/email/src/newcore.rs` |
| CPU rasterizer | `backend_cpu::newcore::start(backend, register, build)` — host-cadence backend (the host calls `render(surface)` and `dispatch_click`; the wrapped `ClickOutcome` handler schedules the flush, headless hosts settle via `flush_sync`); `set_viewport` forwards into the world's viewport ctx; **pixel** parity with the old core (byte-exact `MemSurface` framebuffers) pinned by `backend-cpu/tests/newcore_parity.rs` | `crates/backend/cpu/src/newcore.rs` |
| Roku (command stream) | `backend_roku::newcore::start(backend, register, build)` + the embedder contract `settle()` (drain microtasks + flush) after dispatching `HandlerTable` events, before draining the command queue — that's the "event → staged writes → flush → emitted commands" boundary; serialized command-stream **byte** parity pinned by `backend-roku/tests/newcore_parity.rs` (no BrightScript thin client exists — the stream is the observable). Drives `idealyst build roku`: the generated snapshot wrapper mounts, calls `settle()`, then drains the queue into the baked `data/ui.json`. No viewport surface on this backend (documented in the module) | `crates/backend/roku/src/newcore.rs` |
| Linux (GTK4 scaffold) | `backend_linux::newcore::start(backend, register, build)` — target-gated with the rest of the crate (`#![cfg(target_os = "linux")]`); GTK signal handlers are the wrapped author callbacks. Type-checked on `x86_64-unknown-linux-gnu` (`PKG_CONFIG_ALLOW_CROSS=1` + homebrew gtk4 `.pc` metadata); no GTK host environment to run — the scaffold's real-widget build-out remains open | `crates/backend/linux/src/newcore.rs` |
| Windows (Win32 scaffold) | `backend_windows::newcore::start(backend, register, build)` — target-gated (`#![cfg(target_os = "windows")]`); Win32 control callbacks are the wrapped author callbacks. Type-checked on `x86_64-pc-windows-gnu`; no Windows environment to run | `crates/backend/windows/src/newcore.rs` |
| GPU variants (phone/tablet/tv) | `variant_phone::run(skin, build)` / `run_at` / `run_with` (tablet/tv likewise; all three also resolve under the `variant_*::newcore::` compat re-export) over `host_winit::run_with`; drives `idealyst run sim`. `run_runtime_server` is a separate, core-agnostic path — it replays a dev host's wire stream rather than mounting a tree, and is currently blocked on `RuntimeServerShell`'s `B: Backend` bound (see "Runtime-server clients" below) | `crates/gpu-backend/variant/{phone,tablet,tv}/src/boot.rs` |
| Embedded sim | `host_wgpu::mount(surface, size, profile, painter, build)` — one cross-target entry (wasm32 → `host_web::mount`, macOS/iOS/Android → `host_{macos_desktop,ios_mobile,android_mobile}::mount`, others `MountError::Unsupported`). Each realizes through `render_wgpu::newcore::start_in_world` into the embedding page/app backend's world (`backend_{web,macos,ios,android}::newcore::mounted_world()`), so the host's flush driver commits the embedded app's writes. `mount_newcore` stays as a compat re-export at every level. macOS live-verified (newcore-macos-smoke embedded-mount phase); iOS/Android/web compile-verified. `HostHandle::pause`/`resume` are documented no-ops — see the crate docs | `crates/gpu-backend/host/{macos-desktop,ios-mobile,android-mobile,web,wgpu}` |
| Dev session (runtime-server wire) | `idealyst dev --web` — the sidecar mounts each session through `dev_server::sidecar::run_newcore` → `dev_server::newcore::SceneSession` (per-session `World` + `realize` against the recorder's caps adoption; wire `Command`s out are identical to the old core's, pinned against the frozen wire snapshot by `mock-backend/tests/wire_behavior.rs`). The browser client replays through `dev_client::WireBackend` (bounded on `caps::AllCaps` — wire commands in, capability calls out; `new_newcore` survives as an alias of `new` for the generated wrapper). Saves apply by rebuild-and-respawn — in-place hot-PATCHING needs the `#[component]` hot-dispatch split, which the scene emission doesn't have, so the generated host runs with `hot_patch: None` | `crates/dev/server/src/newcore.rs`, `crates/dev/server/src/sidecar.rs`, `crates/dev/client/src/newcore.rs`, `crates/tools/build/runtime-server` |

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
(the pre-v2 wording, preserved verbatim in
`docs/automatic-batching.md`'s "what changed" paragraph) — only the
*notification* was batched.
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

### `batch` and `cycle` are gone

Staging makes every turn an implicit batch, so the explicit `batch(f)`
fn no longer exists — it is not re-exported by the new surface
(`crates/runtime/vocabulary/src/glue.rs` re-exports the reactive
surface; `batch` is absent by design). Delete the wrapper; the writes
inside it already coalesce.

`cycle(f)` is gone for the same reason, plus one more: it also guarded
re-entrancy while a signal's storage was moved out of the arena for an
`update`, and staging removes that hazard entirely — there is no
moved-out box to re-enter. Delete the wrapper. See "The `runtime_core::`
root: what moved, what is gone, what replaced it" for the full
accounting of removed and reimplemented root items.

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
| `NetworkState::from(&state)` ad-hoc conversion | use `.network_state()` on the handle (the `From` impls would be orphans in glue) | compile error |

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
// re-run and at teardown; tests.rs::returned_closures_are_cleanups).
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

## Authored surfaces that were never ported

These fail loudly at compile time with a message naming the deferral —
none degrade silently. Exact messages live in
`crates/runtime/macros/src/ui.rs` and `lib.rs`. Two rows that were
deferred mid-migration have since landed and are marked **supported**
below; trust the compiler over this table:

| Surface | Status | Workaround |
| --- | --- | --- |
| `link(route = …)` (in-app links) | **supported** — un-deferred in the P6 nav wave. Both cores emit the same three-positional constructor; the retarget maps it onto `glue::primitives::link::link`, whose mount resolves the ambient `LinkActivator` the vocabulary navigators provide. Pinned by `ui.rs::link_route_lowers_to_link_constructor` | — |
| `#[component(lazy)]` / `#[lazy]` | **supported** — lowers to the vocabulary `lazy` prim (`glue::primitives::lazy` + `handlers/lazy.rs`): placeholder-first mount, chunk body swaps in on load, `loading`/`error` props, retry, SSR keeps the loading UI (byte-identical to old-core SSR). Same `#[wasm_split]` chunk naming, so `idealyst build --web` splits identically. One shape change under the hood: the chunk fn returns a body *thunk* (construction runs in the mount handler's swap effect, under the world) | — |
| `web_view` | **supported** — the snake_case tag special-case is gone from the macro entirely (it was never a first-party primitive and never resolved on the old core either); the WebView SDK ships the tag contract itself: `ui! { WebView(url = …) }` is ordinary `BuildElement` dispatch on BOTH cores (`crates/sdk/client/webview`, `type WebView = WebViewProps`) | — |
| virtualizer `for i in count(sig)` sugar | compile error — generator-backend metadata is post-migration | the `flat_list(data=…, key=…, size=…, render=…)` tag works |
| `CardTabs` ui! sugar | compile error — rides the old `cardtabs!` macro | — |
| `#[method]` legacy explicit-props / generic form | compile error — `Bindable<H>` rides the old `Element` | the inline-props form works (see Testing below) |
| `async_reducer(state, perform, apply)` / `AsyncReducer` / `AsyncStatus` | **supported** — reimplemented on the world kernel in `runtime_vocabulary::async_reactive` and re-exported by glue behind the `async-driver` feature (the same gate as `resource` / `mutation`). The row below in "what moved, what is gone, what replaced it" names its divergences: `S: PartialEq` + `E: PartialEq`, no `cycle(..)` wrap, creation requires `World::enter`. **Not** the `runtime_shared::async_reducer` original, which is still arena-built and is not what glue re-exports | — (`mutation(handler)` remains a fine choice when you only need request lifecycle; that is how `crates/api/server/examples/server-fn-demo` and `crates/api/server-aws/examples/contact-form-lambda` are written) |

### The `runtime_core::` root: what moved, what is gone, what replaced it

The old `runtime-core` crate root was one line — `pub use
runtime_shared::*;` — so **everything public at the shared root reached
authors for free**. The surviving root
(`runtime_vocabulary::glue`, re-exported by `runtime-core` and reached
as `runtime_core::…`) *enumerates* its exports instead, because a chunk
of the old surface is old-arena machinery that must be reimplemented on
the world kernel rather than forwarded.

That difference is deliberate, but it means an item nobody listed simply
disappeared. This table is the complete accounting. **Everything in the
"restored" section resolves at `runtime_core::…` again** and is pinned by
`crates/runtime/vocabulary/tests/glue_host_surface.rs` and
`glue_reactive_surface.rs` — a path pin, so a future refactor cannot
satisfy the name with a divergent shim.

#### Restored — same item, same behavior (straight re-exports)

| `runtime_core::…` | Notes |
| --- | --- |
| `announce`, `open_url`, `set_fullscreen`, `color_scheme` | ambient-host free fns; route to the host-installed announcer / opener / setter, no-op when the host installed none |
| `host` (module), incl. `host::color_scheme()` | the module path the old glob also exposed |
| `color` (module: `parse_or`, `Rgba`, …) | shared color parsing/blending |
| `set_app_key_handler` | the free installer; the caps side was always mirrored |
| `use_id`, `use_id_keyed`, `hash_key`, `Identity`, `current_identity`, `with_current_identity` | see the caveat under "Restored but degraded" |
| `flat_list`, `fixed_size`, `FlatListItemSize` | at the ROOT, where the old core had them (they also live at `primitives::flat_list::…`) |
| `FileDropEvent`, `FileDropPhase`, `DroppedFile`, `WheelEvent`, `WheelKind` | the payloads the `*Handler` aliases carry — unspellable before |
| `Recognizer`, `RecognizerCtx`, `RecognizerKind`, `RecognizerUpdate`, `GestureState`, `AsyncNotifier` | the gesture-recognizer contract the pan / zoom / dnd SDKs implement |
| `active_touch_claim`, `set_active_touch_claim` | touch-claim arbitration |
| `schedule_microtask` | at the root, alongside `after_ms` / `raf_loop` |
| `logging` (module) | platform-routed logging |
| `premint` (module) | behind the `style-dump` feature, as before — `stylesheet!`'s dump registration emits into it |

#### Restored as REIMPLEMENTATIONS on the world kernel

These could not be re-exported. The shared originals build old-arena
signals and effects, which nothing on a world mount subscribes to — a
forwarded call would compile and then **silently do nothing**, which is
worse than the gap. Same names, same semantics, with the divergences
named:

| `runtime_core::…` | Divergence from the old core |
| --- | --- |
| `on(deps, f)` | none |
| `on_defer(deps, f)` | none |
| `memo_with(eq, f)` | requires `T: PartialEq` (world signal storage is `PartialEq`-bounded end to end). The custom-comparison use case is fully preserved; the "`T` has no `PartialEq` at all" case is not expressible — wrap `T` in a newtype whose `PartialEq` encodes the comparison |
| `reducer(initial, fold)` | requires `S: PartialEq`. No `cycle(..)` wrap (see below). The "every dispatch notifies" contract is preserved |
| `async_reducer(state, perform, apply)` + `AsyncReducer`, `AsyncStatus` | requires `S: PartialEq` and `E: PartialEq`. No `cycle(..)` wrap. Creation requires `World::enter`; lifetime is the registering scope; in-flight completions that land after teardown are dropped (a stale-handle write is a kernel panic by design) |
| `resource`, `mutation` | see "resource / mutation" above |

#### Restored but DEGRADED — name resolves, contract narrowed

| Item | What is different |
| --- | --- |
| `use_id()` / `use_id_keyed(k)` | 🔴 **Position-independence is currently lost.** The documented contract is "deterministic per position in the tree", which the OLD walker delivered by calling `with_current_identity` before every emission (`runtime-core/src/walker.rs::build`). `runtime_scene::realize` sets no ambient identity, so every call answers from `Identity::UNIDENTIFIED` and all call sites in a tree return the SAME string. Stable and non-panicking, so nothing crashes. `with_current_identity` still works for callers that establish identity themselves. Pinned by `glue_reactive_surface.rs::use_id_is_currently_position_independent_because_the_renderer_seeds_no_identity`, which goes red when the renderer starts seeding — the same change that restores the dev-server recorder's identity-keyed `NodeId` reuse across hot-reload rebuilds |

#### Gone by decision — with the replacement

| Old | Replacement |
| --- | --- |
| `batch(\|\| …)` | **nothing to call.** Staging makes every turn an implicit batch: writes inside a handler coalesce and commit together at the driver's flush. Delete the wrapper |
| `cycle(\|\| …)` | **nothing to call.** `cycle` was the old arena's "one reactive cycle" wrapper — it coalesced sibling writes AND guarded re-entrancy while a signal's box was moved out of the arena for `update`. Staging subsumes both: writes coalesce by construction, and there is no moved-out box to re-enter. The framework's own uses were removed, not ported (`async_reducer`'s apply is the load-bearing example — its old `cycle` wrap existed to stop `apply` writing a sibling signal from aborting with "RefCell already mutably borrowed") |
| `style-dynamic` cargo feature | **nothing to set.** The last remnant of the `prim-*` bundle-size gating model that stage 2 removed by decision. It had already stopped removing anything: the surviving style engine (`runtime-vocabulary/src/style_attach.rs`) matches all six `StyleProp` arms unconditionally, `backend-web` had dropped its forward (so the documented "`default-features = false` on BOTH runtime-core and your backend crate" was an unknown-feature error), and feature unification force-enabled it in any graph containing the vocabulary. `style-dump` no longer implies it. There is no static-only style mode on runtime v2 |
| `Signal::dispose()`, the unowned-signal leak diagnostic, `arena_stats` | old-arena introspection with no world counterpart. Lifetime is ownership now: a `Realized`/`Owned` drop frees its slots. `runtime_world` has no arena-wide statistics surface |

#### Newly public

| Item | Why |
| --- | --- |
| `world_is_entered() -> bool` | **the handler-safe fork.** `true` ⇒ a world is ambient, so injecting/reading world-scoped context is legal; `false` ⇒ the caller is an event handler, timer, or async continuation, which must use a handle captured at build time. Injecting from a handler panics with "signal()/effect() called outside `World::enter`" — the shape of the idea-theme theme-swap crash. It existed as the doc-hidden `__world_is_entered` while it was believed to be migration-internal; SDK authors need it, so it has a public spelling now (the alias stays for existing macro emissions) |

### Runtime-server clients (native `idealyst dev` shells)

The native runtime-server clients — `host_terminal::run_runtime_server`,
`variant_{phone,tablet,tv}::run_runtime_server`,
`host_appkit::run_aas`, and the iOS/Android/macOS backends'
`runtime_server` modules — replay a dev host's wire stream rather than
mounting a tree, so none of their bodies are core-coupled. They were
blocked only on the generic bound they inherit from
`RuntimeServerShell<B: Backend>`; **that bound flip has landed** —
`dev_client::WireBackend<B: caps::AllCaps>` dispatches onto
`runtime_vocabulary::caps` directly (dissolving the `CapsReplay`
adapter) and the shell follows. Every concrete backend already adopts
the caps, so each `run_runtime_server` compiles unchanged.

#### The one thing that did NOT come along: client-side SDK registration

Two shells passed a `fn(&mut ConcreteBackend)` into their boot entry so
they could register first-party SDK handlers on the *client* —
`crates/backend/ios/rs-shell` (`swap_navigator` / `stack_navigator` /
`codeblock` / `table`) and the Android wrapper generated by
`crates/tools/build/android` (`drawer_navigator` / `codeblock` /
`table`). That argument was the old `Element::External` client registry,
and it is **obsolete, not pending**: an SDK handler now runs on the
**host** side of the wire, registered on `Registry<WireRecordingBackend>`
through the sidecar's `register_scene_extensions_recorder` seam, so what
reaches the client is already ordinary primitive commands. Both shells
pass an empty closure and have dropped their SDK dependencies.

One behavior delta follows, and it applies to `idealyst dev` only:
an SDK whose `register` type-dispatches to a backend-CONCRETE handler
(`codeblock` picks a native `UITextView` / `NSTextView` / Android
`TextView` mount when it sees that backend's registry) cannot take that
branch under hot-reload, because the registry it sees belongs to the
recorder. Such a primitive renders its **portable** variant in a dev
session and its native variant in a real build. Locally-mounted device
builds (`idealyst run --ios`, `--android`, …) go through the app's own
`register_scene_extensions` and are unaffected.

### Third-party primitive SDKs (the former "External" layer)

On the old core, peripheral SDKs rendered via `Element::External` plus a
per-backend `register_external` registry. There is no separate External
concept any more — the scene `Registry` treats first-party primitives and
third-party payloads uniformly, so each SDK registers its payload handler
exactly like `register_builtins` registers the core primitives. Every SDK
below is a single implementation with no core feature and no
`oldcore.rs`/`newcore.rs` split; its public paths are unchanged from the
dual-core era, so author code and docs did not churn. App-side changes:

- **Registration moves to the boot seam.** There is no inventory
  self-registration; compose the SDK registers into the boot entry's
  `register` argument: `backend_web::newcore::start_in("#app",
  |r| { svg::register(r); canvas_native::register(r); }, app)`. An
  UNREGISTERED payload panics at realize (the scene contract) — the
  old core rendered a placeholder box instead, so a missed `register`
  fails loud rather than soft.
- **…except for kinds you explicitly declare late-bound.** The old
  core's `defer_external_registration` — registering a heavy SDK's
  handler from inside a `#[component(lazy)]` chunk so its code stays out
  of `main.wasm` — has a scene-model successor. At the boot seam,
  `registry.defer::<TheirProps>()` declares the payload kind late-bound;
  realize then PARKS items of that kind behind a layout-transparent
  placeholder instead of panicking. The handler installs itself later
  with `registry.register_deferred::<TheirProps, _>(mount)` (through
  `runtime_scene::defer_registration::<TheirBackend, _>(…)` when the
  chunk body has no registry in hand), and every parked item realizes in
  place — same position, same node shape as an eager mount, no remount.
  Three differences from the old seam, all deliberate: parking is
  **opt-in per kind** (an undeclared unknown payload still panics —
  see the previous bullet), an item that realized *before* the handler
  arrived is completed rather than left as a permanent placeholder, and
  a deferred payload may not be a subtree ROOT (a `when` branch, keyed
  row, navigator screen or portal root) — wrap it in a view, exactly as
  `Element::Many` requires. `tests/lazy-payload-split` measures the
  bundle win; `crates/runtime/scene/src/tests.rs` pins the semantics.
- **Callbacks flush.** SDK glue that fires author callbacks from
  platform event sources outside the framework's wrapped dispatch
  sites (a `<form>` submit listener, an iframe `message` event, an
  NSToolbar button action) calls the backend's
  `newcore::schedule_flush()` after the callback returns — this closes
  the "External glue must call schedule_flush" residual named in each
  backend's `newcore.rs` module docs (web and macOS closed by this
  wave; the pattern is pinned per SDK).
- **A native leg is selected by REGISTRY TYPE, not by `cfg`.** An SDK
  whose `register` is caps-generic (`register<H>(&mut Registry<H>)`)
  installs its concrete native handler by downcasting the registry
  (`(registry as &mut dyn Any).downcast_mut::<Registry<MacosBackend>>()`)
  and falls through to the portable/placeholder handler otherwise. A
  `cfg(target_os = "macos")` split alone cannot express this: that cfg is
  equally true for an SSG/SSR render running on a macOS host, which needs
  the portable handler. `toolbar`, `codeblock`, `svg`, `video`,
  `canvas-native` and `screen-recorder` all use this shape.

| SDK | Handler coverage | Notes |
| --- | --- | --- |
| `table` | web `<table>` handlers + native CSS-grid | SSR reuses the caps-generic handlers. Flattened single-core: no features, `src/lib.rs` is the whole crate |
| `codeblock` | REAL single-node macOS / iOS / Android handlers + a portable `<pre>`/span handler everywhere else | registration-time type dispatch (see below). Flattened single-core |
| `svg` | web `innerHTML` handler + REAL iOS / Android native usvg vector walk; placeholder elsewhere | the native painters were ported onto `Registry<IosBackend>` / `Registry<AndroidBackend>` via registration-time type dispatch. Flattened single-core |
| `video` | web `<video>` handler + REAL macOS / iOS AVPlayer + Android `VideoView` players; placeholder elsewhere | ported onto the concrete native registries via registration-time type dispatch. Flattened single-core |
| `webview` | web `<iframe>` handler (author callbacks flush); placeholder elsewhere | 🔴 **no native renderer.** A `WKWebView` / `android.webkit.WebView` leg would be a `Registry<IosBackend>` / `Registry<AndroidBackend>` handler returning the platform view, plus a `schedule_flush` wrap on its navigation/message callbacks. `WebView` ui! tag + `WebViewOps` degrade to `UnsupportedOps`. Flattened single-core |
| `form` | web real `<form>` + children + submit-flush; placeholder+children elsewhere | the placeholder arm realizes the form's CHILDREN into the external node, which IS the passthrough-container behavior native wants — nothing is lost off-web. `Form` tag = manual `BuildElement` (children move out of the props). Flattened single-core |
| `markdown` | ONE caps-generic semantic-DOM handler for ALL hosts | every caps-complete host (web, SSR, native) mounts the identical semantic tree. There is no single-node `UILabel`+`NSAttributedString` / `TextView`+`Spannable` renderer; one would be a separate concrete registration off the same `MarkdownDoc`. Flattened single-core |
| `maps` | web OpenStreetMap iframe + REAL iOS `MKMapView`; placeholder elsewhere | both leaves are backend-concrete handlers over the leaf crates' node builders (`maps_web::build_map_iframe`, `maps_ios::build_map_view`); the leaves no longer carry a `register`. Flattened single-core |
| `toolbar` | REAL macOS `NSToolbar` + Windows Common-Controls + GTK4 HeaderBar; placeholder elsewhere | registration-time type dispatch; every desktop leg wraps clicks with its backend's `newcore::schedule_flush`. `toolbar::flush_pending` is back on Windows. Flattened single-core |
| `screen-recorder` (`PrivateLayer`) | REAL capture-EXCLUDED windows on iOS / macOS / Android; passthrough container on web + hosts with no exclusion mechanism | `register_scene` type-dispatches to each backend's `create_private_layer_window`; the vacuous `screen_recorder::register` no-op is gone. Flattened single-core |
| `canvas` | web Canvas2D + REAL iOS/macOS CoreGraphics + Android `android.graphics` (`canvas-native`), GPU vello on every host with the graphics cap (`canvas-vello`), SSR `<canvas>` host (`canvas_core::register_ssr_scene`) | renderers register a `CanvasPrim` handler; `canvas-native` type-dispatches per platform, `canvas-vello` is one generic `register<H: GraphicsOps>`. `canvas_core::ensure_wire_serde` + `register_ssr` are gone with the old External wire-serde registry. Flattened single-core |
| `graphql`, `sync` | plain `runtime-core` dep, no handlers of their own | `sync` entity types now additionally require `PartialEq` — the world kernel's `Signal<T>` is an equality-guarded slot |

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

### The old core's output is frozen

Several of those gates were *cross-core* by construction — they rendered
the same scene on both cores in one process and compared the results.
That form cannot outlive the old core's deletion, so the old core's
output has been **frozen to committed artifacts** and each suite now
compares the new core against the frozen files:

| Frozen artifact | Location |
| --- | --- |
| pixel-exact CPU framebuffers (PNG) | `crates/backend/cpu/tests/goldens/` |
| cell-exact terminal grids | `crates/backend/terminal/tests/goldens/` |
| byte-exact Roku command streams | `crates/backend/roku/tests/goldens/` |
| byte-exact SSR `html` + `head_css` | `crates/backend/ssr/tests/goldens/` |
| byte-exact email output (incl. the real idea-ui-mail welcome template) | `crates/backend/email/tests/goldens/` |
| wire catch-up snapshot JSON | `crates/dev/mock-backend/tests/goldens/` |
| the whole website's SSG output (33 routes + served doc) | `websites/website/tests/goldens/ssg/` |
| backend op-log streams (structural + full-op) | `crates/dev/scene-parity/goldens*/` |

The mechanism is `crates/dev/parity-goldens`
(`IDEALYST_FREEZE_GOLDENS=1` re-derives from the old core; the check
always runs). Each directory has a `README.md` with its corpus table and
regeneration command. **Once `runtime-core` is gone, a "regeneration"
can only re-baseline against the new core** — permanently discarding the
old core's testimony — so a mismatch is a bug to fix, not an artifact to
rewrite.

The pre-deletion record — frozen corpora, per-backend
default-resolved-method lists, green test counts, and the classification
of tests that legitimately die — is
[`runtime-v2-deletion-baseline.md`](runtime-v2-deletion-baseline.md).

Status at time of writing (all from checked-in gates):

- **Conformance:** the robot-driven cross-platform suite passes 8/8 on
  the new core (primitives, modal, stack-nav, `#[method]` suites) vs
  the old core's 8/9 — the one old-core failure is pre-existing and
  unrelated (`crates/dev/robot-e2e/examples/conformance`). The idea-ui
  suite's new-core leg is **[in flight]** with the idea-ui retarget.
- **Performance:** all 11 js-framework-bench ops are within the ±5 %
  gate or better, including `create_1k`/`create_10k` (which the runtime
  v2 build now wins outright after the prim-payload boxing fix) and
  `teardown_10k`. Every interactive-update path — granular bumps,
  shared/point restyles, signal-class flips, hierarchy updates,
  teardown — is at old-core parity. The archived table and the residual
  profile are in `benchmark/idealyst-native/README.md`; the old core is
  gone, so those numbers are a historical record, not a re-runnable
  gate.
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

The migration split the old `runtime-core` in two, then deleted the
walker half:

- **`runtime-shared`** — the permanent substrate: the style engine
  (`StyleRules`, stylesheets, tokens, theme plumbing), colors,
  assets/fonts (`typeface!`/`face!`), animation types,
  touch/hover/wheel/file-drop event types, the legacy reactive arena
  (`Signal`/`effect`/`memo`, `Ref`/`node_ref!`), scheduling / time /
  session, viewport + breakpoints + safe-area, logging, debug counters
  (`debug-stats`), the robot registry + bridge, native introspection,
  page metadata, the host slots (platform / color scheme / URL opener /
  fullscreen / announcer), and every per-primitive prop/handle struct
  (`primitives::*`). **This is what the backends depend on** — they take
  `runtime-shared` + `runtime-scene` + `runtime-vocabulary` directly and
  never the author root.
- **The walker half** — `Element`, the 159-method `Backend` mega-trait,
  the render walker, the `Bound`/builder authoring layer, the External
  table, `batch`, and `LegacyBridge` — is **gone** (~34 k lines). Its
  successors are `runtime-scene` (Element/realize/Host/Registry),
  `runtime-world` (the reactive kernel) and `runtime-vocabulary` (the
  thirty capability traits + the built-in primitive handlers).

Every thread-local moved WITH its module, so `runtime-shared` owns a
single authority. (A second copy would silently split state — signals
set through one path invisible to the other.)

### `runtime_core::` is the author-facing path, and stays that way

Thousands of doc, example, and test references spell the framework
`runtime_core::…`. That spelling is **preserved deliberately**: the
`runtime-core` package still exists at `crates/runtime/core`, but it is
now a paper-thin root that re-exports `runtime_vocabulary::glue::*` plus
the macro set. **No app or SDK import changed** — `use runtime_core::…`
resolves exactly as before.

During the migration this root lived in a separate `runtime-facade`
package reached through an `extern crate runtime_facade as
runtime_core;` alias at each consumer's crate root (a crate-root
`extern crate … as …` shadows the extern prelude for the whole crate).
That package and all 105 alias lines are gone; `runtime_core` is the
real crate again.

**Where the author surface lives**: in `runtime_vocabulary::glue`, not
in `runtime-core`. Extend glue — with vocabulary-suite tests when the
extension carries logic — so the items sit next to the machinery they
wrap. The `runtime-core` root holds only what a module re-export cannot
carry: the proc-macro set (`ui!`, `#[component]`, `#[props]`,
`stylesheet!`, `#[lazy_component]`, …) and the `#[macro_export]` decl
macros whose `$crate::…` expansions need a root (`rx!`, `effect!`,
`timeline!`, `animated!`, `typeface!`, `face!`, `node_ref!`).

A consumer crate that uses the macros also needs `runtime-vocabulary` as
a direct dependency: the macro expansions emit absolute
`::runtime_vocabulary::glue::…` paths.

### Other layout notes

- **`legacy-bridge` feature (`runtime-vocabulary`) — deleted.** It gated
  `bridge::LegacyBridge` (mounting the new core through an old `Backend`
  impl) and the `NavigatorOps::create_navigator` cap (whose
  `NavigatorHost` closed over the old `Element`). Both went with the
  walker. `create_navigator` did NOT fall back to a default — the method
  ceased to exist, and every backend deleted its delegation rather than
  re-defaulting it. Navigators mount through
  `runtime_vocabulary::handlers::navigator` over the Lifecycle/View caps.
  The bridge's delegation proof lives on as
  `runtime-vocabulary/tests/caps_conformance.rs`, which drives the same
  thirty caps + seven `Host` ops over `crates/dev/host-mock`.
- **`new-core` / `old-core` cargo features — deleted everywhere**,
  including `runtime-macros`'s lowering switch. There is one core, one
  lowering: `ui!` / `jsx!` / `#[component]` / `#[props]` always emit at
  `runtime_scene::Element` + `runtime_vocabulary` builders +
  `runtime_world` reactivity. A `features = ["new-core"]` dep line is
  now an unknown-feature error.

### Where a backend's mechanism lives

A backend used to carry TWO surfaces: the `Element`-walking
`impl runtime_core::Backend` (the mechanism) and a `newcore.rs` whose
`Host` + 30 caps impls UFCS-delegated into it. The mega-trait's deletion
collapses that into one. Two shapes are in use, and both are correct:

- **Mechanism folded into the capability impls** (ssr, email, terminal,
  cpu, roku, linux, windows, backend-web, the Apple/Android backends).
  One home per primitive family, which is the point of splitting a
  159-method trait. On the backends with a dispatch-site flush driver the
  fold is load-bearing rather than cosmetic: a method that takes an author
  callback wraps it (`flushing0`/`flushing1`/…) and then runs the
  mechanism, so the wrap and the body are one method.
- **Mechanism in inherent `<method>_impl` methods with the caps impls
  delegating** (`crates/dev/scene-parity`'s `FullRecorder`, and the five
  LIVE backends: backend-web, backend-macos, backend-ios-mobile,
  backend-android-mobile, render-wgpu). Right when the bodies cannot
  move to the caps impls without moving code out of the module that owns
  it: on those five the mechanism lives beside the rest of the platform
  code (`imp/mod.rs`, `backend_impl.rs`, `lib.rs`) and leans on that
  module's private imports and helpers, while `newcore.rs` — a different
  file — holds the capability impls. Moving ~440 bodies across that
  boundary would have re-homed platform code and re-imported dozens of
  symbols per crate; the bodies moved VERBATIM instead, which is what
  makes the swap provably behavior-preserving on the five backends that
  have **no frozen artifact corpus** to catch a regression
  (`runtime-v2-deletion-baseline.md` §2.9). Their dispatch-site flush
  wrappers stay in the caps impl, wrapping the author callback before
  calling the mechanism.

Methods a backend does not implement are simply ABSENT from its caps
impls — the caps-trait DEFAULT bodies serve them. Those defaults were
audited byte-for-byte against the `Backend` defaults they replaced
(`runtime-v2-deletion-baseline.md` §2.1). The four seam methods that had
a `Backend` default but are `Host`-**required** — `insert_at`,
`remove_child`, `create_anchor`, `supports_splice` — carry an explicit
body reproducing that default, and `supports_splice`'s value is pinned by
a literal assertion in each backend's suite (§2.2). That assertion is not
belt-and-braces: on the CPU and terminal corpora, flipping
`supports_splice` to `true` leaves every frozen framebuffer/grid dump
byte-identical, so the artifacts alone would not have caught it.
- **`css` and `runtime-layout`** depend on `runtime-shared` (they only
  ever needed the style data model, not the walker).

---

## Testing and the robot bridge

The robot surface (`idealyst test`, the MCP verbs, `robot-test`)
adapts verb-for-verb. What changes for test authors:

- **`test_id=`** works on all 14 registering primitives
  (`crates/runtime/vocabulary/src/robot.rs` mirrors the old registry's
  model 1:1 — same query semantics, same last-wins duplicate policy).
- **`#[method]`** requires the inline-props component shape (props as
  fn parameters; zero params is fine) — the legacy explicit-props form
  is a compile error (see table above). In robot builds
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
  `__mcp` inventory anchor is reachable two ways —
  `runtime_vocabulary::glue::__mcp` (where the retargeted `#[component]`
  emissions land) and `runtime_core::__mcp` (the root re-export, where
  the derive/`recipe!`/`doc_scope!` emissions land) — both the same
  `mcp-catalog` instance. A graph turns everything on with
  `runtime-core/dev` (= robot + catalog); the generated sidecar
  wrapper enables `runtime-core/dev`, and the sidecar session
  starts the bridge and installs
  `dev_server::newcore::install_robot_env` (vocabulary registry for
  element verbs, shared-bridge fallback for `get_catalog`/logs/
  customs via `runtime_shared::robot::bridge::install_verb_router`).
  Pinned by `dev-server/tests/newcore_robot_catalog.rs` and the
  build-runtime-server wrapper tests.
- **Catalog EMISSION has its own suite.** The 41-test emission battery
  (doc capture, `composes` edges walked out of real `ui!`/`jsx!` bodies,
  param/props schemas, tools, `#[method]` entries, `animated!` bindings,
  scopes, recipes, the JSON round-trip) lives at
  `crates/dev/newcore-catalog/tests/shared/catalog_emission.rs` and is
  compiled by `cargo test -p newcore-catalog`, so it exercises the
  anchors the retarget actually uses:
  `#[component]` resolves `::runtime_vocabulary::glue::__mcp` (its
  emission passes through `runtime_macros::finish`, which rewrites
  absolute `::runtime_core::` heads), while
  `#[derive(IdealystSchema)]` / `#[idealyst_tool]` / `recipe!` /
  `doc_scope!` resolve `::runtime_core::__mcp` through the crate root —
  those entry points do NOT go through `finish`. Anchor identity is
  pinned by `catalog_inventory_is_identical_across_cores`, a sorted
  fingerprint of every macro-emitted slice whose expected value is a
  literal in the suite source. While the pre-v2 crate still existed the
  SAME body was `include!`d by `crates/mcp/catalog/tests/registers_component.rs`
  so both lowerings were compared directly; that leg dies with the old
  anchor, which is why the shared body lives on the surviving side.
  (mcp-catalog cannot host the suite itself: `runtime-core` depends
  transitively on `mcp-catalog`, so a normal dep is a cargo cycle, and
  dev-dependencies cannot be optional.) Two constraints worth knowing
  when writing catalog fixtures: a `#[method]`-bearing component must
  use the **inline-props** form (the legacy explicit-props shape does not
  compile) and write `set(get() + n)` rather than `update`; and
  `view(..)` returns a builder, so a bare stub body needs
  `IntoElement::into_element(..)`.
- **`animated!` is mirrored by the vocabulary.** Like `rx!`/`effect!`/
  `timeline!`, the old `animated!` expands to `$crate::animation::…`
  against the pre-v2 root and constructs the SHARED `AnimatedValue`,
  whose inherent `bind*` anchors through the old `on_cleanup` and is
  dropped at bind time (the frozen-Switch-thumb bug). The
  `runtime-core` root re-exports `runtime_vocabulary::animated!`, which
  lands on the glue wrapper instead — so `animated!(0.0_f32)` keeps
  working verbatim.
- **The navigation recipes are static data too.** `list_recipes` /
  `describe_recipe` serve `swap_three_screens_tab_bar` and
  `stack_two_screens` from `runtime_shared::recipes` (sources at
  `crates/runtime/shared/recipes/`), not from the navigator SDKs. They
  used to be `recipe!` invocations inside those crates, gated off
  whenever runtime v2 was selected, so v2 served no navigation recipe at
  all. A `recipe!` body's `ui!` lowering is decided build-graph-wide and
  each SDK then carried an unconditional pre-v2 `runtime_core` dep (so
  `::runtime_core::Element` in a recipe body meant the OLD `Element`) —
  hence the move. The compile-and-realize
  gate is `crates/dev/newcore-app/tests/recipes_compile.rs`, which
  builds them against the SDKs' real public surface.

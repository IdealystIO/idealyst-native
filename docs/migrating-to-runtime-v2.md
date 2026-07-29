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

## Opting in: the `new-core` feature

`new-core` is **off by default** everywhere. Your app opts in with its
own forwarding feature; the canonical block is
`crates/dev/newcore-app/Cargo.toml`:

```toml
[features]
default = []
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

### idea-ui **[in flight]**

idea-ui and idea-theme compile the same component sources on the new
core through an alias crate: under `new-core`, `extern crate
runtime_facade as runtime_core;` swaps the old root for
`runtime-facade`, a thin re-export of `runtime_vocabulary::glue`
(`crates/runtime/facade/src/lib.rs`, `crates/ui/idea-ui/src/lib.rs`).
Enable it with default features off and re-enable the prim features you
need:

```toml
idea-ui = { version = "…", default-features = false, features = [
    "new-core",
    "prim-icon", "prim-image", "prim-text-input",
    "prim-activity", "prim-portal", "prim-presence",
] }
```

The `table` feature cannot join a `new-core` build — the table SDK is
still old-core-authored, and idea-ui pins that with a `compile_error!`
(`crates/ui/idea-ui/src/lib.rs`):

> `idea-ui: the 'table' component feature cannot join a 'new-core'
> build (the table SDK is old-core-authored; its retarget is a later
> P6 wave).`

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
| GPU desktop | `host_winit::newcore::run(profile, skin, build)` / `run_with(profile, skin, register, build)` | `crates/gpu-backend/host/winit/src/newcore.rs` |

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
| `#[component(lazy)]` / `#[lazy]` | compile error — no chunk-mount prim yet | make the component eager |
| `web_view` | compile error — dispatches through the old-core WebView SDK | avoid on new-core builds |
| virtualizer `for i in count(sig)` sugar | compile error — generator-backend metadata is post-migration | the `flat_list(data=…, key=…, size=…, render=…)` tag works |
| `CardTabs` ui! sugar | compile error — rides the old `cardtabs!` macro | — |
| `#[method]` legacy explicit-props / generic form | compile error — `Bindable<H>` rides the old `Element` | the inline-props form works (see Testing below) |

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
- **SSR:** output is byte-identical to the old renderer across the
  6-scenario corpus, html and head CSS
  (`crates/backend/ssr/tests/newcore_byte_identity.rs`) — which is also
  the hydration-compatibility proof.

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

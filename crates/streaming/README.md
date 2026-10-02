# Remote components (spike)

A remote component (working name in code: "streamed"; the attribute will be `#[component(remote)]`) is ordinary Rust compiled to `wasm32`. An app downloads it as a **bundle**, runs it on-device in the [wasmi](https://github.com/wasmi-labs/wasmi) interpreter, and mounts it like any other component. The same mechanism covers server-driven UI (small bundles per screen) and OTA updates (one bundle per host build).

This directory is a spike. It proves the boundary works end to end and measures what it costs. Nothing here is published or depended on by the framework.

The spike holds **three models** of what runs inside a bundle:

- **Model A, a host-owned graph** (`abi`, `guest`, `host`, `spike/guest`). The bundle holds handles into the app's reactive graph and returns small node descriptions. Bundles are small, around 37 KB. But `ui!` can't run inside one: it expands to the framework's full author API, about 220 items, so model A would need a second implementation of that API.
- **Model B, the real framework in the bundle** (`spike/components`, `spike/fullguest`). The bundle compiles the actual `runtime-world`, `runtime-scene` and `runtime-vocabulary`. It renders through `dev-server`'s wire recorder, and the app replays the commands through `dev-client`, which is runtime-server sandboxed down to one subtree. `ui!`, `#[component]` and context work unchanged, but the app pays for the framework twice: once natively, once inside every bundle (545 KB for one component). See [Model B](#model-b-the-real-framework-in-the-bundle).
- **The bridged design, model B with the framework shared** (`spike/kernelguest`, `spike/remoteguest`). The bundle runs the same real framework code, but its reactive kernel is *bridged*: its signals, effects and scopes live in the app's own graph, and the tree it builds crosses to the app as data, to be realized by the app's own registry and backend. One graph, one backend; the bundle carries only its own code (147 KB for the same component). **This is the chosen direction.** See [The bridged design](#the-bridged-design).

What carries over from model A to model B: the loader, the manifest and import checks, prop contracts, `#[host_fn]`, live reload, and native host components.

| Crate | Role |
|---|---|
| `abi` (`stream-abi`) | The byte contract both sides link: import/export names, the `Wire` codec, `Node` descriptions, the `Manifest`. Zero dependencies, because it ships inside every bundle. |
| `guest` (`stream-guest`) | What a bundle links: handle-backed `signal` / `effect`, node builders, the `bundle!` export macro. |
| `host` (`stream-host`) | Loads a bundle into wasmi, checks its manifest, binds it to the app's reactive graph, and turns node descriptions into `runtime_scene::Element`s. |
| `macros` (`stream-macros`) | `#[host_fn]`. |
| `spike` (`stream-spike`) | Builds both spike bundles to wasm32 in its build script. Holds the end-to-end tests, the `measure` / `measure_full` examples, `stream-serve`, and the model-B host wrapper (`full.rs`). |
| `spike/components` | Model B's remote component (`RemoteCounter`): a plain crate of `#[component]`s that the app also links natively. |
| `spike/fullguest` | Model B's bundle: the wasm exports around `spike/components`. |
| `spike/kernelguest` | Test bundle for the kernel bridge: plain `runtime-world` code whose graph is the app's. |
| `spike/remoteguest` | `spike/components`' `RemoteCounter` as a bridged remote component. |

## The boundary

**The host owns the only reactive graph.** A guest `signal()` creates a real `runtime_world` signal in the host's world; the guest holds a `u32` handle. A guest `effect()` is a real host effect whose body calls back into the guest. Because there is one graph, flushed once, a guest can never see host state a frame late, and each bundle doesn't carry its own copy of the kernel. The cost is a host call per signal access (measured below).

**Closures stay in the guest.** They cross as callback ids. When the owning scope tears down, the host releases each id (`stream_drop`).

**Props are a named, typed contract.** Each component declares its props in `bundle!`, and the declarations go into the manifest as a schema: name, type tag, and whether the prop is required or has a default. The app passes props by name with `HostProps`. Host signals can be passed in read-only (`read_signal`) or two-way (`signal`). A read-only handle has no setter on the guest side, and the host also refuses writes to it.

**Prop drift is checked before mount.** It works like a server function's `IncompatibleVersion`, except the check compares individual props instead of a whole-function hash. A hash would also reject harmless changes, such as adding an optional prop. `Bundle::check(name, &props)` and `Bundle::try_mount` return a `MountError` that names every problem, and nothing runs in the guest. The rules match what would compile at a native call site:

| Bundle change | Result |
|---|---|
| A prop's type changed | Incompatible: `TypeChanged` |
| New required prop | Incompatible: `MissingRequired` |
| New prop with a default | Compatible; the guest uses its default |
| Prop removed | Compatible; the app's value is dropped |
| `Signal<T>` → `ReadSignal<T>` | Compatible; the guest gets the read-only view |
| `ReadSignal<T>` → `Signal<T>` | Incompatible, because it would give the guest write access the app never granted |

The app decides what to do with a `MountError`. The demo checks a fetched bundle against the exact props it is about to mount with, `stream_spike::demo_props`. If the check fails, it keeps running the previous bundle and shows the mismatch. To run the same check from a terminal against the running server:

```sh
cargo run -p stream-spike --example check_served
```

**App functions are `#[host_fn]`s** (`stream-macros`). This is the `#[server]` shape pointed the other way. You define the function once, in a crate both the app and the bundle depend on (`spike/camera` is the example):

```rust
#[host_fn] pub fn battery_level() -> f64 { … }
#[host_fn] pub async fn take_photo(opts: PhotoOptions) -> Result<Photo, CameraError> { … }
```

What it compiles to:

- **In the app:** the real function, plus `take_photo::export()`. The app lists what bundles may call: `HostExports::new().host_fn(take_photo::export())`. That list is an allowlist, so an OTA bundle can't start using a capability the app never offered it.
- **In a bundle:** a stub that calls one wasm import, named `<module path>::<fn>#<signature hash>`. The bundle's import section therefore lists exactly the host functions it calls. The linker writes that list, and drops unused stubs. At load, the host reads it and refuses the bundle with `MissingHostFunctions` or `IncompatibleHostFunctions`, before any guest code runs.

Sync calls return their value inline. An async `#[host_fn]` becomes a `HostCall<T>` in the bundle, run with `stream_guest::spawn_then(call, then)`, which has the same shape as the native `spawn_then(future, then)`. The host runs it under that same native `spawn_then`, so the guarantee is the same:

- The IO always completes.
- `then` runs only if the scope that started the call is still mounted. That scope is the component being built, or the component whose event handler started the call.

The bundle has no async executor of its own. Composing several async steps belongs inside one host function.

A bundle build is marked by `--cfg idealyst_stream_guest`, which the bundle build passes; it is deliberately not a cargo feature. Features unify, so one `cargo build` covering both the app and a guest crate would hand the app guest stubs in place of its own functions. A `--cfg` belongs to a single build, and a bundle is always its own build.

Native views stay native. The camera preview is a host component (`CameraPreview`) that the bundle places by name; frames never enter wasm.

**Host components are imported by stable name**, never by `TypeId`, since a `TypeId` only means something inside one compiled binary. A bundle declares its imports in its manifest. `Bundle::load` refuses a bundle whose imports the app doesn't export (`LoadError::MissingHostComponents`) before any component body runs. This is how an old app binary rejects a newer bundle.

**One wasm instance per bundle**, shared by every mount of every component in it.

### Invariants worth knowing

- **Guest effects are created right after the guest call that asked for them, not during it.** A kernel effect runs its body immediately, and that body is a guest call, which would borrow the wasmi `Store` twice. Nothing can observe the difference, because the component body's own writes are staged until flush anyway.
- **`Signal::update` re-enters the guest through the wasmi `Caller`.** It has to apply the guest's closure to the *staged* value, so two updates in one batch compose (0 → 1 → 2). A `set(get() + 1)` would lose one. The test `guest_updates_compose_within_one_batch_and_guest_effects_rerun` fails against that naive version.
- **A host flush inside a guest call would break the above.** It panics with a diagnostic rather than deadlocking.
- **Signal handles are never reused**, so a stale guest handle can't point at a later signal.

## Running it

```sh
cargo test -p stream-abi -p stream-spike
cargo run --release -p stream-spike --example measure        # model A; host MUST be --release
cargo test -p stream-spike --test full_framework              # model B end to end
cargo run --release -p stream-spike --example measure_full   # model B
cargo test -p stream-spike --test kernel_bridge               # the bridged kernel over wasm
cargo test -p stream-spike --test remote_counter              # a ui! component, bridged, vs native
cargo test -p runtime-world --features loopback-engine       # every kernel test, through the bridge
cargo test -p runtime-vocabulary --features remote-loopback --test remote_elements  # the element codec
```

### See it live

```sh
cargo run --release -p stream-spike --bin stream-serve   # serves the bundle, rebuilds on save
cargo run --release -p stream-demo                        # AppKit window
```

1. Edit `spike/guest/src/lib.rs`.
2. Press **Refresh bundle**. The tinted sections remount from the new build. The app is not rebuilt or restarted.

How it behaves:

- **Host signals keep their values** across a swap; guest-local state starts over.
- **A failed build** shows the compiler error, and the app keeps running the previous bundle.
- **With no server running**, the app uses the bundle compiled into it.

The test `swapping_bundles_releases_the_old_instance_and_keeps_host_state` pins down the swap: the old instance's closures and handles are all released, and host state carries over.

## Model B: the real framework in the bundle

`RemoteCounter` is an ordinary component:

```rust
#[component]
pub fn RemoteCounter(title: String, external: ReadSignal<i64>) -> Element {
    let clicks = signal(0i64);
    let user = inject::<CurrentUser>().map(|u| u.0);
    ui! { view() { text { "{title}" } text { "external: {external}" } … if let Some(user) = user { text { "signed in as {user}" } } } }
}
```

The bundle mounts it in a `dev_server::newcore::SceneSession` against a `WireRecordingBackend`. It returns `DevToApp::Commands` batches, which the app replays through `dev_client::WireBackend` into its own backend.

- **The bundle has its own world**, so host state crosses as mirrors. A host signal prop, like `external`, becomes a signal in the bundle that the app updates when its own signal changes. The bundle then flushes and returns the delta in the same turn.
- **Context works the same way:** the app's provided `CurrentUser` is mirrored, `provide`d at the root of the bundle's world, and read with a plain `inject`. It stays reactive.
- **Taps:** the replay client reports the tapped button's `HandlerId`, and the bundle dispatches it.
- **Unmount:** the bundle frees its world, and the app removes the replayed subtree under its own mount point. The recorder emits no teardown commands, because in runtime-server teardown is the whole session.

The test `bundle_emits_exactly_what_the_native_build_emits` shows the bundle *is* the framework. Mounting the same component natively against the same recorder produces a byte-identical command stream. The bundle imports nothing from the host.

Making this possible changed one framework crate: **`dev-server` gained a default-on `session` feature.** It covers the sidecar, the websocket transport, the file watcher and the test harness. With the feature off, the crate is just the recorder, which builds for `wasm32-unknown-unknown`; tungstenite's handshake pulls in `getrandom`, which doesn't. Existing consumers are unchanged, and `dev-server`'s tests pass.

### Model B measurements

M3 Max, release, portable dispatch, replayed into `mock-backend`. Mount and update times include the JSON wire codec and the replay.

| | Bundle | Native + wire | Native |
|---|---|---|---|
| Size | 545 KB raw, 454 KB after wasm-opt, **148 KB brotli** | — | — |
| Load, lazy translation (eager) | 1.9 ms (5.9 ms) | — | — |
| Mount `RemoteCounter`, cold | 2.2 ms | — | — |
| Mount, warm | 590 µs | 41 µs | 10.5 µs |
| Host prop change → replayed | 31 µs | — | 0.6 µs |
| Tap → replayed | 34 µs | — | — |
| Context change → replayed | 33 µs | — | — |

The "native + wire" column isolates the cost of recording, the JSON codec and replay, about 30 µs of a mount. The rest of the gap is the interpreter running framework code.

### What building model B found

1. **wasmi's default tail-call dispatch can overflow the native stack.** That dispatch keeps the stack flat only if LLVM turns every handler call into a sibling call. Whether it does depends on how wasmi *and its dependencies* are compiled. In the workspace dev profile, the model-B bundle overflowed a 2 MB thread with wasmi at opt-level 3 and passed at `"z"`. wasmi picks tail calls from opt-level alone, so an app's profile choices could turn a big bundle into a stack-overflow crash. iOS's main thread has a 1 MB stack. The host therefore uses **`portable-dispatch`**, a loop that never grows the stack. Its cost, measured:

   | | Tail-call dispatch | Portable dispatch |
   |---|---|---|
   | Model B warm mount | 348 µs | 590 µs |
   | Model B prop update | 16.5 µs | 31 µs |
   | Model A typed signal read | 98 ns | 155 ns |
   | Pure compute vs native | 3.9× | 18.7× |

   `regression_full_framework_bundle_fits_a_2mb_thread` pins this. Tail calls could become an opt-in only alongside a CI check that runs a large bundle on a 1 MB thread in the exact shipping profile.
2. **A bundle crate must be `cdylib` only.** Built as both `cdylib` and `rlib`, it lost link-time optimization: 668 KB instead of 545 KB. Components therefore live in an ordinary crate (`spike/components`) with a thin `cdylib` wrapper around it, which is the shape the CLI would generate.
3. **A workspace-inherited dependency can't turn default features off** unless the workspace entry does. `dev-server = { workspace = true, default-features = false }` silently kept the session layer, so `spike/fullguest` uses a path dependency.

## The bridged design

A bundle is built with `--cfg idealyst_stream_guest` (a build flag, not a cargo feature, so it can never leak into an app through feature unification). Under that flag two things change, and nothing else:

**The kernel is bridged** (`runtime-world/src/bridge`). runtime-world's typed layer runs on an `Engine`; natively that is the arena, in a bundle it is `Bridged`. `Bridged` keeps what cannot leave the bundle (signal values, effect bodies, cleanups, context values) in local tables, and forwards every graph operation (slots, subscriptions, staging, flush, scopes, context stacks) to the app over wasm imports. The app holds a proxy in each slot the bundle owns. So a bundle's signal is a slot in the app's arena, the app's flush runs the bundle's effects, and the app's scopes own them.
- **Props and context the app owns** cross as handles: the app exports a signal (`stream_host::kernel::export_signal` / `export_read_signal`) and the bundle imports it (`runtime_world::remote_guest::import_signal`); reads subscribe in the app's graph. Context is declared by name on both sides (`export_context` / `register_remote_context`); context the app did not declare is invisible to bundles.
- **Parity is enforced, not hoped for.** Adding an `Engine` method doesn't compile until every engine has it, and `--features loopback-engine` runs the entire kernel suite through the bridge in one process.

**The tree crosses as data** (`runtime-vocabulary/src/remote`, feature `remote`). The bundle encodes the `Element` it built into a `Node`. Every closure in it (a dynamic text getter, an `on_press`, a `Dyn` hole's builder, a keyed list's items and render, a stylesheet) stays in a bundle-side table under a callback id, and the id crosses instead. The app decodes the `Node` into real primitives whose closures call the bundle by id, then realizes it like any other tree. When the app drops such a closure it releases the id, and the bundle frees the closure.
- A component's `Owned` crosses as the app-side scope id it already is, and the app claims it (`runtime_world::remote::claim_scope`), so unmounting tears the bundle's state down like a native component's.
- **Styles keep the app's theme.** Style rules cross with token names intact, so the app's theme resolves them. A stylesheet crosses as its shape, and the app rebuilds the same sheet with each closure calling back into the bundle. State, breakpoint and container overlays then work exactly as for a native sheet. Ids are refcounted, so a sheet shared by many nodes crosses once.
- **App components are imported by name.** A bundle calls `remote::bundle::import("Card", &props, children)`; the app exports `Card` with `remote::host::register_import`. A bundle that needs a component the app doesn't export fails to decode with `MissingImport`, rather than rendering half a tree.
- **Every builtin primitive has a decision.** `remote::crossing` says whether each payload crosses; a test fails when `register_builtins` gains one with no entry. Crossing today: `view`, `pressable`, `text`, `button`. Everything else, and unsupported fields of crossing primitives (`on_touch`, `ref`, icons, styled runs), panics at encode, naming itself.

What the tests prove (`remote_counter.rs`): the real `RemoteCounter`, written with `#[component]` and `ui!`, mounted from the bundle drives the app's backend through exactly the same calls as the native build, through mount, button presses (bundle state), a prop change and a context change (app state), and unmount. After unmount, no bundle callback or scope is left behind.

## Model A measurements

Apple M3 Max, host-mock scene, medians. The guest is the 28 KB `spike/guest` (release, opt-level z, no wasm-opt). "Native" is the same component compiled into the binary. These numbers were taken with wasmi's tail-call dispatch, before the switch to portable dispatch; see the dispatch table above for that cost.

| | Streamed | Native |
|---|---|---|
| Load (validate + instantiate + manifest), default lazy translation | 273 µs | — |
| Load, `CompilationMode::Lazy` | 143 µs | — |
| Load, eager translation | 487 µs | — |
| First mount of `Counter` (5 nodes, 2 reactive texts, 1 effect), cold | 120 µs | — |
| Same mount, warm | 18 µs | 5.6 µs |
| Host `set` → flush → guest text re-rendered | 2.0 µs | 0.5 µs |
| One signal read across the boundary, typed | 98 ns | 24 ns |
| One signal read, raw crossing with no decoding | 57 ns | — |
| Pure compute (20 M xorshift steps) | 3.9× slower | 1× |

### What the measurements changed

1. **wasmi must be built at `opt-level = 3`.** The workspace release profile is size-optimized (`"z"`). Built that way, wasmi was about 2.5× slower in every phase. The root `Cargo.toml` now overrides opt-level for the wasmi crates and `wasmparser`. Cargo only reads profiles from the root workspace, so **an app that streams components needs the same lines in its own `Cargo.toml`.**
2. **The guest's wasm stack should be small.** rustc defaults the wasm32 stack to 1 MB, so the module's initial memory was 17 pages, and the interpreter zeroes all of it on instantiate. The spike's build script links the guest with `-zstack-size=65536`, which brings initial memory down to 2 pages.
3. **Formatting code is a big part of bundle size.** One `format!("{:.0}", f64)` pulled in float formatting and took the bundle from 29 KB to 66 KB. Integer math brought it back to 37 KB. Avoid float and `{:?}` formatting in bundles, or move them to the host.
4. **The guest's read helper cost more than the crossing.** A thread-local `RefCell<Vec>` buffer added about 300 ns per read in the interpreter. Reading into a 64-byte stack buffer removed most of that. What remains of the typed read beyond the raw crossing is `Wire` decoding run by the interpreter.

## Not covered yet

- **On-device numbers.** These were measured on a Mac. An iPhone run is the next measurement.
- **Props passed from a bundle to a host component** (`Node::Host`, e.g. `Badge`) are still positional and unchecked. The same schema mechanism applies in that direction.
- **Plain value props are fixed at mount** in model A. Model B mirrors props as signals, so `#[component(remote)]` can make every prop reactive by default, as `#[props]` does natively.
- **Model A only: most of the primitive vocabulary.** `Node` has `View`, `Text`, `Button` and `Host`. This doesn't apply to model B, which has the whole vocabulary.
- **The bridged design is hand-wired.** `spike/remoteguest`'s mount export is written by hand. `#[component(remote)]` should generate it (prop imports, context registration, encode), plus a manifest listing the app components and context names the bundle needs, checked before mount.
- **The bridged design carries four primitives.** The rest of the vocabulary (`crossing` lists each), event handlers on `view`, `ref`s and handles still have to cross.
- **Model B's wire codec is JSON.** (The bridged design's element codec is postcard.)
- **Nested bundles**, where one bundle mounts another bundle's component by name.
- **Guest trap handling.** It's a panic for now. Whether a trap should be contained to the bundle in release builds is still an open decision.
- **Host handles.** Large or native results (a photo, a capture session) should cross as scoped handles, not bytes. The spike's `Photo` is a small value.
- **Host component imports are still hand-listed** in `bundle!`. They could use the same import-section trick as `#[host_fn]`.
- **Bundle signing, and caching compiled modules by content hash.**

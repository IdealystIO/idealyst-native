# Remote components (spike)

A remote component (working name in code: "streamed"; the attribute will be `#[component(remote)]`) is ordinary Rust compiled to `wasm32`. An app downloads it as a **bundle**, runs it on-device in the [wasmi](https://github.com/wasmi-labs/wasmi) interpreter, and mounts it like any other component. The same mechanism covers server-driven UI (small bundles per screen) and OTA updates (one bundle per host build).

This directory is a spike. It proves the boundary works end to end and measures what it costs. Nothing here is published or depended on by the framework.

The spike holds **two designs** of what runs inside a bundle:

- **Model A, a host-owned graph** (`abi`, `guest`, `host`, `spike/guest`). The bundle holds handles into the app's reactive graph and returns small node descriptions. Bundles are small, around 37 KB. But `ui!` can't run inside one: it expands to the framework's full author API, about 220 items, so model A would need a second implementation of that API.
- **The bridged design** (`spike/kernelguest`, `spike/remoteguest`, `spike/remoteattr`, `example`). The bundle runs the same real framework code, but its reactive kernel is *bridged*: its signals, effects and scopes live in the app's own graph, and the tree it builds crosses to the app as data, to be realized by the app's own registry and backend. One graph, one backend; the bundle carries only its own code (147 KB for `RemoteCounter`). **This is the chosen direction.** See [The bridged design](#the-bridged-design).

A third design, model B, ran a private copy of the framework inside each bundle; it was removed once the bridged design replaced it — see [History](#history-model-b).

What can carry over from model A to the bridged design: the manifest and import checks, prop contracts, `#[host_fn]`, and native host components.

| Crate | Role |
|---|---|
| `abi` (`stream-abi`) | The byte contract both sides link: import/export names, the `Wire` codec, `Node` descriptions, the `Manifest`. Zero dependencies, because it ships inside every bundle. |
| `guest` (`stream-guest`) | What a bundle links: handle-backed `signal` / `effect`, node builders, the `bundle!` export macro. |
| `host` (`stream-host`) | Loads a bundle into wasmi, checks its manifest, binds it to the app's reactive graph, and turns node descriptions into `runtime_scene::Element`s. |
| `macros` (`stream-macros`) | `#[host_fn]`. |
| `spike` (`stream-spike`) | Builds the spike's bundles to wasm32 in its build script. Holds the end-to-end tests, model A's `measure` example and `stream-serve`. |
| `spike/components` | `RemoteCounter`: a plain crate of `#[component]`s the app also links natively (the parity baseline). |
| `spike/kernelguest` | Test bundle for the kernel bridge: plain `runtime-world` code whose graph is the app's. |
| `spike/remoteguest` | `spike/components`' `RemoteCounter` as a bridged remote component, with a hand-written mount export. |
| `spike/remoteattr` | `#[component(remote)]` end to end: a remote component using an app component. |
| `example/app`, `example/bundle` | An app and its remote components in one file. |

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

The window shows the **bridged** RemoteCounter (green, from `/remote.wasm`) next to the same component compiled into the app (grey), and model A's components below.

1. Edit `spike/components/src/lib.rs` (the bridged component) or `spike/guest/src/lib.rs` (model A).
2. Press **Refresh bundle**. The tinted sections remount from the new builds; the app is not rebuilt or restarted. The grey native copy keeps the old code, which is the point of comparison.
3. The host buttons drive host state the bridged component reads: `external ± 1` is a prop, **switch user** is context.

Crossing today: `view`, `pressable`, `text` (including styled runs), `button` (including icons), `image`, `icon`, `link`, `toggle`, `slider`, `activity_indicator`, `text_input`, `text_area` and `scroll_view`, with every event handler they take. Anything else panics in the bundle while it mounts, and the window shows the bundle's panic message in place of the component. A panic at any other time (a press handler, an effect) does the same; see [Panics](#panics-in-a-bundle).

How it behaves:

- **Host signals keep their values** across a swap; guest-local state starts over.
- **A failed build** shows the compiler error, and the app keeps running the previous bundle.
- **With no server running**, the app uses the bundle compiled into it.

The test `swapping_bundles_releases_the_old_instance_and_keeps_host_state` pins down the swap: the old instance's closures and handles are all released, and host state carries over.

## `#[component(remote)]`: an app and its remote components in one file

`example/app/src/main.rs` is a complete app: a native `App`, and a
`#[component(remote)] Scoreboard` it renders with the app's state as props.

```rust
#[component(remote)]
pub fn Scoreboard(player: String, score: ReadSignal<i64>, cheers: Signal<i64>) -> Element { … }
```

The file compiles twice. As the app, `Scoreboard`'s body is left out: the macro replaces it with a stub that sends the props and mounts the component from the installed bundle (`stream_host::remote::install`). As the bundle (`example/bundle`, which points at the same file; built by the app's build script and served by `stream-serve`), only the remote component's body compiles, plus a mount export. On web, `remote` is a no-op.

Props are taken as declared. A `ReadSignal<T>` crosses as a handle: the bundle reads the app's signal live. A `Signal<T>` lets the bundle write it too. A plain value is copied at mount. Your own value types cross with `runtime_vocabulary::remote_value!(MyType)`.

```sh
cargo run --release -p stream-spike --bin stream-serve   # terminal 1: serves /example.wasm, rebuilds on save
cargo run --release -p remote-example                     # terminal 2
```

Edit `Scoreboard`, press **Reload remote**: the remote section remounts from the new build, with the app's state intact. `tests/remote_attr.rs` covers the same path end to end.

### Remote code using app components

**Every component not marked `remote` lives in the app binary.** When remote code renders one (`Badge` in the example, idea-ui's components, anything), the bundle does not contain it: in a bundle build `#[component]` compiles its body out and replaces it with a stub that sends the props and asks the app for its own copy by name (`module_path::Name`). In a native app build with `remote` on, every component registers itself for that at link time (`runtime_vocabulary::remote::host::APP_COMPONENTS`). An app that doesn't have the component (an older binary) shows `MissingImport` in its place; props that don't decode show `BadProps`. **The bundle must compile the app's source under the app's crate name**, because the import name starts with it: `example/bundle` sets `[lib] name = "remote_example"`, and the app's build script fails the build if the two differ.

Props cross bundle → app with `ImportArg`:

| Prop | How it crosses |
|---|---|
| Values (`String`, numbers, `StyleRules`, your types via `remote_value!`) | Copied |
| `Reactive<T>` | Copied if static; a getter into the bundle if live |
| Callbacks (`Rc<dyn Fn()>`, `Rc<dyn Fn(A)>`, `Option<…>`) | Run in the bundle |
| Children, `Element` props | Encoded subtrees, built by the bundle |
| `Rc<StyleSheet>` | A proxied sheet, resolved by the app's theme |
| `Signal<T>` / `ReadSignal<T>` the app gave the bundle | The app's own handle |
| `Signal<T>` / `ReadSignal<T>` the bundle created | **Promoted**: the value moves into the app's arena |

A prop type with no `ImportArg` doesn't stop anything compiling: it fails by name at runtime, in the component's place.

**Promotion.** A bundle-created signal keeps its value in the bundle until the bundle hands it to native code. Then the app's prop decoder (which knows `T`) takes the value over: the slot keeps its subscribers and the bundle keeps its handle, but the value is now a real `SignalData<T>` in the app's arena, and the bundle reads and writes it the way it does any app signal. Native code reads it at native speed; a memo's output promotes too, and its derivation keeps writing the native value. It is two-phase (`GuestHooks::promote` / `promote_finish`), so a value the app can't decode leaves the bundle untouched. The native slot holds a `Promoted<T>` whose `as_any_mut` returns the inner `SignalData<T>`, so every native read, write and commit runs unchanged code — measured on the shipped profiles: no regression with `bridge` on or off.

**Web never compiles any of this.** `remote` is a no-op on web, and the bridge, the codec and the registrations are compiled only for native targets or a bundle build, whatever features an app enables.

## Refs: handles to nodes the app mounted

A remote component takes refs like any component (`text_input(..., bind = input)`, `on_handle`), and gets the same handle types (`TextInputHandle`, `ViewHandle`, …). A handle is a node plus a static ops table, so the bundle's handle is built with a forwarding ops table (`runtime_vocabulary::remote::handles::RemoteOps`, one type implementing every primitive's ops trait): each method crosses to the app as a `HandleCall` (over wasm, the `idealyst_ui.handle_call` import) and runs on the real handle, which the app holds from the moment the backend fills the ref. Layout subscriptions cross too (the app holds the bundle's callback), and so does a portal anchored to a node the bundle holds a ref to: the app's portal asks the bundle for the rect, and the bundle asks the app's handle.

The app's entry goes when the bundle drops its handle, and in any case when the tree that made it unmounts. The decoded root owns its connection, so the handle lives exactly as long as the tree, whether or not the bundle releases it (it may keep it in a `Ref` slot, or have been stopped). `tests/remote_elements.rs` (`refs`) checks every method reaches the backend's handle in the same order as natively; `tests/remote_attr.rs` checks it over wasm.

## Context

A remote component reads the app's context with a plain `inject`, for context types marked `#[remote_context]`:

```rust
#[remote_context]
#[derive(Clone)]
pub struct Theme { pub accent: ReadSignal<String>, pub compact: bool }
```

Fields cross like a remote component's props: a `ReadSignal` / `Signal` as a handle into the app's graph (read live, writable for `Signal`), a value as a copy. Each marked type registers itself — natively in a link-time slice, which the app's remote loader offers to bundles under the type's path; in a bundle (`linkme` has no wasm32 support) as an `__idealyst_ctx_<path>` export the loader calls at load to register its decoder. A bundle's `inject::<Theme>()` then falls back to the app's value when the remote tree provides none itself; the exports live as long as the component that asked. Unmarked context stays invisible to bundles. `tests/remote_attr.rs` covers it over wasm (live signal, unmarked context hidden, unprovided context absent).

## Navigation

A remote component can be a screen in the app's navigator, and navigate it. It gets the navigator's handle as a prop, as app screens do (`nav: Ref<StackHandle>`, `Ref<NavHandle>` or a `NavHandle`), and pushes, pops, selects and follows route links with typed routes, written exactly as natively:

```rust
#[component(remote)]
pub fn Home(nav: Ref<StackHandle>) -> Element {
    ui! { button(label = "open", on_click = move || if let Some(h) = nav.get() { h.push(&DETAIL, ItemId(7)) }) }
}
```

- **Typed params cross as the url.** A route's params are values of the bundle's build of the type, which the app's navigator can't downcast, so a command crosses as its route name, url and query, and the navigator rebuilds the params from the url with the screen's own `from_segments` — exactly as for a deep link (`ParamsFromUrl`). The routes must be the same on both sides (one shared definition, as for any component).
- **A `Ref` crosses as the ref.** A screen is built before its navigator fills the ref, so the app keeps its `Ref` in the handle table and reads it when the bundle navigates, as native code reads its ref at call time. Handing the ref on to an app component (a header's back button) gives that component the app's original `Ref`.
- `StackHandle` / `SwapHandle` cross as themselves too (`Option<StackHandle>` as a prop — a component prop needs a `Default`, natively as well): each SDK invokes `runtime_vocabulary::__remote_nav_handle!` once, because the impls can't be one blanket impl in the vocabulary (it would overlap the other prop impls) and a plain impl in the SDK couldn't follow the vocabulary's build flags.

`tests/remote_attr.rs` (`a_remote_screen_navigates_the_apps_navigator`) runs it over wasm: a remote home screen in an app stack pushes `/items/7`, pops, follows a route link to `/items/9`, and hands its ref to an app component that pops.

**A remote component can also define a navigator** — its screens, their typed params, its layout and the screens' chrome options are bundle code; the navigator machinery (stack, url sync, mount policy, presentation) is the app's, as for any navigator:

- Each screen's params stay in the bundle as held items: its `from_segments` makes one from the url's segments, its `build` consumes one (the initial `Route<()>` screen is built with unit params). Commands from the bundle cross with `ParamsFromUrl`, which the navigator resolves through those same `from_segments`.
- The layout reads `StackNav` / `SwapNav` from context as natively: the app's layout closure (which runs inside the navigator's `provide`) exports them — signals as handles, `pop` / `on_select` as app closures the bundle calls — and the bundle provides its own copy around the author's layout. Both live as long as the layout.
- A screen's chrome options stay in the bundle; the layout reads them back through `screen_chrome`.
- `on_handle` gets a handle to the app's real navigator. A tree's handles are dropped when it unmounts (a navigator's handle reaches its screens' callbacks, so waiting for the tree's connection to drop would be a cycle).

`tests/remote_attr.rs` mounts one stack navigator and one swap navigator definition natively and from the bundle and requires the same screens after every step (typed push, pop, route link; tab selects), and nothing left after unmount.

## Calling native code: `#[host_fn]`

A remote component calls native code through `#[host_fn]` (`stream-macros`). One definition, in a crate both builds see:

```rust
#[host_fn]
pub fn battery_level() -> f64 { … }
#[host_fn]
pub async fn take_photo(opts: PhotoOptions) -> Result<Photo, CameraError> { … }
```

In the app it is the function as written. In a bundle it becomes a stub that asks the app to run it: a sync call answers inline, and an async one returns a future (`runtime_vocabulary::remote::bundle::HostFuture`) that the framework's own `spawn_then` drives, the same call shape as natively. The app runs the real future on its own executor and delivers the result to the bundle as a one-shot callback. Inside the bundle a small executor (`remote::wasm`, installed at load) polls the futures as results arrive.

The app lists what bundles may call: `stream_host::remote::install_with(wasm, vec![take_photo::export(), …])`. A bundle that calls anything else is refused at load with `MissingHostFunctions`, naming it; one whose signature changed since the app was built (the import name carries a fingerprint of the signature) with `IncompatibleHostFunctions`. A result that arrives after its bundle was stopped is dropped.

Plain functions are always compiled into the bundle; only `#[host_fn]` crosses to the app. `example` has one (`os_name`); `tests/remote_attr.rs` covers sync, async, the load-time refusals and a result for a stopped bundle.

Model A bundles build with an extra `--cfg idealyst_stream_model_a` (in their own target dir) and keep model A's stub.

## Panics in a bundle

A panic in a bundle never takes the app down. A Rust panic in wasm is `panic = "abort"`: it traps the interpreter, and no destructor in the bundle runs, so a `RefCell` it held stays borrowed and its tables stay half-updated. So the first trap **poisons** the bundle (`stream-host`, `kernel.rs`):

1. The call that trapped, and every later call into that bundle, answers nothing. Each kind of call has a safe answer: an effect doesn't run, a commit reports no change, a handler does nothing, a getter returns its last value, a `Dyn` hole or keyed row renders nothing, a style getter resolves to defaults (`runtime_vocabulary::remote::host`).
2. The loader is told (`KernelBundle::on_poison`) and remounts every remote component. The poisoned bundle refuses the mounts, so each shows the bundle's own panic message (kept by the bundle's panic hook) in its place, and its old tree is torn down.
3. The app's own UI and state carry on. Reloading a new bundle brings the components back.

`tests/remote_attr.rs` covers a panicking press handler, a panicking effect and recovery by reload, against a real wasm bundle. Each regression test was checked to fail with the old "trap panics the app" code.

## The bridged design

A bundle is built with `--cfg idealyst_stream_guest` (a build flag, not a cargo feature, so it can never leak into an app through feature unification). Under that flag two things change, and nothing else:

**The kernel is bridged** (`runtime-world/src/bridge`). runtime-world's typed layer runs on an `Engine`; natively that is the arena, in a bundle it is `Bridged`. `Bridged` keeps what cannot leave the bundle (signal values, effect bodies, cleanups, context values) in local tables, and forwards every graph operation (slots, subscriptions, staging, flush, scopes, context stacks) to the app over wasm imports. The app holds a proxy in each slot the bundle owns. So a bundle's signal is a slot in the app's arena, the app's flush runs the bundle's effects, and the app's scopes own them.
- **Props and context the app owns** cross as handles: the app exports a signal (`stream_host::kernel::export_signal` / `export_read_signal`) and the bundle imports it (`runtime_world::remote_guest::import_signal`); reads subscribe in the app's graph. Context is declared by name on both sides (`export_context` / `register_remote_context`); context the app did not declare is invisible to bundles.
- **Parity is enforced, not hoped for.** Adding an `Engine` method doesn't compile until every engine has it, and `--features loopback-engine` runs the entire kernel suite through the bridge in one process.

**The tree crosses as data** (`runtime-vocabulary/src/remote`, feature `remote`). The bundle encodes the `Element` it built into a `Node`. Every closure in it (a dynamic text getter, an `on_press`, a `Dyn` hole's builder, a keyed list's items and render, a stylesheet) stays in a bundle-side table under a callback id, and the id crosses instead. The app decodes the `Node` into real primitives whose closures call the bundle by id, then realizes it like any other tree. When the app drops such a closure it releases the id, and the bundle frees the closure.
- A component's `Owned` crosses as the app-side scope id it already is, and the app claims it (`runtime_world::remote::claim_scope`), so unmounting tears the bundle's state down like a native component's.
- **Styles keep the app's theme.** Style rules cross with token names intact, so the app's theme resolves them. A stylesheet crosses as its shape, and the app rebuilds the same sheet with each closure calling back into the bundle. State, breakpoint and container overlays then work exactly as for a native sheet. Ids are refcounted, so a sheet shared by many nodes crosses once.
- **App components are imported by name.** A bundle calls `remote::bundle::import("Card", &props, children)`; the app exports `Card` with `remote::host::register_import`. A bundle that needs a component the app doesn't export fails to decode with `MissingImport`, rather than rendering half a tree.
- **Every builtin primitive has a decision.** `remote::crossing` says whether each payload crosses; a test fails when `register_builtins` gains one with no entry. Crossing today: `view`, `pressable`, `text` (including styled runs), `button` (including icons), `image`, `icon`, `link`, `toggle`, `slider`, `activity_indicator`, `text_input`, `text_area` and `scroll_view`, with every event handler they take (touch, wheel, hover, file drop, key, focus, blur, scroll, image load and error). Event handlers cross as callbacks whose event and reply are encoded; a stopped bundle's handlers answer the platform default. Everything else panics at encode, naming itself. `tests/remote_elements.rs` (`controls`) checks every one against the native build, handler by handler. The structural primitives cross too: `repeat`, `presence`, `portal` (viewport and named targets), `virtualizer` (including measured sizes and `item_diff`, whose snapshots stay in the bundle) and `virtual_grid`; their builders, sizes and keys are callbacks the app's backend calls, and `tests/remote_elements.rs` (`structural`) drives them the way a backend does.

What the tests prove (`remote_counter.rs`): the real `RemoteCounter`, written with `#[component]` and `ui!`, mounted from the bundle drives the app's backend through exactly the same calls as the native build, through mount, button presses (bundle state), a prop change and a context change (app state), and unmount. After unmount, no bundle callback or scope is left behind.

## History: model B

Model B compiled the real `runtime-world`, `runtime-scene` and `runtime-vocabulary` into each bundle, rendered through `dev-server`'s wire recorder, and had the app replay the commands — with the bundle's own private world, so app state crossed as mirrored copies. It proved real framework code runs unchanged in a bundle (byte-identical commands to the native build), and showed the cost: 545 KB raw / 148 KB brotli for one component, warm mount 590 µs vs 10.5 µs native. The bridged design replaced it; it was deleted after the kernel bridge made its private-world build impossible. Its code is in git history (`5c1bdc9c`). What it found still holds:

1. **wasmi's default tail-call dispatch can overflow the native stack.** It keeps the stack flat only if LLVM turns every handler call into a sibling call, which depends on how wasmi and its dependencies are compiled: with wasmi at opt-level 3, a large bundle overflowed a 2 MB thread. iOS's main thread has 1 MB. The host uses **`portable-dispatch`**, a loop that never grows the stack, at a measured cost of 1.7–1.9× on UI work and 4.8× on pure compute. `regression_bundle_runs_on_a_2mb_thread` (`tests/remote_attr.rs`) pins it, and was checked to overflow with tail-call dispatch at opt-level 3.
2. **A bundle crate must be `cdylib` only.** Built as both `cdylib` and `rlib`, a bundle lost link-time optimization (668 KB instead of 545 KB). Components live in an ordinary crate with a thin `cdylib` wrapper — or, as in `example`, a bundle package whose `lib` points at the app's source.

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
- **Plain value props are fixed at mount.** A live prop is declared `ReadSignal<T>`; `#[component(remote)]` could make plain props reactive by default, as `#[props]` does natively.
- **No manifest.** A manifest listing the props, app components and context names a bundle needs, checked before mount, is still to do (by choice: see where things break first). `remote` can't be combined with `lazy` yet.
- Context still needs the hand-written registration `spike/remoteguest` shows. A manifest listing the props, app components and context names a bundle needs, checked before mount, is still to do. `remote` can't be combined with `lazy` yet.
- **Every builtin primitive crosses except `graphics`** (and `lazy`, which has no meaning in a bundle). `crossing` records each decision. `graphics` never will: it hands the author's code a native GPU surface, which interpreted wasm can't drive — draw in an app component and use that from the remote component.
- **Nested bundles**, where one bundle mounts another bundle's component by name.
- **A poisoned bundle can keep running in one case.** If a host→bundle call made *from inside* a bundle call traps (a bundle calling `flush`, whose effects then panic), the outer bundle frame is still on the stack and resumes. Its later imports still reach the app's graph. Imports could refuse a poisoned bundle, at the cost of every import returning a `Result`.
- **Host handles.** Large or native results (a photo, a capture session) should cross as scoped handles, not bytes. The spike's `Photo` is a small value.
- **Host component imports are still hand-listed** in `bundle!`. They could use the same import-section trick as `#[host_fn]`.
- **Bundle signing, and caching compiled modules by content hash.**

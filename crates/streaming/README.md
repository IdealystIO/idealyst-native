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

The file compiles twice. As the app, `Scoreboard`'s body is left out: the macro replaces it with a stub that sends the props and mounts the component from the installed bundle (`stream_host::remote::install`). As the bundle (`example/bundle`, which points at the same file; built by the app's build script and served by `stream-serve`), only the remote component's body compiles, plus a mount export. On web, `remote` is a no-op. With runtime-vocabulary's `remote-inline` feature, a native app compiles the remote components' bodies in and runs them in-process, with no bundle involved. Use it to measure a remote component against itself, or to debug one.

Props are taken as declared. A `ReadSignal<T>` crosses as a handle: the bundle reads the app's signal live. A `Signal<T>` lets the bundle write it too. A plain value is copied at mount. Your own value types cross with `runtime_vocabulary::remote_value!(MyType)`.

```sh
cargo run --release -p stream-spike --bin stream-serve   # terminal 1: serves /example.wasm, rebuilds on save
cargo run --release -p remote-example                     # terminal 2
```

Edit `Scoreboard`, press **Reload remote**: the remote section remounts from the new build, with the app's state intact. `tests/remote_attr.rs` covers the same path end to end.

### The showcase: a fuller app

`showcase/app` is a shopping-style app whose screens come from a bundle (`src/lib.rs` is both the app and, via `showcase/bundle`, the bundle):

| Piece | Where | Shows |
|---|---|---|
| Tab shell (`App`, a swap navigator) | app | native code hosting remote screens |
| `FeedScreen` | bundle | its own stylesheets over idea's theme tokens (the light/dark switch restyles it), `FeedPrefs` context read live, bundle state, a sync `#[host_fn]`, idea-ui components |
| `ShopNavigator` | bundle | a stack navigator defined in the bundle: route links with typed params, a header reading `StackNav` and the screen's title option, an async `#[host_fn]` (reviews), idea-ui's `Slider`/`Switch`/`Button` writing the app's cart signal |
| `Settings` | app | native controls: the idea theme (light/dark) every screen follows, and the `FeedPrefs` the feed reads |
| idea-ui (`Card`, `Badge`, `Button`, `Typography`, `Switch`, `Slider`) | app | a component library the bundle uses: imported from the app, rendered natively with the app's idea theme, not bundled |

```sh
cargo run --release -p stream-spike --bin stream-serve   # optional: serves /showcase.wasm, rebuilds on save
cargo run --release -p remote-showcase
```

Every screen root — and each navigator layout root, which holds the outlet — sizes itself to fill its parent (`screen_fill()`): a screen is responsible for its own size; its navigator doesn't size it.

`showcase/app/tests/flow.rs` drives the real bundle through the whole app against the mock backend: likes, the theme toggle reaching the feed, the shop list, a product (title in the header, reviews arriving from the async host function, quantity and gift wrap), adding to the cart (seen by the bundle's header and the app shell), back, settings — and nothing left alive after unmount. `--features inline` runs the same flow with the remote screens compiled into the app, which is also the native baseline for the measurements below.

### Remote code using app components

**Every component not marked `remote` lives in the app binary.** When remote code renders one (`Badge` in the example, idea-ui's components, anything), the bundle does not contain it: in a bundle build `#[component]` compiles its body out and replaces it with a stub that sends the props and asks the app for its own copy by name (`module_path::Name`). In a native app build with `remote` on, every component registers itself for that at link time (`runtime_vocabulary::remote::host::APP_COMPONENTS`). An app that doesn't have the component (an older binary) shows `MissingImport` in its place; props that don't decode show `BadProps`. **The bundle must compile the app's source under the app's crate name**, because the import name starts with it: `example/bundle` sets `[lib] name = "remote_example"`, and the app's build script fails the build if the two differ.

**Only the props the call site set cross.** `ui!` (and `jsx!`) pass the names of the fields they set (`BuildElement::build_set`); a bundle build's import sends just those, by name, and the app starts from its OWN `defaults()` and overwrites them. So `Card()` sends nothing, a default that can't cross (idea-ui's default `VariantRef`) never has to, and a prop the app's component doesn't have — a bundle built against a newer version of it — fails naming the prop (`the app's … has no prop `volume``) instead of misreading the rest. Props built without `ui!` (a direct `Foo(props)` call) send every field.

Each prop that does cross goes through its type's `ImportArg`:

| Prop | How it crosses |
|---|---|
| Values (`String`, numbers, `StyleRules`, the framework's style and event types — `TextAlign`, `Color`, `ElementSide`, `KeyEvent`, …) | Copied |
| Your types, with `#[derive(Remote)]` | Field by field, each as its own type crosses (see below) |
| A `stylesheet!` variant axis (`StackGap`, `CardPadding`, …) | Copied: `stylesheet!` derives it |
| `IconData` | Copied; the receiving side interns its `'static` paths |
| `Reactive<T>` (any `T` here) | As `T` if static; a getter into the bundle if live, each reply decoded as `T` |
| Things the app defines with behavior, from an open set (idea-theme's `ToneRef`, `VariantRef`, `ButtonSizeRef`, `ShapeRef`, `TypographyKindRef`) | **By key**: the bundle sends `key()`, the app rebuilds its own registered value. A key the app has no value for fails naming it |
| Callbacks: `Fn()`, `Fn(A)`, `Fn(A, B)`, answering `Fn(A) -> R`, `Fn(&KeyEvent) -> KeyOutcome`, `Option<…>` | Run in the bundle; arguments and replies cross as values. A stopped bundle answers `R::default()` (a key handler: the platform default) |
| Render slots (`Fn() -> Element`, idea-ui's `ModalContent`) | The app calls it; the bundle builds the tree and it crosses back |
| Children, `Element` props | Encoded subtrees, built by the bundle |
| `Rc<StyleSheet>` | A proxied sheet, resolved by the app's theme |
| A `Ref` the bundle hands an app component to fill (`Button(bind_to = Some(trigger))`, `Field(field_ref = …)`) | The app passes the component a ref of its own and holds it; the bundle's ref forwards each call to it, read at call time (a call before it's filled answers the method's default) |
| `AnchorTarget` (`Popover(target = AnchorTarget::from(trigger))`) | A getter into the bundle, which measures its target — through the app, for a ref like the one above |
| `Signal<T>` / `ReadSignal<T>` the app gave the bundle | The app's own handle |
| `Signal<T>` / `ReadSignal<T>` the bundle created | **Promoted**: the value moves into the app's arena |

A prop type with no `ImportArg` doesn't stop anything compiling: it fails by name at runtime, in the component's place.

**`#[derive(Remote)]`** makes a type cross in every direction it can: as an app component's prop set by remote code (`ImportArg`), as a remote component's prop (`RemoteProp`), and as plain data — a signal's value, a callback's argument (`RemoteValue`). Each field crosses as its own type does, through probes, so a field that can't (a callback, inside a signal's value) fails naming its type when such a value actually crosses — never at compile time. The expansion is empty unless the app hosts remote components, so a library derives it on its value types unconditionally. `stylesheet!` derives it on the variant enums it generates. A signal's value needs `RemoteValue` (once the app holds a bundle's signal, every bundle read and write re-encodes it); a `Reactive` inside such a value crosses as its current value.

**Crossing by key** is the rule for components ("not remote = in the app") applied to the other things a library defines. A type marks itself with `runtime_vocabulary::__remote_keyed!(MoodRef, |v| v.0.key())`, and each value registers with `__remote_key!(MoodRef, |v| v.0.key(), MoodRef(Rc::new(Loud)))` — a link-time table (`remote::host::KEYED`), like `APP_COMPONENTS`. idea-theme does both: its five ref types are keyed, and every built-in marker, every `tone!` / `variant!` an app declares, and idea-ui's card variants register themselves (`idea_theme::__remote_marker!`). A hand-written `impl Tone for X` in an app needs that one line too. All of it expands to nothing unless the app hosts remote components.

**idea-ui works as a remote component library** with no change to its components. `spike/ideaui` uses every one of them from remote code — layout, text and status, actions, forms, dates, navigation, overlays (tooltip, a popover and a menu anchored to a `Button`'s ref, a modal, the toast host), data — and `tests/idea_ui.rs` checks each area against the same tree rendered in-process: the tree, the handle methods the components call, and what a press does to the backend. idea-ui's part: `#[derive(Remote)]` on ~30 value types, keyed token refs, and `#[props]` on its two field-less props structs. Still open: idea-ui's global functions (`push_toast`, `set_theme`, `active_theme`) run inside the bundle against the bundle's own state — a toast remote code pushes never reaches the app's `ToastHost` (checked) — so they either forward to the app or are app-only.

**Promotion.** A bundle-created signal keeps its value in the bundle until the bundle hands it to native code. Then the app's prop decoder (which knows `T`) takes the value over: the slot keeps its subscribers and the bundle keeps its handle, but the value is now a real `SignalData<T>` in the app's arena, and the bundle reads and writes it the way it does any app signal. Native code reads it at native speed; a memo's output promotes too, and its derivation keeps writing the native value. It is two-phase (`GuestHooks::promote` / `promote_finish`), so a value the app can't decode leaves the bundle untouched. The native slot holds a `Promoted<T>` whose `as_any_mut` returns the inner `SignalData<T>`, so every native read, write and commit runs unchanged code — measured on the shipped profiles: no regression with `bridge` on or off.

**Web never compiles any of this.** `remote` is a no-op on web, and the bridge, the codec and the registrations are compiled only for native targets or a bundle build, whatever features an app enables.

## Refs: handles to nodes the app mounted

A remote component takes refs like any component (`text_input(..., bind = input)`, `on_handle`), and gets the same handle types (`TextInputHandle`, `ViewHandle`, …). A handle is a node plus a static ops table, so the bundle's handle is built with a forwarding ops table (`runtime_vocabulary::remote::handles::RemoteOps`, one type implementing every primitive's ops trait): each method crosses to the app as a `HandleCall` (over wasm, the `idealyst_ui.handle_call` import) and runs on the real handle, which the app holds from the moment the backend fills the ref. Layout subscriptions cross too (the app holds the bundle's callback), and so does a portal anchored to a node the bundle holds a ref to: the app's portal asks the bundle for the rect, and the bundle asks the app's handle.

The app's entry goes when the bundle drops its handle, and in any case when the tree that made it unmounts. The decoded root owns its connection, so the handle lives exactly as long as the tree, whether or not the bundle releases it (it may keep it in a `Ref` slot, or have been stopped). `tests/remote_elements.rs` (`refs`) checks every method reaches the backend's handle in the same order as natively; `tests/remote_attr.rs` checks it over wasm.

## Context

A remote component reads the app's context with a plain `inject`, for any type that derives `Remote` (and is `Clone` — `inject` hands out a copy):

```rust
#[derive(Clone, Remote)]
pub struct FeedPrefs { pub compact: ReadSignal<bool> }

// App:    runtime_world::provide(FeedPrefs { compact: compact.read_only() });
// Remote: let prefs = runtime_world::inject::<FeedPrefs>();
```

Fields cross like a remote component's props: a `ReadSignal` / `Signal` as a handle into the app's graph (read live, writable for `Signal`), a value as a copy. Each derived type registers itself under `module_path::Name` — natively in a link-time slice, which the app's remote loader offers to bundles; in a bundle (`linkme` has no wasm32 support) as an `__idealyst_ctx_<path>` export the loader calls at load to register its decoder. A bundle's `inject::<FeedPrefs>()` then falls back to the app's value when the remote tree provides none itself; the exports live as long as the component that asked. Context that doesn't derive `Remote` stays invisible to bundles. `tests/remote_attr.rs` covers it over wasm (live signal, a non-`Remote` context hidden, unprovided context absent).

### Themes are tokens, not context

A stylesheet doesn't read a theme value. In `stylesheet! { pub Banner<IdeaThemeRef> { base(t) { background: t.intent.primary.solid_bg() } } }`, `t` is a namespace of token NAMES (`intent-primary-solid-bg`); values come from the token registry at resolve time, which `install_idea_theme` / `set_idea_theme` write. So a stylesheet in remote code needs nothing from the app: it runs in the bundle, the token names cross with its rules, and the app resolves them against its installed theme — following a theme swap exactly as native code does. `tests/idea_ui.rs` (`a_bundles_own_stylesheet_uses_the_apps_theme_tokens`) mounts remote code's own token sheets on plain primitives, swaps light → dark in the app, and checks the resolved colors against the same tree in-process. The showcase's feed does the same on screen (Settings → Dark theme).

Code that reads the active theme object directly (`active_theme()`, an idea-theme `Variant::render`) runs wherever it's called: inside idea-ui's components that is the app; called from remote code, it would see the bundle's own (empty) copy — the same open question as `push_toast`.

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
- **Props and context the app owns** cross as handles: the app exports a signal (`stream_host::kernel::export_signal` / `export_read_signal`) and the bundle imports it (`runtime_world::remote_guest::import_signal`); reads subscribe in the app's graph. Context is declared by name on both sides (`export_context` / `register_remote_context`, emitted by `#[derive(Remote)]`); context the app did not declare is invisible to bundles.
- **Parity is enforced, not hoped for.** Adding an `Engine` method doesn't compile until every engine has it, and `--features loopback-engine` runs the entire kernel suite through the bridge in one process.

**The tree crosses as data** (`runtime-vocabulary/src/remote`, feature `remote`). The bundle encodes the `Element` it built into a `Node`. Every closure in it (a dynamic text getter, an `on_press`, a `Dyn` hole's builder, a keyed list's items and render, a stylesheet) stays in a bundle-side table under a callback id, and the id crosses instead. The app decodes the `Node` into real primitives whose closures call the bundle by id, then realizes it like any other tree. When the app drops such a closure it releases the id, and the bundle frees the closure.
- A component's `Owned` crosses as the app-side scope id it already is, and the app claims it (`runtime_world::remote::claim_scope`), so unmounting tears the bundle's state down like a native component's.
- **Styles keep the app's theme.** Style rules cross with token names intact, so the app's theme resolves them. A stylesheet crosses as its shape, and the app rebuilds the same sheet with each closure calling back into the bundle. State, breakpoint and container overlays then work exactly as for a native sheet. Ids are refcounted, so a sheet shared by many nodes crosses once.
- **App components are imported by name.** A bundle calls `remote::bundle::import("Card", &props, children)`; the app exports `Card` with `remote::host::register_import`. A bundle that needs a component the app doesn't export fails to decode with `MissingImport`, rather than rendering half a tree.
- **Every builtin primitive has a decision.** `remote::crossing` says whether each payload crosses; a test fails when `register_builtins` gains one with no entry. Crossing today: `view`, `pressable`, `text` (including styled runs), `button` (including icons), `image`, `icon`, `link`, `toggle`, `slider`, `activity_indicator`, `text_input`, `text_area` and `scroll_view`, with every event handler they take (touch, wheel, hover, file drop, key, focus, blur, scroll, image load and error). Event handlers cross as callbacks whose event and reply are encoded; a stopped bundle's handlers answer the platform default. Everything else panics at encode, naming itself. `tests/remote_elements.rs` (`controls`) checks every one against the native build, handler by handler. The structural primitives cross too: `repeat`, `presence`, `portal` (viewport and named targets), `virtualizer` (including measured sizes and `item_diff`, whose snapshots stay in the bundle) and `virtual_grid`; their builders, sizes and keys are callbacks the app's backend calls, and `tests/remote_elements.rs` (`structural`) drives them the way a backend does.

What the tests prove (`remote_counter.rs`): the real `RemoteCounter`, written with `#[component]` and `ui!`, mounted from the bundle drives the app's backend through exactly the same calls as the native build, through mount, button presses (bundle state), a prop change and a context change (app state), and unmount. After unmount, no bundle callback or scope is left behind.

## Measurements: the bridged design

`showcase/measure.sh` builds `showcase/app/examples/measure.rs` twice: once with the screens mounted from the bundle, and once with `--features inline`, which compiles the same `#[component(remote)]` source into the app (the vocabulary's `remote-inline` feature). Each is a separate binary, built under the profile a native app ships with (opt 3, no LTO). The two runs alternate, and each figure is the median of 7. Both run on host-mock, so the numbers cover the framework and the interpreter; a platform toolkit adds the same cost to both. Measured on an Apple M3 Max with the bundle at the workspace release profile (opt z, fat LTO).

The screens use idea-ui, which renders natively either way: only the screen's own code is interpreted.

| | Remote | Same code in-process |
|---|---|---|
| Bundle | 312 KB raw, 95 KB brotli | — |
| Load (validate, instantiate) | 1.5 ms (first: 1.9 ms) | — |
| `FeedScreen` mount (3 idea-ui cards with buttons, context, a host fn) | 665 µs (first: 3.0 ms) | 51 µs (first: 0.8 ms) |
| `ShopNavigator` mount (a stack navigator defined in the bundle) | 691 µs | 41 µs |
| Push / pop a product screen | 463 / 68 µs | 40 / 6.3 µs |
| A press handled by the bundle (♥: an idea-ui `Button`, bundle state, its label) | 9.2 µs | 0.4 µs |
| The app sets a signal the bundle reads (cart in the header) | 11 µs | 0.5 µs |
| The app toggles context that adds and removes 3 bodies | 126 µs | 7.8 µs |
| An idea-ui `Slider` drag (its handler is bundle code) | 21 µs | 2.5 µs |
| Memory per mounted `FeedScreen` | 85 KB | 82 KB |
| Bundle linear memory, idle / with 200 feeds mounted | 256 / 704 KB | — |

Mounts cost 11–17× in-process, against about 20× when the screens drew everything themselves (the earlier hand-rolled showcase: `FeedScreen` 405 µs vs 19 µs): a library the app ships does its rendering natively.

**Where the time goes** (measured on the earlier, hand-rolled showcase). In a `sample` profile of repeated `FeedScreen` mounts, 87% of the time is wasmi executing bundle code. Decoding the tree, the bridge's imports and the app's graph each take under 1%. So the cost is the framework code that runs in the bundle (building the tree, the bridged kernel's tables, encoding), interpreted at roughly 20× native. It is not the crossing. Every mount and update above still takes well under a 16 ms frame.

**The bundle's opt level trades size for speed.** The earlier showcase bundle, all builds with fat LTO:

| Bundle opt level | Raw / brotli | Load | `FeedScreen` mount | Press | Push |
|---|---|---|---|---|---|
| `z` (default) | 289 / 90 KB | 1.36 ms | 405 µs | 8.6 µs | 282 µs |
| `s` | 392 / 98 KB | 1.73 ms | 308 µs | 6.5 µs | 218 µs |
| `3` | 563 / 111 KB | 2.33 ms | 274 µs | 5.9 µs | 199 µs |

**Native cost of turning remote on: none measured.** The only runtime change in an app with remote components is runtime-world's `bridge` feature. The kernel benchmark at the native shipped profile shows bridge on within ±3% of bridge off on every case (peek, set+flush, fan-out, scope create/drop, memo chains), which is noise (separate builds of identical code agree within 1%). Registering app components and contexts happens at link time.

**Fixed by these measurements:**
- **Reloads leaked the old bundle's code.** A wasmi `Engine` never frees compiled functions until it drops. The loader shared one engine across reloads, so every reload kept about 750 KB. Each bundle now gets its own engine, which its store holds (`remote::engine`); `regression_a_reload_frees_the_replaced_bundles_code` pins this.
- **The app's profile leaked into the bundle build.** `CARGO_PROFILE_*` overrides given to the app's build reached the build script's nested cargo, which rebuilt the bundle at opt 3 without LTO (575 KB instead of 287 KB). The bundle build drops them (`regression_bundle_build_ignores_the_apps_profile_overrides`).
- **Edits to a library a bundle uses didn't rebuild it.** The build scripts and `stream-serve` watched only the app's own source, so a change to idea-ui or the framework left the embedded and served bundles stale. Both now rerun on every file the bundle compiled, from cargo's dep-info (`guest_build::bundle_sources`; `regression_bundle_sources_include_the_libraries_the_bundle_compiles`).
- **A pending animation aborted the process at exit.** A timer holding an idea-ui `AnimatedValue`, destroyed during thread-local teardown, unregistered its tick from the animation clock after the clock's own thread-local was gone; `unregister` now does nothing once the clock is gone (`regression_a_registration_dropped_after_the_clock_does_not_panic`). A desktop app quitting with an animation in flight hit the same path.

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

- **On-device numbers.** The bridged design's numbers are from a Mac. iPhone and Android runs are next.
- **Plain value props are fixed at mount.** A live prop is declared `ReadSignal<T>`; `#[component(remote)]` could make plain props reactive by default, as `#[props]` does natively.
- **No manifest.** A manifest listing the props, app components and context names a bundle needs, checked before mount, is still to do (by choice: see where things break first). `remote` can't be combined with `lazy` yet.
- **Every builtin primitive crosses except `graphics`** (and `lazy`, which has no meaning in a bundle). `crossing` records each decision. `graphics` never will: it hands the author's code a native GPU surface, which interpreted wasm can't drive — draw in an app component and use that from the remote component.
- **Nested bundles**, where one bundle mounts another bundle's component by name.
- **A poisoned bundle can keep running in one case.** If a host→bundle call made *from inside* a bundle call traps (a bundle calling `flush`, whose effects then panic), the outer bundle frame is still on the stack and resumes. Its later imports still reach the app's graph. Imports could refuse a poisoned bundle, at the cost of every import returning a `Result`.
- **Host handles.** Large or native results (a photo, a capture session) should cross as scoped handles, not bytes. The spike's `Photo` is a small value.
- **Host component imports are still hand-listed** in `bundle!`. They could use the same import-section trick as `#[host_fn]`.
- **Bundle signing, and caching compiled modules by content hash.**

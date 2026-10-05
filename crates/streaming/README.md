# Remote components (spike)

A remote component (`#[component(remote)]`) is ordinary Rust compiled to `wasm32`. An app downloads it as a **bundle**, runs it on-device in the [wasmi](https://github.com/wasmi-labs/wasmi) interpreter, and mounts it like any other component. The same mechanism covers server-driven UI (small bundles per screen) and OTA updates (one bundle per host build).

The crates in this directory (the loader `stream-host`, the showcase, the example, the spike's fixtures and tests) are unpublished. The mechanism itself ships in the framework's own crates, all behind features an app opts into: runtime-world's `bridge`, runtime-vocabulary's `remote` (and `remote-inline`), runtime-shared's `remote-serde`, and the macros (`#[component(remote)]`, `#[derive(Remote)]`, `#[host_fn]`). An app that doesn't enable them gets none of it, and web builds never compile it.

What runs inside a bundle is **the bridged design**: the bundle runs the same real framework code, but its reactive kernel is *bridged*: its signals, effects and scopes live in the app's own graph, and the tree it builds crosses to the app as data, to be realized by the app's own registry and backend. One graph, one backend; the bundle carries only its own code (147 KB for `RemoteCounter`). See [The bridged design](#the-bridged-design).

Two earlier designs were built and deleted; what they established is under [History: model A](#history-model-a) (a host-owned graph with hand-written node descriptions) and [History: model B](#history-model-b) (a private copy of the framework in each bundle).

| Crate | Role |
|---|---|
| `abi` (`stream-abi`) | `Wire`, the byte codec the hand-written kernel-bridge code shares with the loader (`stream_host::kernel::export_signal` & co., `spike/kernelguest`, `spike/remoteguest`). Zero dependencies, because it ships inside every bundle that uses it. `#[component(remote)]` code doesn't use it: its values cross as the framework's `RemoteValue`. |
| `host` (`stream-host`) | Loads a bundle into wasmi and runs its bridged kernel on the app's graph (`kernel`), the loader `#[component(remote)]` mounts from (`remote::install` / `install_with`), and a minimal bundle fetch (`fetch`). |
| `spike` (`stream-spike`) | Builds the spike's bundles to wasm32 in its build script. Holds the end-to-end tests and `stream-serve`. |
| `spike/components` | `RemoteCounter`: a plain crate of `#[component]`s the app also links natively (the parity baseline). |
| `spike/kernelguest` | Test bundle for the kernel bridge: plain `runtime-world` code whose graph is the app's. |
| `spike/remoteguest` | `spike/components`' `RemoteCounter` as a bridged remote component, with a hand-written mount export. |
| `spike/remoteattr` | `#[component(remote)]` end to end: a remote component using an app component. |
| `spike/camera` | A fake camera SDK: two `#[host_fn]`s (`battery_level`, async `take_photo`) the remote `Snapshot` calls. |
| `spike/ideaui` | Remote components using every idea-ui component. |
| `spike/demo` (`stream-demo`) | An AppKit window: `RemoteCounter` from a served bundle next to the same component compiled in. |
| `example/app`, `example/bundle` | An app and its remote components in one file. |
| `showcase/app`, `showcase/bundle` | A fuller app whose screens come from a bundle. |

## Running it

```sh
cargo test -p stream-abi -p stream-spike
cargo test -p stream-spike --test kernel_bridge               # the bridged kernel over wasm
cargo test -p stream-spike --test remote_counter              # a ui! component, bridged, vs native
cargo test -p runtime-world --features loopback-engine       # every kernel test, through the bridge
cargo test -p runtime-vocabulary --features remote-loopback --test remote_elements  # the element codec
```

The build scripts of `stream-spike`, the showcase and the example compile their bundles for `wasm32-unknown-unknown` (`rustup target add wasm32-unknown-unknown`). Without that target they don't fail: they embed empty placeholders and print a warning, so a workspace build still goes through, and anything that loads one of those bundles fails with a load error. Installing the target reruns them.

### See it live

```sh
cargo run --release -p stream-spike --bin stream-serve   # serves the bundles, rebuilds on save
cargo run --release -p stream-demo                        # AppKit window
```

The window shows RemoteCounter from the bundle (green, from `/remote.wasm`) next to the same component compiled into the app (grey).

1. Edit `spike/components/src/lib.rs`.
2. Press **Refresh bundle**. The green section remounts from the new build; the app is not rebuilt or restarted. The grey native copy keeps the old code, which is the point of comparison.
3. The host buttons drive host state the bridged component reads: `external ± 1` is a prop, **switch user** is context.

Every builtin primitive crosses — leaves, structural primitives and navigators — except `graphics` (and `lazy`, which has no meaning in a bundle); see [The bridged design](#the-bridged-design). Using `graphics` panics in the bundle while it mounts, and the window shows the bundle's panic message in place of the component. A panic at any other time (a press handler, an effect) does the same; see [Panics](#panics-in-a-bundle).

How it behaves:

- **Host signals keep their values** across a swap; guest-local state starts over.
- **A failed build** shows the compiler error, and the app keeps running the previous bundle.
- **With no server running**, the app uses the bundle compiled into it.

`tests/remote_attr.rs` pins down the swap: `regression_a_reload_frees_the_replaced_bundles_code` checks a reload frees the old bundle, and the reload tests check app state carries over.

## `#[component(remote)]`: an app and its remote components in one file

`example/app/src/main.rs` is a complete app: a native `App`, and a
`#[component(remote)] Scoreboard` it renders with the app's state as props.

```rust
#[component(remote)]
pub fn Scoreboard(player: String, score: ReadSignal<i64>, cheers: Signal<i64>) -> Element { … }
```

The file compiles twice. As the app, `Scoreboard`'s body is left out: the macro replaces it with a stub that sends the props and mounts the component from the installed bundle (`stream_host::remote::install`). As the bundle (`example/bundle`, which points at the same file; built by the app's build script and served by `stream-serve`), only the remote component's body compiles, plus a mount export. On web, `remote` is a no-op. With runtime-vocabulary's `remote-inline` feature, a native app compiles the remote components' bodies in and runs them in-process, with no bundle involved. Use it to measure a remote component against itself, or to debug one.

Props are taken as declared. A `ReadSignal<T>` crosses as a handle: the bundle reads the app's signal live. A `Signal<T>` lets the bundle write it too. A plain value is copied at mount. Your own value types cross with `#[derive(Remote)]` (see [Remote code using app components](#remote-code-using-app-components)).

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

**idea-ui works as a remote component library** with no change to its components. `spike/ideaui` uses every one of them from remote code — layout, text and status, actions, forms, dates, navigation, overlays (tooltip, a popover and a menu anchored to a `Button`'s ref, a modal, the toast host), data — and `tests/idea_ui.rs` checks each area against the same tree rendered in-process: the tree, the handle methods the components call, and what a press does to the backend. idea-ui's part: `#[derive(Remote)]` on ~30 value types, keyed token refs, and `#[props]` on its two field-less props structs. idea-ui's global functions that change app state are host functions (below): remote code's `push_toast` lands on the app's toast queue, and `set_idea_color_scheme` switches the app's theme.

**Promotion.** A bundle-created signal keeps its value in the bundle until the bundle hands it to native code. Then the app's prop decoder (which knows `T`) takes the value over: the slot keeps its subscribers and the bundle keeps its handle, but the value is now a real `SignalData<T>` in the app's arena, and the bundle reads and writes it the way it does any app signal. Native code reads it at native speed; a memo's output promotes too, and its derivation keeps writing the native value. It is two-phase (`GuestHooks::promote` / `promote_finish`), so a value the app can't decode leaves the bundle untouched. The native slot holds a `Promoted<T>` whose `as_any_mut` returns the inner `SignalData<T>`, so every native read, write and commit runs unchanged code — measured on the shipped profiles: no regression with `bridge` on or off.

**Web never compiles any of this.** `remote` is a no-op on web, and the bridge, the codec and the registrations are compiled only for native targets or a bundle build, whatever features an app enables.

## Refs: handles to nodes the app mounted

A remote component takes refs like any component (`text_input(..., bind = input)`, `on_handle`), and gets the same handle types (`TextInputHandle`, `ViewHandle`, …). A handle is a node plus a static ops table, so the bundle's handle is built with a forwarding ops table (`runtime_vocabulary::remote::handles::RemoteOps`, one type implementing every primitive's ops trait): each method crosses to the app as a `HandleCall` (over wasm, the `idealyst_ui.handle_call` import) and runs on the real handle, which the app holds from the moment the backend fills the ref. Layout subscriptions cross too (the app holds the bundle's callback), and so does a portal anchored to a node the bundle holds a ref to: the app's portal asks the bundle for the rect, and the bundle asks the app's handle.

The app's entry goes when the bundle drops its handle, and in any case when the tree that made it unmounts. The decoded root owns its connection, so the handle lives exactly as long as the tree, whether or not the bundle releases it (it may keep it in a `Ref` slot, or have been stopped). Each entry belongs to the bundle whose tree holds it: another bundle's call or release on it is refused (the caller is stopped). Handles the app passes to its remote components as props (a navigator, `pop`) are any bundle's to call, since a reload remounts with the same props, but end only when the mount that passed them does. `tests/remote_elements.rs` (`refs`) checks every method reaches the backend's handle in the same order as natively; `tests/remote_attr.rs` checks it over wasm.

## Context

A remote component reads the app's context with a plain `inject`, for any type that derives `Remote` (and is `Clone` — `inject` hands out a copy):

```rust
#[derive(Clone, Remote)]
pub struct FeedPrefs { pub compact: ReadSignal<bool> }

// App:    runtime_world::provide(FeedPrefs { compact: compact.read_only() });
// Remote: let prefs = runtime_world::inject::<FeedPrefs>();
```

Fields cross like a remote component's props: a `ReadSignal` / `Signal` as a handle into the app's graph (read live, writable for `Signal`), a value as a copy. Each derived type registers itself under `module_path::Name` — natively in a link-time slice, which the app's remote loader offers to bundles; in a bundle (`linkme` has no wasm32 support) as an `__idealyst_ctx_<path>` export the loader calls at load to register its decoder. A bundle's `inject::<FeedPrefs>()` then falls back to the app's value when the remote tree provides none itself; the exports live as long as the component that asked. Context that doesn't derive `Remote` stays invisible to bundles. The cost of "every `Remote` type is context": each registration is a bundle export, which keeps that type's decoder in every bundle that links it, injected or not — idea-ui's ~31 value types add about 23 KB raw / 5 KB brotli. A way to opt a library's value types out (a `#[remote(no_context)]`-style marker on the derive) is the optimization if bundle size ever matters; nothing needs it to work. `tests/remote_attr.rs` covers it over wasm (live signal, a non-`Remote` context hidden, unprovided context absent).

### Themes are tokens, not context

A stylesheet doesn't read a theme value. In `stylesheet! { pub Banner<IdeaThemeRef> { base(t) { background: t.intent.primary.solid_bg() } } }`, `t` is a namespace of token NAMES (`intent-primary-solid-bg`); values come from the token registry at resolve time, which `install_idea_theme` / `set_idea_theme` write. So a stylesheet in remote code needs nothing from the app: it runs in the bundle, the token names cross with its rules, and the app resolves them against its installed theme — following a theme swap exactly as native code does. `tests/idea_ui.rs` (`a_bundles_own_stylesheet_uses_the_apps_theme_tokens`) mounts remote code's own token sheets on plain primitives, swaps light → dark in the app, and checks the resolved colors against the same tree in-process. The showcase's feed does the same on screen (Settings → Dark theme).

Code that reads the active theme object directly (`active_theme()`, an idea-theme `Variant::render`) runs wherever it's called: inside idea-ui's components that is the app; called from remote code, it sees the bundle's own (empty) copy. Remote code CHANGES the theme through a host function: `idea_ui::set_idea_color_scheme(ColorScheme::Dark)` switches the app between the light/dark pair it installed (`install_idea_theme_schemes`). A whole theme can't be an argument — it's code, not data — so the app owns the themes and remote code picks one.

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

Remote code calls native code through `#[host_fn]` (`runtime_core::host_fn`). One definition, in a crate both builds see — an app, or a library:

```rust
#[host_fn]
pub fn device_name() -> String { … }
#[host_fn]
pub async fn fetch_reviews(product: u32) -> Vec<String> { … }
```

In the app it is the function as written. In a bundle it becomes a stub that asks the app to run it: a sync call answers inline, and an async one returns a future (`runtime_vocabulary::remote::bundle::HostFuture`) that the framework's own `spawn_then` drives, the same call shape as natively. The app runs the real future on its own executor and delivers the result to the bundle as a one-shot callback. Arguments and results cross as `RemoteValue`s — plain data, including values that cross by key (an idea-theme `ToneRef`). The choice of build goes through the vocabulary, so a library using it declares no cfg, and an app without remote components gets only the function.

The app lists what bundles may call: `stream_host::remote::install_with(wasm, vec![device_name::export(), …])`. A bundle that calls anything else is refused at load with `MissingHostFunctions`, naming it; one whose signature changed since the app was built (the import name carries an FNV-1a fingerprint of the signature's type spellings, stable across toolchains) with `IncompatibleHostFunctions`. Arguments that still don't decode stop the bundle, never the app. A result that arrives after its bundle was stopped is dropped.

**Libraries' global functions.** A function that changes the APP's state has to be a host function, or remote code changes the bundle's own copy of that state. idea-ui's are: `set_idea_color_scheme` (the theme), `push_standard_toast` (what `push_toast` / `push_toast_with` call, with a concrete signature) and `dismiss_toast`. An app adds them all with `idea_ui::host_fns()`. The showcase's feed switches the app's theme and pushes a toast from the bundle; `showcase/app/tests/flow.rs` checks both land in the app (and was checked to fail with the functions run bundle-side). Toasts built from closures (`Toast` with an `action`, `push_toast_node`) aren't data, so they stay on whichever side builds them.

Plain functions are always compiled into the bundle; only `#[host_fn]` crosses to the app. `tests/remote_attr.rs` covers sync, async, the load-time refusals and a result for a stopped bundle.

`spike/camera` is a host-function SDK in miniature: its value types (`PhotoOptions`, `Photo`, `CameraError`) derive `Remote`, and `take_photo` returns a `Result`, which crosses as a value like `Option`.

## Panics in a bundle

A panic in a bundle never takes the app down. A Rust panic in wasm is `panic = "abort"`: it traps the interpreter, and no destructor in the bundle runs, so a `RefCell` it held stays borrowed and its tables stay half-updated. So the first trap **poisons** the bundle (`stream-host`, `kernel.rs`):

1. The kernel frames the trapped call had open in the app are closed. The bundle never makes their end calls, so without this the app would be left with an entered world, tracking off, or a collecting scope sweeping up its own new signals. The app journals every frame a bundle opens (`runtime_world::remote::bundle_frames_mark`), and each entry into a bundle unwinds back to its mark on a trap (`unwind_bundle_frames`); what an abandoned scope collected is freed.
2. The call that trapped, and every later call into that bundle, answers nothing. Each kind of call has a safe answer: an effect doesn't run, a commit reports no change, a handler does nothing, a getter returns its last value, a `Dyn` hole or keyed row renders nothing, a style getter resolves to defaults (`runtime_vocabulary::remote::host`).
3. The loader is told (`KernelBundle::on_poison`) and remounts every remote component. The poisoned bundle refuses the mounts, so each shows the bundle's own panic message (kept by the bundle's panic hook) in its place, and its old tree is torn down.
4. The app's own UI and state carry on. Reloading a new bundle brings the components back.

The same end comes for a bundle that sends the app something invalid, since release apps build with `panic = "abort"` and a panic in the app would end it. Invalid requests include a kernel request out of order (a signal created outside any world, a frame ended that was never begun, a write to a slot it was never given or got read-only), a reply or handle call that doesn't decode, a pointer out of bounds, and host-function arguments that don't decode. The app's kernel reports the request as a fault instead of panicking (`runtime_world::remote::trap_faults` / `take_fault`), every kernel import turns a fault into a trap in the bundle that made it, and reply decoders stop the bundle through `Link::fail`. In one process (the loopback tests) there is no bundle to stop, so these still panic.

`tests/remote_attr.rs` covers a panicking press handler (including one inside `collect_owned(|| untrack(..))`), a bundle making each kind of invalid request (`Rogue`), a panicking effect and recovery by reload, against a real wasm bundle. Each regression test was checked to fail with the old "trap panics the app" code.

## The bridged design

A bundle is built with `--cfg idealyst_stream_guest` (a build flag, not a cargo feature, so it can never leak into an app through feature unification). Under that flag two things change, and nothing else:

**The kernel is bridged** (`runtime-world/src/bridge`). runtime-world's typed layer runs on an `Engine`; natively that is the arena, in a bundle it is `Bridged`. `Bridged` keeps what cannot leave the bundle (signal values, effect bodies, cleanups, context values) in local tables, and forwards every graph operation (slots, subscriptions, staging, flush, scopes, context stacks) to the app over wasm imports. The app holds a proxy in each slot the bundle owns. So a bundle's signal is a slot in the app's arena, the app's flush runs the bundle's effects, and the app's scopes own them.
- **Props and context the app owns** cross as handles: the app exports a signal (`stream_host::kernel::export_signal` / `export_read_signal`) and the bundle imports it (`runtime_world::remote_guest::import_signal`); reads subscribe in the app's graph. Each export is its own registration, withdrawn when its guard drops, so two mounts sharing one signal (or a remount overlapping the old tree) don't withdraw it from each other, and a read-only export never takes away a live two-way one. On the bundle side every import of one slot shares a single entry, freed when the last import's scope ends (never one the bundle created and promoted). Context is declared by name on both sides (`export_context` / `register_remote_context`, emitted by `#[derive(Remote)]`); context the app did not declare is invisible to bundles.
- **Parity is enforced, not hoped for.** Adding an `Engine` method doesn't compile until every engine has it, and `--features loopback-engine` runs the entire kernel suite through the bridge in one process.

**The tree crosses as data** (`runtime-vocabulary/src/remote`, feature `remote`). The bundle encodes the `Element` it built into a `Node`. Every closure in it (a dynamic text getter, an `on_press`, a `Dyn` hole's builder, a keyed list's items and render, a stylesheet) stays in a bundle-side table under a callback id, and the id crosses instead. The app decodes the `Node` into real primitives whose closures call the bundle by id, then realizes it like any other tree. When the app drops such a closure it releases the id, and the bundle frees the closure.
- A component's `Owned` crosses as the app-side scope id it already is, and the app claims it (`runtime_world::remote::claim_scope`), so unmounting tears the bundle's state down like a native component's.
- **Styles keep the app's theme.** Style rules cross with token names intact, so the app's theme resolves them. A stylesheet crosses as its shape, and the app rebuilds the same sheet with each closure calling back into the bundle. State, breakpoint and container overlays then work exactly as for a native sheet. Ids are refcounted, so a sheet shared by many nodes crosses once.
- **App components are imported by name.** In a bundle build, a `#[component]` that isn't remote is a stub that sends the props its call site set and asks the app for its own copy by `module_path::Name`; every `#[component]` in a native app with `remote` registers itself for that at link time (`remote::host::APP_COMPONENTS`). A bundle that needs a component the app doesn't have fails to decode with `MissingImport`, rather than rendering half a tree. A subtree the bundle builds later (a `Dyn` hole, a keyed row, a screen, a render slot) that fails the same way renders the error in its place. Either way nothing leaks: every reply carrying a tree also lists what crossed with it (`Crossed`: the callback ids registered for it, including those inside app-component props, which are sent when the bundle builds the element; re-crossed sheets; scopes), and a failed decode releases whatever it never reached. (`remote::bundle::import` / `remote::host::register_import` are the same exchange by hand, which the codec's own tests use.)
- **Every builtin primitive has a decision.** `remote::crossing` says whether each payload crosses; a test fails when `register_builtins` gains one with no entry. Crossing today: `view`, `pressable`, `text` (including styled runs), `button` (including icons), `image`, `icon`, `link`, `toggle`, `slider`, `activity_indicator`, `text_input`, `text_area` and `scroll_view`, with every event handler they take (touch, wheel, hover, file drop, key, focus, blur, scroll, image load and error). Event handlers cross as callbacks whose event and reply are encoded; a stopped bundle's handlers answer the platform default. Only `graphics` (and `lazy`) panic at encode, naming themselves. `tests/remote_elements.rs` (`controls`) checks every one against the native build, handler by handler. The structural primitives cross too: `repeat`, `presence`, `portal` (viewport, named and anchored targets), `virtualizer` (including measured sizes and `item_diff`, whose snapshots stay in the bundle), `virtual_grid`, and the stack and swap navigators with their outlet (see [Navigation](#navigation)); their builders, sizes and keys are callbacks the app's backend calls, and `tests/remote_elements.rs` (`structural`) drives them the way a backend does.

What the tests prove (`remote_counter.rs`): the real `RemoteCounter`, written with `#[component]` and `ui!`, mounted from the bundle drives the app's backend through exactly the same calls as the native build, through mount, button presses (bundle state), a prop change and a context change (app state), and unmount. After unmount, no bundle callback or scope is left behind.

## Measurements: the bridged design

`showcase/measure.sh [ROUNDS] [macos|ios-sim]` builds `showcase/app/examples/measure.rs` twice: once with the screens mounted from the bundle, and once with `--features inline`, which compiles the same `#[component(remote)]` source into the app (the vocabulary's `remote-inline` feature). Each is a separate binary, built under the profile a native app ships with (opt 3, no LTO). The two runs alternate, and each figure is the median of 7. `ios-sim` builds for `aarch64-apple-ios-sim` and runs inside the booted simulator (`simctl spawn`): iOS-target code on the Mac's CPU, not a phone's. Both modes run on host-mock, so the numbers cover the framework and the interpreter; a platform toolkit adds the same cost to both. Apple M3 Max, wasmi 2.0, the bundle at the workspace release profile (opt z, fat LTO). The iOS simulator figures match macOS within a few percent; macOS shown.

The screens use idea-ui, which renders natively either way: only the screen's own code is interpreted.

| | Remote | Same code in-process |
|---|---|---|
| Bundle | 375 KB raw, 106 KB brotli | — |
| Load (validate, instantiate) | 1.8 ms (first: 2.1 ms) | — |
| `FeedScreen` mount (3 idea-ui cards with buttons, context, a host fn) | 922 µs (first: 3.4 ms) | 73 µs (first: 0.8 ms) |
| `ShopNavigator` mount (a stack navigator defined in the bundle) | 740 µs | 43 µs |
| Push / pop a product screen | 511 / 70 µs | 43 / 6.8 µs |
| A press handled by the bundle (♥: an idea-ui `Button`, bundle state, its label) | 8.8 µs | 0.4 µs |
| The app sets a signal the bundle reads (cart in the header) | 12 µs | 0.5 µs |
| The app toggles context that adds and removes 3 bodies | 134 µs | 7.7 µs |
| An idea-ui `Slider` drag (its handler is bundle code) | 21 µs | 2.4 µs |
| A theme swap (the screen's token sheets restyle) | 49 µs | 50 µs |
| Memory per mounted `FeedScreen` | 146 KB | 127 KB |
| Bundle linear memory, idle / with 200 feeds mounted | 320 / 576 KB | — |

**Compute** (`showcase/app/src/bench.rs`: the same functions as wasm in the interpreter and as native code, checksums compared; median of 5):

| | wasm (opt z bundle) | wasm (opt 3 bundle) | native |
|---|---|---|---|
| Recursive Fibonacci(27): calls | 10.2 ms (20×) | 9.4 ms (18×) | 0.52 ms |
| FNV-1a over 256 KB ×8: integer math | 44.5 ms (16×) | 31.0 ms (11×) | 2.70 ms |
| Build + parse 20k JSON-like records: allocation, strings | 99.8 ms (49×) | 52.3 ms (26×) | 2.03 ms |
| Sort 200k u32s: memory, branches | 177.7 ms (87×) | 79.9 ms (39×) | 2.04 ms |
| 96×96 f64 matrix multiply (native vectorizes) | 45.8 ms (339×) | 32.4 ms (240×) | 0.13 ms |

UI work costs 9–27× in-process and stays well under a frame; heavy computation costs 16–340× and belongs in the app (a `#[host_fn]`) when it matters.

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


## History: model A

Model A was the spike's first design: a **host-owned graph with hand-written node descriptions** (crates `stream-guest`, `stream-macros`, `spike/guest`, `spike/demo`'s second half, and most of `stream-abi` / `stream-host`). A bundle linked a small guest runtime of its own: a guest `signal()` was a `u32` handle into a per-bundle table of real signals in the app's world, a guest `effect()` a real app effect whose body called back into the guest, and a component returned a `Node` description (`view` / `text` / `button` / host component by name) that the app turned into `runtime_scene::Element`s. Components declared their props in a `bundle!` macro, which went into a manifest the app checked before mount (prop types, required props, `Signal` → `ReadSignal` narrowing allowed, the reverse refused). Bundles were small, around 37 KB. But `ui!` couldn't run inside one — it expands to the framework's full author API, about 220 items — so model A would have needed a second implementation of that API. The bridged design replaced it, carrying over `#[host_fn]` (now `runtime_core::host_fn`) with its load-time import check, and native host components imported by stable name rather than `TypeId`. Model A was deleted; its code is in git history (`3bf2ae0d`). What it established still holds:

- **One graph, owned by the app.** A guest that never owns reactive state can't see app state a frame late. The bridged kernel keeps that property with the real kernel API instead of handles.
- **Bundle builds are marked by `--cfg idealyst_stream_guest`, not a cargo feature.** Features unify, so one `cargo build` covering both the app and a bundle crate would hand the app bundle stubs in place of its own functions.
- **Anything the app imports by name, never by `TypeId`**, since a `TypeId` only means something inside one compiled binary.
- **Re-entering the bundle mid-call goes through the wasmi `Caller`.** An effect created during a bundle call runs its body (a bundle call) at once; borrowing the `Store` again would panic. The bridged loader publishes each import's `Caller` for the same reason (`kernel.rs`, "Re-entrancy").

### Model A measurements

Apple M3 Max, host-mock scene, medians. The guest was the 28 KB `spike/guest` (release, opt-level z, no wasm-opt). "Native" is the same component compiled into the binary. These numbers were taken with wasmi's tail-call dispatch, before the switch to portable dispatch; see model B's finding 1 above for that cost.

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

### What the measurements found (still true)

1. **wasmi must be built at `opt-level = 3`.** The workspace release profile is size-optimized (`"z"`). Built that way, wasmi was about 2.5× slower in every phase. The root `Cargo.toml` now overrides opt-level for the wasmi crates and `wasmparser`. Cargo only reads profiles from the root workspace, so **an app that streams components needs the same lines in its own `Cargo.toml`.**
2. **The guest's wasm stack should be small.** rustc defaults the wasm32 stack to 1 MB, so the module's initial memory was 17 pages, and the interpreter zeroes all of it on instantiate. The spike's bundle builds (`guest_build_command`) still link every bundle with `-zstack-size=65536`, which brings initial memory down to 2 pages.
3. **Formatting code is a big part of bundle size.** One `format!("{:.0}", f64)` pulled in float formatting and took the bundle from 29 KB to 66 KB. Integer math brought it back to 37 KB. Avoid float and `{:?}` formatting in bundles, or move them to the host.
4. **Bundle-side helpers can cost more than the crossing.** Model A's guest read helper, with a thread-local `RefCell<Vec>` buffer, added about 300 ns per read in the interpreter; reading into a 64-byte stack buffer removed most of that. Code that runs in the interpreter is ~20× native, so small bundle-side overheads show up.

## Not covered yet

- **On-device numbers.** The bridged design's numbers are from a Mac. iPhone and Android runs are next.
- **Plain value props are fixed at mount.** A live prop is declared `ReadSignal<T>`; `#[component(remote)]` could make plain props reactive by default, as `#[props]` does natively.
- **No manifest.** A manifest listing the props, app components and context names a bundle needs, checked before mount, is still to do (by choice: see where things break first). `remote` can't be combined with `lazy` yet.
- **Every builtin primitive crosses except `graphics`** (and `lazy`, which has no meaning in a bundle). `crossing` records each decision. `graphics` never will: it hands the author's code a native GPU surface, which interpreted wasm can't drive — draw in an app component and use that from the remote component.
- **Nested bundles**, where one bundle mounts another bundle's component by name.
- **A poisoned bundle can keep running in one case.** If a host→bundle call made *from inside* a bundle call traps (a bundle calling `flush`, whose effects then panic), the outer bundle frame is still on the stack and resumes. Its later imports still reach the app's graph. Imports could refuse a poisoned bundle, at the cost of every import returning a `Result`.
- **Host handles.** Large or native results (a photo, a capture session) should cross as scoped handles, not bytes. The spike's `Photo` is a small value.
- **Bundle signing, and caching compiled modules by content hash.**
- **Interned strings are never freed.** What crosses as owned data but is `'static` where it's used (an icon's paths, a route name, a `test_id`) is interned once per distinct value, for the life of the process. Bounded by the distinct values bundles send — fine for real UI, unbounded for a bundle that builds names dynamically across many reloads.
- **Bundle-only helpers are plain functions.** In a bundle build every `#[component]` that isn't remote is an import of the app's copy, so a helper that exists only in the bundle (the showcase's `product_list`) can't be a `#[component]` — it's a plain fn with positional arguments. A `#[component(bundle)]`-style marker (compiled into the bundle, not imported) would let it be one.

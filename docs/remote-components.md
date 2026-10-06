# Remote components

A remote component is part of your app's UI that ships separately from the
app binary. You write it in ordinary Rust with `ui!`, like any component, and
mark it `#[component(remote)]`. It is compiled to WebAssembly as a **bundle**.
Your app downloads the bundle at runtime and runs it on the device, and the
component renders natively, styled by your app's theme, next to your app's own
components.

Use it to:

- **Change screens without an app-store release.** Ship a new bundle, and the
  installed app shows the new screens on its next load.
- **Drive UI from the server.** Send different bundles to different users or
  builds.

The rest of the app (navigation shell, native SDKs, settings, anything not
marked `remote`) stays in the binary as usual.

This guide shows how to use the feature. How it works inside is in
[remote-components-internals.md](remote-components-internals.md).

## Where it runs

- **iOS, Android, macOS, Linux, Windows:** remote components load from bundles.
- **Web:** `remote` does nothing. A remote component compiles into the web
  build like any other component, since the web app is itself downloaded.

Bundles run in [wasmi](https://github.com/wasmi-labs/wasmi), an interpreter, so
they work where native code can't be downloaded and generated code can't run
(iOS). Interpreted code is slower than native: UI work stays well within a
frame, and heavy computation should be handed to the app (see
[Calling native code](#calling-native-code)).

## Quick start

### 1. Put the remote components in a library crate

A bundle is a library crate. It can be a crate of its own (`shop-screens`), or
your app's own library. It needs runtime-vocabulary's `remote` feature:

```toml
# shop-screens/Cargo.toml
[dependencies]
runtime-core = { version = "1", registry = "idealyst" }
runtime-vocabulary = { version = "1", registry = "idealyst", features = ["remote"] }
idea-ui = { version = "3", registry = "idealyst" }   # if you use it
```

```rust
// shop-screens/src/lib.rs
use runtime_core::{component, ui, Element, ReadSignal, Signal};

#[component(remote)]
pub fn Offer(title: String, cart: Signal<u32>, compact: ReadSignal<bool>) -> Element {
    ui! {
        view() {
            text() { title }
            if !compact.get() {
                text() { "Free shipping this week" }
            }
            button(label = "Add", on_click = move || cart.update(|n| n + 1))
        }
    }
}
```

### 2. Host it in the app

The app depends on the same crate (so it can render `Offer`) and on the loader,
`remote-host`:

```toml
# app/Cargo.toml
[dependencies]
shop-screens = { path = "../shop-screens" }
runtime-vocabulary = { version = "1", registry = "idealyst", features = ["remote"] }
remote-host = { version = "1", registry = "idealyst" }
```

Install the bundle **before** any remote component mounts, then use the
component like any other:

```rust
fn app() -> Element {
    // Bytes you downloaded, cached, or shipped with the app.
    let bundle: Vec<u8> = load_bundle_bytes();
    let remote = remote_host::remote::install(&bundle).expect("bundle loads");
    keep_for_reloads(remote);

    let cart = signal(0u32);
    let compact = signal(false);
    ui! { Offer(title = "Trail Mug".to_string(), cart = cart, compact = compact.read_only()) }
}
```

In the app's native build, `Offer`'s body isn't compiled in: the function sends
its props to the installed bundle and mounts what the bundle builds. Mounting a
remote component before a loader is installed panics, naming the component.

### 3. Build the bundle

Declare it in the app's `Cargo.toml`, then build:

```toml
[package.metadata.idealyst.remote]
bundles = [{ name = "shop", package = "shop-screens" }]
```

```sh
rustup target add wasm32-unknown-unknown    # once
idealyst build --remote
# → target/idealyst/remote/shop.wasm and shop.json
```

Serve `shop.wasm` however you like. To update the screens, build again and have
the app install the new file.

## Writing remote components

A remote component is an ordinary component in a bundle crate: `ui!`, signals,
effects, `memo`, stylesheets, async tasks, idea-ui, your own helper functions.
What's different is only at the edges, where it meets the app.

### Props

Props are taken exactly as declared:

| Prop type | What the component gets |
|---|---|
| A value: numbers, `bool`, `String`, `Vec`/`Option` of them, style types (`Color`, `Length`, `StyleRules`, …) | A copy, fixed at mount |
| Your own type with `#[derive(Remote)]` | A copy, field by field |
| `ReadSignal<T>` | The app's signal, read live: when the app changes it, the component updates |
| `Signal<T>` | The app's signal, read live, and the component can write it |
| `Ref<StackHandle>`, `Ref<NavHandle>`, `NavHandle`, `Option<StackHandle>`, … | The app's navigator, to navigate (see [Navigation](#navigation)) |

A plain value doesn't change after mount. For a live value, declare
`ReadSignal<T>`. To report something back to the app, take a `Signal<T>` and
write it: callbacks and children aren't props a remote component can take.

### Your own types: `#[derive(Remote)]`

A type crosses between the app and a bundle when it derives `Remote`:

```rust
#[derive(Clone, Debug, PartialEq, Remote)]
pub struct Product { pub id: u32, pub name: String, pub price_cents: u64 }
```

Put it in a crate both sides compile (the bundle crate, or a shared crate). It
works as a remote component's prop, as a signal's value, as context, and as a
host function's argument or result. The derive expands to nothing in an app
with no remote components, so a library can derive it on every value type.

### Using the app's components

**Every component that isn't marked `remote` lives in the app.** When a remote
component renders `Card()` or `Button(…)`, the bundle doesn't contain them: it
asks the app for its own copy, by name, and the app renders it natively. So:

- idea-ui works in remote code unchanged, and renders with the app's idea-ui.
- Your app's own components are available to remote code the same way.
- Only the props the call site sets are sent. The app fills in its own
  defaults for the rest.
- If the app doesn't have a component the bundle asks for (an older app with a
  newer bundle), that component shows an error in its place, and the rest of
  the screen renders.

A consequence: a helper that should exist only in the bundle can't be a
`#[component]` (it would be an import of the app's copy). Write it as a plain
function.

### Styling and themes

Stylesheets in remote code name the theme's **tokens** (`t.color.background()`),
and the app resolves them against its installed theme. So remote screens follow
the app's theme, including light/dark switches, exactly as native screens do,
with nothing to wire up.

To change the app's theme from remote code, call idea-ui's
`set_idea_color_scheme(..)`, which is a host function and switches the app's
own theme.

### Context

Remote code reads the app's context with `inject`, for types that derive
`Remote` (and `Clone`):

```rust
#[derive(Clone, Remote)]
pub struct FeedPrefs { pub compact: ReadSignal<bool> }

// In the app:    provide(FeedPrefs { compact: compact.read_only() });
// In the bundle: let prefs: Option<FeedPrefs> = inject::<FeedPrefs>();
```

Signals inside it stay live. Context types that don't derive `Remote` are
invisible to bundles.

### Navigation

A remote component can be a screen in the app's navigator, and navigate it with
the same handles and typed routes as native code:

```rust
#[component(remote)]
pub fn Home(nav: Ref<StackHandle>) -> Element {
    ui! { button(label = "Open", on_click = move || if let Some(h) = nav.get() { h.push(&DETAIL, ItemId(7)) }) }
}
```

Define the routes once, in a crate both sides compile. A remote component can
also define a whole navigator (its screens, typed params, layout and header):
the navigation machinery is the app's, and the screens are the bundle's.

### Refs

Refs work as natively: `text_input(bind = input)`, `on_handle`, layout
subscriptions, anchoring a popover to a node. Calls on a handle go to the app's
real node.

## Calling native code

Code in a bundle can't call platform APIs. When remote code needs the device
(the camera, the file system, the network), a native SDK, or fast computation,
the app provides a **host function**:

```rust
// In a crate both sides compile.
#[host_fn]
pub fn device_name() -> String { … }

#[host_fn]
pub async fn fetch_reviews(product: u32) -> Vec<Review> { … }
```

In the app it is the function as written. In the bundle it becomes a call to
the app. Remote code calls it like any function:

```rust
let name = device_name();
spawn_then(fetch_reviews(id), move |reviews| state.set(reviews));
```

The app lists what bundles may call when it installs the loader:

```rust
use remote_host::remote::{install_with_options, Options};

install_with_options(&bundle, Options {
    host_fns: vec![device_name::export(), fetch_reviews::export()],
    ..Options::default()
})?;
```

A bundle that calls a function the app didn't list, or calls one whose
signature has changed since the app was built, is refused when it loads, with
an error naming the function. Libraries can ship host functions too: idea-ui's
functions that change app state (`push_toast`, `set_idea_color_scheme`, …) are
host functions, and `idea_ui::host_fns()` lists them.

Arguments and results can be any type that crosses: values, `Vec`, `Option`,
tuples, `Result`, and `#[derive(Remote)]` types. A list of numbers crosses as
one block of memory, so passing large numeric data is cheap.

### Generic host functions

A host function can be generic, and a bundle can call it with **its own
types**, ones the app never compiled:

```rust
#[host_fn]
pub fn sort_order<K: Key>(keys: Vec<K>) -> Vec<u32> {          // any key
    let mut order: Vec<u32> = (0..keys.len() as u32).collect();
    order.sort_by(|a, b| keys[*a as usize].cmp(&keys[*b as usize]));
    order
}

#[host_fn]
pub fn group_by<K: Key, V>(rows: Vec<(K, V)>) -> Vec<(K, Vec<V>)> { … }

#[host_fn]
pub fn stats<N: Numeric>(values: Vec<N>) -> Stats { … }
```

```rust
// In the bundle, with a type only the bundle has:
#[derive(Clone, Key)]
struct Seniority { team: String, hired: u16 }

let order = sort_order(staff.iter().map(|s| Seniority { team: s.team.clone(), hired: s.hired }).collect());
```

What a type parameter can be depends on its bound:

| Bound | The function may | Works for |
|---|---|---|
| `K: Key` | compare, hash, clone | any `Key` type, including ones only the bundle has |
| `V` with no bound (or `Opaque`) | move it around (and clone, with `+ Clone`) | any `#[derive(Remote)]` type |
| `N: Numeric` | arithmetic, `total_cmp`, `to_f64` | the number types, `u8` to `f64` |
| any other trait (`S: Area`) | whatever the trait offers | the types the app lists |

- **`#[derive(Key)]`** makes a struct or enum a `Key`. It also gives it `Eq`,
  `Ord` and `Hash`, from its fields in order (an enum's variant first), so don't
  derive those separately. `f32`/`f64` fields compare with `total_cmp`. Numbers,
  `String`, `bool`, `char`, `Vec`, `Option`, tuples, arrays and
  `std::cmp::Reverse` (for descending order) are already keys.
- **Another trait** needs the type's own code, so it only works for types the
  app knows. List them in place of `export()`:
  `runtime_vocabulary::host_fn_instances!(shapes::total_area: Circle, Rect)`.
  A bundle calling it with an unlisted type is stopped, with an error naming
  it.
- A generic parameter can appear as itself, or inside `Vec`, `Option`, tuples
  and arrays. A map crosses as `Vec<(K, V)>`.
- **Closures aren't allowed** (`F: Fn(&T) -> bool`): calling back into the
  bundle for every element would be slower than doing the work in the bundle.
  Pass the data the closure would compute instead (a key, a range, a list of
  fields).

**When it's worth it:** the bundle still encodes every value it sends, in
interpreted code. Sorting 200k numbers through the app takes 7.5 ms, against
176 ms in the bundle. Sorting 200k items by a two-field key takes 67 ms against
196 ms. Linear work (dedup, counting, one pass of group-by) gains about 2×.

## Loading, updating and errors

```rust
use remote_host::remote::{install, install_with, install_with_options, Options};

let remote = install(&bytes)?;           // no host functions
remote.reload(&newer_bytes)?;            // every mounted remote component remounts from it
```

- **Reload** swaps the bundle while the app runs. Every mounted remote
  component remounts from the new bundle. App state (the signals you passed as
  props, context) carries over; state the components kept for themselves starts
  over. If the new bundle doesn't load, the current one keeps running.
- **Several bundles:** `install_empty(options)` starts with none, and
  `remote.set("shop", &bytes)` adds or replaces one by name. Each remote
  component mounts from the bundle that exports it. Replacing one bundle
  remounts only its own components; the others keep their state.
  `remote.on_missing(|component| …)` hears about a component no bundle
  provides yet, which shows an empty place until one is set.
- **Downloading:** the [`ota`](ota.md) crate and `idealyst ota publish`
  deliver bundles over the air, from static files on a CDN. Or use your own
  HTTP client and call `install`, `set` or `reload` with the bytes.
  (`remote_host::fetch` is a plain `http://` GET for local development, not for
  production.)
- **A bundle that fails to load** returns an `Err` saying why: not valid wasm,
  an unlisted or changed host function, a bundle built against a different
  framework codec ("rebuild the bundle"), a refused signature, or a release
  bundle that needs something this app doesn't have (below).
- **A panic in a bundle never takes the app down.** The bundle is stopped, and
  each of its components shows the panic message in its place. The rest of the
  app keeps running, and reloading a fixed bundle brings the components back.
  The same happens to a bundle that sends the app something invalid.
- A bundle that loops forever blocks the UI thread: there is no time limit.

## Building bundles for release

Declare each bundle in the app's `Cargo.toml`. A bundle is a library crate in
the app's workspace, and the app's own library can be one:

```toml
[package.metadata.idealyst.remote]
bundles = [
  { name = "shop", package = "shop-screens" },
  { name = "home", package = "my-app" },
]
```

```sh
idealyst build --remote                         # every bundle
idealyst build --remote --bundle shop           # just one
idealyst build --remote --remote-out dist/remote
```

For each bundle, the build:

- compiles the crate's library for `wasm32-unknown-unknown`, as a bundle;
- checks it only imports what the loader provides. If a crate the bundle
  depends on uses web bindings (wasm-bindgen) or C code, the build fails and
  names the imports;
- stamps it with its name, crate, version, and the framework codec version it
  was built with;
- records what it requires of an app (below);
- signs it, if you pass a key (below);
- writes `<name>.wasm` and `<name>.json`. The JSON holds the size, SHA-256,
  signing key and requirements, for your server or cache.

**Dependencies only the app needs** (the loader, native SDKs, anything that
can't compile to wasm) go in a section the bundle build skips. This matters
when the app's own library is a bundle:

```toml
[target.'cfg(not(idealyst_stream_guest))'.dependencies]
remote-host = { version = "1", registry = "idealyst" }
```

In code, `#[cfg(not(idealyst_stream_guest))]` marks what only the app compiles
(add `unexpected_cfgs = { level = "warn", check-cfg = ['cfg(idealyst_stream_guest)'] }`
under `[lints.rust]` to keep rustc quiet about the name).

### Which apps a bundle runs on

A release bundle carries a list of what it needs from the app, and an app
checks the list before running any of the bundle. The list holds:

- each app component the bundle builds, and each prop it sets, with the prop's
  type;
- each host function it calls, with its argument and return types;
- the parameters of each remote component it provides;
- the framework codec it was built with.

So a bundle runs on every app that has at least those, with the same types. An
app that adds a component or a prop still runs older bundles. A bundle that
starts using something newer is refused by older apps, with every problem
named:

```text
bundle needs what this app doesn't have:
  `idea_ui::components::button::Button` has no prop `glow` in the app;
  host function `shop::checkout`: the bundle calls `fn(Cart{items:list<str>,coupon:str})->u64`,
  the app has `fn(Cart{items:list<str>})->u64`
```

- **Only what the bundle actually uses counts.** The list comes from the
  bundle's reachable code: a component used only in a function nothing calls
  isn't on it, and a prop counts only if some call site sets it.
- **Types are compared by structure.** A `#[derive(Remote)]` struct that gained
  a field is a different type, even under the same name. Values cross in field
  order, so it would otherwise be misread.
- **`remote_host::remote::provides(&host_fns)`** gives the app's side, and
  `remote_bundle::check(&requires, codec, &provides)` compares them. That's how
  a server or an update client can pick a bundle an app can run before
  downloading it. `idealyst remote inspect` prints a bundle's list.
- **Not covered:** a value that crosses by key (a tone or variant name) is
  only checked when it crosses. A key this app doesn't have, as an app
  component's prop, shows an error in the remote component's place, naming
  the key. In a host function's argument, a signal or a callback it becomes
  the type's default instead (the default tone, say), and the app logs a
  warning naming it: there's no place to show an error, and failing the call
  would stop the whole bundle. Types the framework
  defines (`Color`, `StyleRules`) are covered by the codec version, not by
  structure. A context type the bundle reads is compared as a warning only,
  since the list can't tell which ones it reads.

Development bundles (`idealyst dev`, a build script's embedded copy) carry no
list. They're checked as they run: a missing component or prop shows an error
in that component's place.

## Signing bundles

Signatures are optional. An app that downloads its bundles should require them,
so it only runs bundles you built.

**Once, create a key:**

```sh
idealyst remote keygen --out release.key
# private key  release.key (keep it secret)
# key id       acc15dd056392849
# public key   c5dcf42a15f9549735d43964d671e041632796d35e19398c6163f06fe5a3d99f
```

Keep `release.key` out of version control. Anyone with it can sign bundles your
app will run.

**Sign when you build:**

```sh
idealyst build --remote --sign-key release.key
# or, in CI, with the key's contents in an environment variable:
IDEALYST_REMOTE_SIGNING_KEY=… idealyst build --remote
```

**Require the signature in the app:**

```rust
use remote_host::remote::{install_with_options, Options, PublicKey, Trust};

const RELEASE_KEY: &str = "c5dcf42a15f9549735d43964d671e041632796d35e19398c6163f06fe5a3d99f";

let remote = install_with_options(&bundle, Options {
    host_fns: vec![/* … */],
    trust: Trust::default()
        .key(PublicKey::from_hex(RELEASE_KEY)?)
        .require_signature(),
})?;
```

The check runs before any of the bundle runs, at install and at every reload. A
refused reload keeps the current bundle.

| App's policy | Unsigned | Signed by an unknown key | Signed by a trusted key | Changed after signing |
|---|---|---|---|---|
| `.key(..).require_signature()` | refused | refused | loads | refused |
| `.key(..)` only | loads | loads | loads | refused |
| default | loads | loads | loads | loads |

- **Rotating keys:** trust both the old and the new key (`.key(old).key(new)`),
  sign new bundles with the new one, and drop the old key from a later app
  version.
- **Other tools:**
  - `idealyst remote sign bundle.wasm --key release.key` signs a bundle built
    elsewhere, or re-signs it.
  - `idealyst remote verify bundle.wasm --public-key <hex>` checks one.
  - `idealyst remote inspect bundle.wasm` prints what a bundle says it is.

The signature is stored inside the `.wasm` file, so a signed bundle is still one
file to serve and cache.

## Developing

- **Run remote components in-process.** Turn on runtime-vocabulary's
  `remote-inline` feature in the app, and remote components compile into the app
  and run natively, with no bundle. It's useful for debugging, and for measuring
  a component against itself.
- **Try a new bundle without rebuilding the app.** Build with
  `idealyst build --remote` and have a development-only button or hot key call
  `remote.reload(..)` with the new file.

## Limits

- **`graphics` can't be used in remote code.** It hands your code a GPU
  surface, which interpreted code can't drive. Draw in an app component and use
  that from the remote component.
- **No callbacks or children as remote component props.** Pass signals, and
  have the component write a `Signal<T>` the app watches.
- **One bundle can't use another bundle's remote component.** Both run in
  the same app, side by side, but a remote component's tree can only hold app
  components and its own bundle's.
- **Large native results** (a photo, a video frame) cross as bytes. Keep them in
  the app and pass small values or ids to the bundle.
- **No time or memory limit** on bundle code. Bundles are your own code, signed
  by you. They're not a sandbox for untrusted third parties.
- **Measured on a Mac only so far.** iPhone and Android numbers are still to
  come.

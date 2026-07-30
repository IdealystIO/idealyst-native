+++
title = "SDKs & opt-in crates"
order = 65
tags = ["sdk", "crates", "net", "storage", "credentials", "discovery"]
+++

# SDKs & opt-in crates

The framework core (`runtime-core`) ships only the lowest UI primitives and the
reactive system. Everything else — networking, persistence, camera, maps, a full
component library — lives in **separate opt-in crates** you add to your project's
`Cargo.toml` as you need them. This keeps binaries small: you only link what you
use.

These crates are **not** in the `list_components` / `list_primitives` catalog
surface, because they expose plain Rust functions and types (e.g.
`net::Client`, `storage::platform_storage()`), not `#[component]`s or `ui!`
primitives. This guide is the index for them.

## Adding an SDK

Add the crate the same way your project references `runtime-core` — by the bare
crate name (no `idealyst-` prefix), pointing at the same source your
`runtime-core` line uses (git `rev`, path, or workspace):

```toml
[dependencies]
net = { git = "https://github.com/.../idealyst-native", rev = "<same rev as runtime-core>" }
storage = { git = "...", rev = "..." }
```

Inside the workspace, examples use `net = { workspace = true }`. After adding the
dep, the SDK's functions are importable (`use net::Client;`).

## Networking & data

| Crate | What it gives you |
|---|---|
| **`net`** | Cross-platform async networking — HTTP, WebSocket, and Server-Sent Events. `net::Client` is the HTTP entry point. The transport layer the server-functions layer composes (see [[server-functions]]). |
| **`server`** | Full-stack server functions — `#[server] async fn`, `server::configure`, `server::router`, extractors, auth guards. See the dedicated [[server-functions]] guide. |
| **`storage`** | Cross-platform **insecure** key-value storage for non-sensitive app data. For signal-backed state PREFER `storage::persisted_signal(ns, key, initial)` (feature `reactive`): hydrates on creation, persists on change, and a user write before the async load resolves WINS — don't hand-roll load/persist wiring, it almost always clobbers that write. Raw store: `storage::platform_storage(ns) -> Arc<dyn Storage>`; async `get(key) -> Result<Option<String>, _>` / `set` / `remove` (see "Calling async APIs" below). Backends: localStorage / NSUserDefaults / SharedPreferences / JSON file. Once storage is a dependency, the `persisted_signal` recipe registers in `list_recipes`. Use `credentials` for secrets. |
| **`credentials`** | Cross-platform **secure** storage for secrets (auth tokens, API keys) — Keychain / Keystore on device. Web errors rather than faking security. |
| **`files`** | Cross-platform blob/file storage for **binary data** (recordings, downloads). |
| **`file-export`** | Save a file to a user-chosen location through the platform's native "save" UI (no permission prompt). |
| **`i18n`** | Localization / translation / multi-language — the internationalization SDK. Declare translations inline with the `i18n!` macro in a `mod t` (`locales: { En = "en" (default), Es = "es" } greeting(name) { En: "Hello, {name}", Es: "Hola, {name}" }`); a missing translation or bad `{placeholder}` is a COMPILE error. Each message is a fn returning `Reactive<String>` you pass to any reactive-text prop (`Typography(content = t::greeting("Ada"))`). Switch language live with `t::set_locale(t::Locale::Es)` / `i18n::set_locale_code("es")` — every visible translated string re-renders in place. Bundled locales compile in; `(lazy)` locales fetch a JSON pack (feature `lazy-fetch`). Full walkthrough in the [[i18n]] guide. |

## Media & capture

| Crate | What it gives you |
|---|---|
| **`media-stream`** | A platform-agnostic handle to a live video source — the common abstraction camera / screen-recorder yield. |
| **`camera`** | Cross-platform camera capture → a `MediaStream`. |
| **`microphone`** | Cross-platform microphone capture → an audio stream. |
| **`screen-recorder`** | Cross-platform screen / window recording → a `MediaStream`. |
| **`media-writer`** | Record live media streams to a file (mp4). |
| **`video`** | Third-party `Video` playback primitive (a scene-registry payload). |
| **`canvas`** | The author-facing facade for the 2D-drawing SDK (GPU canvas + self-capture compositor). |

## UI primitives & extensions (scene-registry payloads)

These are third-party UI primitives. The framework has one dispatch model for
every primitive: a **payload handler on the scene `Registry`**. First-party
primitives (`view`, `text`, …) register through
`runtime_vocabulary::register_builtins`; a third-party SDK registers exactly
the same way. Adding the crate and calling the primitive in `ui!` is **not
sufficient**: an unregistered payload **panics at realize**. See "Registering
extension SDKs" just below.

| Crate | What it gives you |
|---|---|
| **`idea-ui`** | The cross-platform **component library** — `Button`, `Card`, `Field`, `Select`, etc. Most apps depend on this. Its components ARE catalogued (`list_components`) once linked. |
| **`idea-theme`** | Theming abstraction + extensibility for the idealyst design system. |
| **`icons-lucide`** | Lucide icon pack — only icons you reference end up in the binary. |
| **`webview`** | Third-party `WebView` primitive. The canonical single-crate cfg-gated External pattern. |
| **`maps`** | Third-party `MapView` primitive. |
| **`svg`** | Third-party SVG renderer. |
| **`markdown`** | CommonMark/GFM document primitive. |
| **`codeblock`** | Read-only colored-text (code) panel primitive. |
| **`table`** | Cross-platform table — a real `<table>` on web. |
| **`form`** | Third-party `Form` SDK. |
| **`toolbar`** | Third-party `Toolbar` SDK. |
| **`menu`** | OS-level menu-bar SDK (desktop). |

### Registering extension SDKs

Every extension SDK (`webview`, `maps`, `svg`, `markdown`, `codeblock`,
`table`, `toolbar`, `video`) exposes a `register(&mut registry)` that installs
its payload handler. Registration is **mandatory on every target**: the scene
registry has no fallback handler, so a payload with no handler is a panic at
realize, not a placeholder. (This is deliberate — a missed `register` fails
loud instead of rendering a silent grey box.)

Call it from your crate's `register_scene_extensions` — the seam the
CLI-generated wrapper invokes after `runtime_vocabulary::register_builtins`:

```rust
pub fn register_scene_extensions<H: runtime_scene::Host>(
    registry: &mut runtime_scene::Registry<H>,
) {
    table::register(registry);      // real <table> on web, CSS-grid on native
    markdown::register(registry);   // CommonMark handler
    // …one line per extension SDK you render.
}
```

Most SDK `register` fns are **caps-generic** (`register<H>(&mut Registry<H>)`)
and serve every backend from one handler. An SDK with a real platform leg
(`svg`'s iOS/Android vector walk, `video`'s AVPlayer, `toolbar`'s `NSToolbar`)
selects it by **registry type**, not by `cfg`: its `register` downcasts the
registry to the concrete backend's `Registry<MacosBackend>` / `<IosBackend>` /
`<AndroidBackend>` and falls through to the portable handler otherwise. A
`cfg(target_os = "macos")` split alone cannot express that — the cfg is
equally true for an SSG render running on a macOS host, which needs the
portable handler. If your `register_scene_extensions` needs to name a concrete
backend, specialize `H` to that backend's registry type in your app crate and
add the backend dep.

SSR needs the same handlers so first paint matches the client (see the
website's `examples/serve.rs`, which registers alongside the web build).

### Code-splitting a heavy extension (web)

If an extension SDK is large but used in only one corner of the app, wrapping
the *usage* in `#[component(lazy)]` splits that corner's **rendering** code
into a chunk. What it does **not** split on its own is the SDK's handler: a
handler named in `register_scene_extensions` at boot — and everything it
statically reaches — is reachable from `main.wasm`.

Registration is the anchor, and the registry has a **late-registration seam**
for moving it out of main: declare the payload kind at boot with
`registry.defer::<T>()` (which lets `realize` park an item of that kind instead
of panicking), then install the real handler from inside the chunk with
`runtime_scene::defer_registration` → `Registry::register_deferred`. This is the
runtime-v2 successor to the pre-v2 core's `defer_external_registration`. Full
recipe and the size measurement in [[lazy-loading]].

Practical consequences:

- Split the **body**, and defer the **handler** when the handler is the heavy
  part. If you do register at boot instead, keep the handler thin: the heavy
  work (a rasterizer, a font stack, an embedded payload) should live behind a
  function the *chunk* calls, not behind one the handler calls at
  registration time.
- Static **data** never leaves `main.wasm` by default regardless of chunking;
  dropping it needs the experimental opt-in `idealyst build --web --release
  --data-prune` (see [[lazy-loading]]).
- Register only the SDKs you actually render. An unused `register` line costs
  its handler's whole reachable graph in the main bundle — and now that
  registration is explicit everywhere, that list is entirely under your
  control.

### There are no `prim-*` features

Earlier releases let you trim whole primitive families out of the build with
twelve `prim-*` cargo features on `runtime-core` (mirrored by each backend
crate), six matching ones on `idea-ui` that `#[cfg]`-deleted the components
rendering each family, and `idealyst build --web --primitives=…` to select the
set. **All three layers are gone**, along with the SDK-side forwards that fed
them (`virtualized` → `prim-virtualizer`, the navigators → `prim-navigator`).
They gated three things in the pre-v2 core — walker dispatch arms, authoring
builder fns, and `Backend` trait methods — and none of them exist now.

Nothing in your source changes: no type, function, macro, or component was
lost, and the component set is unconditional. Manifests and build commands do:
drop any `features = ["prim-…"]` entry (cargo rejects the manifest otherwise)
and drop `--primitives` (the CLI rejects it with a pointer to the migration
guide). If you want a smaller bundle, the lever is now **what you register** —
see "Register only the SDKs you actually render" above.

The structural successor for primitives themselves is per-primitive **handler
registration**: `runtime_vocabulary::handlers::register_builtins` holds the
only reference to each primitive's module and its caps calls, so a per-family
gate belongs there (and in each backend's caps impls). Until that lands,
keeping the old flags would have shipped a lever that deletes components from
the public API while saving nothing in the bundle. See
`docs/migrating-to-runtime-v2.md` for the full before/after, and
[[migration-0-4-0-to-0-5-0]] for the historical contract.

## Device & platform integration

The OS-integration capabilities. `permissions` is the shared runtime-grant
substrate: any capability that prompts the user (`notifications`, `location`,
and the media SDKs `camera` / `microphone`) goes through it rather than
re-implementing an OS prompt. Each capability SDK declares its own build-time
permission requirement (`[package.metadata.idealyst] capabilities = [...]`); the
app supplies the reason string.

| Crate | What it gives you |
|---|---|
| **`permissions`** | Cross-platform **runtime permission** requests — the shared grant substrate. `permissions::request(Permission)` / `status(Permission)` → a uniform `PermissionStatus`. Other SDKs depend on this instead of re-implementing a grant flow. |
| **`biometrics`** | Cross-platform biometric authentication ("prove the device owner is present"). |
| **`notifications`** | Local + scheduled notifications and the raw device push token. Authorization goes through `permissions`; server-side push delivery is the app's job. |
| **`location`** | Device geolocation — one-shot `current()` + continuous `watch()` yielding a `Position`. Permission grant goes through `permissions`. |
| **`clipboard`** | System copy/paste of plain text — `clipboard::set_text` / `text`. |
| **`share`** | The system share sheet (outbound) — hand text/url/files to another app. The inverse of `file-picker`. |
| **`deep-link`** | Inbound URL handling — `initial_link()` + `on_link()` deliver the parsed launch/resume URL (custom scheme / universal / app link). |
| **`connectivity`** | Network reachability — `current()` snapshot + `watch()` of online/offline and coarse transport (wifi/cellular/ethernet). |
| **`haptics`** | Tactile feedback — `impact` / `notify` / `selection`. Fire-and-forget, best-effort. |
| **`audio`** | Sound playback — `load(AudioSource)` → a `Sound` you `play()`, with a controllable `Playback`. The playback peer of the capture SDKs. |

## How they relate to the catalog

- An SDK that ships `#[component]`s (like `idea-ui`) surfaces those components
  through `list_components` / `describe_component` **once it's a dependency of
  the build the catalog is extracted from**.
- An SDK that exposes free functions / scene-registry primitives (like
  `net`, `storage`, `webview`) is documented here and in its own crate docs —
  read the crate's `lib.rs` module docs for the full API.

When you're unsure which crate provides a capability, this list is the map:
networking → `net`, persistence → `storage` / `credentials` / `files`,
server relay → `server`, camera/mic/recording → the media crates.

## Calling async APIs from UI code

Several SDK surfaces (`storage`, `net`, …) are `async`. UI handlers and
component bodies are synchronous — bridge with
`runtime_core::driver::spawn_async`:

```rust
use runtime_core::driver::spawn_async;

let items = signal(Vec::new());
spawn_async(async move {
    let store = storage::platform_storage("my-app");
    if let Ok(Some(saved)) = store.get("items").await {
        items.set(parse(saved));
    }
});
```

No `Send` bound is required, and the executor is pre-installed by the
CLI-generated app wrappers on every platform — no setup in app code. Signal
writes inside the future notify the UI exactly like writes from a handler.

`spawn_async` exists only when the `runtime-core` dependency enables the
`async-driver` feature — CLI-generated wrapper Cargo.tomls do, but a
hand-written dep line must add it:

```toml
runtime-core = { path = "…", features = ["async-driver"] }
```

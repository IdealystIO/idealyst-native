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
| **`video`** | Third-party `Video` playback primitive (`Element::External`). |
| **`canvas`** | The author-facing facade for the 2D-drawing SDK (GPU canvas + self-capture compositor). |

## UI primitives & extensions (`Element::External`)

These are third-party UI primitives wired through `Element::External` + a
per-backend registry. Adding the crate and calling the primitive in `ui!` is
**not sufficient on web**: the primitive's handler must also be **registered**
with the backend, or it renders an `External "…Props" not supported`
placeholder at runtime (not a compile error). See "Registering External UI
SDKs (required for web)" just below.

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

### Registering External UI SDKs (required for web)

Every `Element::External` UI SDK (`webview`, `maps`, `svg`, `markdown`,
`codeblock`, `table`, `toolbar`, `video`) exposes a per-target
`register(&mut backend)` that installs its handler. On **native** the SDK
also self-registers via `inventory::submit!`, so the call is often a no-op
belt-and-suspenders. On **web under the CLI `--local` build** that inventory
submission can be dead-stripped, so the primitive renders the framework's
`External "…Props" not supported on web` placeholder (a runtime message, not
a compile error) unless the app calls `register` explicitly.

Call it from your crate's `register_extensions` — the per-target hook the
CLI-generated wrapper invokes before mount — on the wasm32 arm:

```rust
#[cfg(target_arch = "wasm32")]
pub fn register_extensions(backend: &mut backend_web::WebBackend) {
    table::register(backend);      // real <table> handler
    markdown::register(backend);   // CommonMark handler
    // …one line per External UI SDK you render on web.
}
```

`register` is defined for every target (a no-op on backends without a
binding), so a native/iOS/Android `register_extensions` can call the same
lines harmlessly — but the web arm is the one that's actually load-bearing.
SSR needs the same handlers so first paint matches (see the website's
`examples/serve.rs`, which calls `codeblock::register(b)` alongside the web
build). This is the **basic** requirement; the next section is the advanced
variant that defers registration into a code-split chunk.

### Code-splitting a heavy extension (web)

If an `External` SDK is large but used in only one corner of the app, you can
keep it out of the web **main bundle** so it downloads only when that corner
mounts. The catch: wrapping the *usage* in `lazy!` is not enough. An `External`
handler registered eagerly — at boot in `register_extensions`, or via an
`inventory::submit!` drained at backend construction — is statically reachable
from `main.wasm`, so wasm-split keeps the whole SDK there. **Registration, not
rendering, is the anchor.**

Move registration into the chunk. The SDK exposes a `register_lazy()` built on
`defer_external_registration`; the app calls it as the first line of the `lazy!`
body, then renders the primitive:

```rust
// In the SDK (web target): queue the handler instead of installing it now.
#[cfg(target_arch = "wasm32")]
pub fn register_lazy() {
    runtime_core::defer_external_registration::<backend_web::WebBackend, _>(|b| {
        b.register_external::<HeavyProps, _>(build_heavy::<backend_web::WebBackend>);
    });
}
#[cfg(not(target_arch = "wasm32"))]
pub fn register_lazy() {} // native registers eagerly; no chunk, no bundle cost

// In the app: register-then-render, both inside the chunk.
lazy! {
    heavy_sdk::register_lazy();
    heavy_sdk::widget(props)
}
```

Now `build_heavy` (and any static data it reaches) is reachable only from the
chunk, so the release data-prune drops it from `main.wasm`; the backend applies
the queued registration before dispatching the chunk's own `External`. This is
per-SDK opt-in — an SDK that wants it must NOT also `inventory::submit!` its web
handler (that submission is itself a main-bundle anchor); it keeps inventory
self-registration for native, where bundle size is a non-issue. Measured on a
512 KiB test SDK: main bundle 1294 KiB → 781 KiB. See the `lazy!` macro and
[[defer_external_registration]].

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
- An SDK that exposes free functions / `Element::External` primitives (like
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

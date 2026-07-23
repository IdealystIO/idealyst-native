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
| **`i18n`** | Lightweight, Rust-native internationalization — runtime half. |

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
per-backend registry — add the crate and call the primitive in `ui!`.

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
chunk, so wasm-split keeps its code out of `main.wasm` (its data leaves main only
under the experimental opt-in `idealyst build --web --release --data-prune`);
the backend applies the queued registration before dispatching the chunk's own
`External`. This is
per-SDK opt-in — an SDK that wants it must NOT also `inventory::submit!` its web
handler (that submission is itself a main-bundle anchor); it keeps inventory
self-registration for native, where bundle size is a non-issue. Measured on a
512 KiB test SDK: main bundle 1294 KiB → 781 KiB. See the `lazy!` macro and
[[defer_external_registration]].

Beyond lazy chunks, the framework itself is trimmable: runtime-core exposes
`prim-*` cargo features (all ON by default) that gate whole primitive
families out of the build — walker dispatch, backend implementation, and any
embedded JS shims. All twelve families are gated: `prim-virtualizer`
(flat_list / grids / the structured `for i in count(sig)` form), `prim-icon`,
`prim-image`, `prim-text-input` (TextInput + TextArea), `prim-toggle`,
`prim-slider`, `prim-activity`, `prim-portal` (overlay / anchored_overlay),
`prim-presence`, `prim-graphics`, `prim-navigator` (navigator + outlet +
URL sync + Link's nav dispatch), and `prim-lazy` (`#[lazy_component]` chunk
mounting). The supported opt-out is two-sided (cargo unifies features
across the build graph, so both edits must land together): the app crate
sets `default-features = false` on its `runtime-core` dependency, and the
build names the families the app's own code uses —
`idealyst build --web --release --primitives icon,text-input` (or
`--primitives none` for a text/view-only bundle). The build warns when the
app-side edit is missing, and an unknown family name is a hard error. A
view+text-only baseline drops from ~548 KB to ~392 KB raw (~133 KB brotli
over the wire). An SDK that renders through a gated primitive forwards the
feature on its runtime-core dep (see `virtualized`, `swap-navigator`,
`stack-navigator`) so depending on the SDK re-enables exactly what it
needs. Authoring a gated-out primitive is a compile error naming the
feature; one arriving at runtime — over the wire, or through a feature
mismatch between crates — renders the standard "not supported" placeholder
on every backend (never a panic). Full contracts and the SDK-author
checklist live in [[migration-0-4-0-to-0-5-0]].

Note for backend/intermediate crates: cargo ignores `default-features =
false` on `workspace = true` deps, and any dep line with default features
re-enables the whole `prim-*` set for everyone (feature unification).
Backend and utility crates that sit between the app and runtime-core
(`backend-web`, `css`) therefore declare runtime-core as a *path* dep with
`default-features = false` and forward per-family features explicitly —
without that, an app's opt-out silently does nothing.

The anchor rule is transitive: a *dependency's* `inventory::submit!` pins its
code just as hard as your own. An SDK that both self-registers (zero-config
eager use) and gets consumed as a delegate by another SDK should put the
submit behind a default-on cargo feature so the delegate consumer can take it
with `default-features = false`. `canvas-native` is the model: its
`self-register` feature is on for apps that depend on it directly, and off in
`canvas-vello`'s fallback-delegate dep — that alone moved the rasterizer +
font stack (~670 KB) from a lazy canvas app's `main.wasm` into the chunk.

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

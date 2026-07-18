# `sdk/` — opt-in capability crates

SDKs are organized by **tier**:

- **[`client/`](./client)** — capabilities that run in the app (device, media,
  UI primitives, navigators). Everything documented in the tables below lives
  here (paths are `client/<name>`).
- **[`server/`](./server)** — capabilities that run in the server / worker
  binary, building on the `server` RPC layer (`crates/api/server`) the way
  client SDKs build on `runtime-core`. See **[Server SDKs](#server-sdks)**.

## Client SDKs (`Element::External` + capabilities)

The Runtime ships a fixed list of primitives — View, Text, Button,
ScrollView, Pressable, TextInput, … — that every Backend has to
know. But runtime-core also ships **`Element::External`**, an
escape hatch: a tagged primitive variant + per-Backend registry
that lets a third party define their own primitive plus the
Backend impls that render it.

That's what the crates under `client/` are. None of them is part of
runtime-core. Each is a self-contained crate that an app opts
into; the framework registers the external handler at backend init
and routes draw / update / event calls through it.

| Crate | Path | What it adds |
| --- | --- | --- |
| `webview` | [`webview/`](./client/webview) | A `WebView` primitive backed by `WKWebView` on iOS, `android.webkit.WebView` on Android, and `<iframe>` on web. The canonical single-crate `cfg`-gated pattern — one crate ships every backend. |
| `maps` (+ nested `maps-core` / `maps-ios` / `maps-web`) | [`maps/`](./client/maps) — leaves under [`maps/core/`](./client/maps/core), [`maps/ios/`](./client/maps/ios), [`maps/web/`](./client/maps/web) | A `MapView` primitive. Demonstrates the multi-crate split: a shared core (`maps-core`) + per-backend leaves (`maps-ios` = `MKMapView`, `maps-web` = OSM iframe), nested under the umbrella so the SDK feature reads as one entry. Useful when backends have independent maintainers or wildly different transitive deps. |
| `video` | [`video/`](./client/video) | A `Video` primitive — `<video>` on web, `AVPlayer` on iOS, `VideoView` on Android, placeholder elsewhere. |
| `svg` | [`svg/`](./client/svg) | An `Svg` primitive — resolution-independent vector rendering: browser-native on web, `usvg`→CoreGraphics on iOS, `usvg`→`Picture` on Android. |
| `table` | [`table/`](./client/table) | `Table` / `TableRow` / `TableCell` — real HTML `<table>`/`<tr>`/`<th>`/`<td>` on web, equal-width flex views on native. |
| `form` | [`form/`](./client/form) | A `Form` container — real `<form>` (Enter-to-submit, autofill) on web, transparent passthrough on native. |
| `toolbar` | [`toolbar/`](./client/toolbar) | A `Toolbar` window-chrome primitive — `NSToolbar` on macOS, zero-size no-op elsewhere. |
| `codeblock` | [`codeblock/`](./client/codeblock) | Syntax-highlighted code rendering. Used by the docs site. |

## Utility SDKs (not `Element::External`)

Some crates here add a cross-platform *capability* rather than a
rendered primitive — they have no `*Props` and register no external
handler. They follow the same single-crate `cfg`-gated shape, but the
public surface is a plain Rust API (a trait or a handle), not a tag you
drop into `ui!`.

| Crate | Path | What it adds |
| --- | --- | --- |
| `net` | [`net/`](./client/net) | Async HTTP client over each platform's native stack. |
| `storage` | [`storage/`](./client/storage) | Plaintext key-value persistence (preferences, cache). The `AsyncStorage` half. |
| `credentials` | [`credentials/`](./client/credentials) | **Secure** key-value for secrets — Keychain / Android Keystore / Windows Credential Manager / Linux Secret Service; web errors (use a server httpOnly cookie). The `SecureStore` half. |
| `biometrics` | [`biometrics/`](./client/biometrics) | Biometric **auth gate** — Face/Touch ID (`LAContext`), Android `BiometricPrompt`, Windows Hello (`UserConsentVerifier`); web maps to WebAuthn (assertion verified server-side). The unopinionated "prove the owner is present" capability. |
| `files` | [`files/`](./client/files) | Binary blob/file storage by path — real filesystem on native (per-app dir), IndexedDB on web. For recordings, images, downloads. |
| `microphone` | [`microphone/`](./client/microphone) | Live microphone capture — a raw f32 PCM stream via cpal (desktop/iOS), `getUserMedia`+Web Audio (web), and `AudioRecord`/JNI (Android). |
| `camera` | [`camera/`](./client/camera) | Live camera capture — yields a `MediaStream` (see `media-stream`). `AVCaptureSession` (iOS/macOS), `getUserMedia`+`<canvas>` (web), `Camera2`+`ImageReader` via a Kotlin shim (Android). No preview widget. |
| `media-stream` | [`media-stream/`](./client/media-stream) | The platform-agnostic live-video-source abstraction — the common currency between capture SDKs (`camera`, `screen-recorder`) and display/compositing consumers. Thin + GPU-free: a CPU frame tap (`subscribe`/`latest`) plus an opaque zero-copy `native_source` handle. |
| `screen-recorder` | [`screen-recorder/`](./client/screen-recorder) | Screen / window frame capture as a raw frame stream. Capability API plus a private-layer `Element::External` overlay. |
| `menu` | [`menu/`](./client/menu) | OS menu-bar definitions — `NSMenu` / native app menus. A capability API (no rendered primitive); reactivity is full on macOS, one-shot elsewhere. |

## Device / platform-integration SDKs

OS-integration capabilities. `permissions` is the shared runtime-grant
substrate — any SDK that prompts the user (`notifications`, `location`, and the
media SDKs `camera` / `microphone`) delegates to it instead of re-implementing
an OS grant flow.

| Crate | Path | What it adds |
| --- | --- | --- |
| `permissions` | [`permissions/`](./client/permissions) | Cross-platform runtime permission requests — `request(Permission)` / `status(Permission)` → a uniform `PermissionStatus`. The shared grant substrate every prompting capability depends on. |
| `notifications` | [`notifications/`](./client/notifications) | Local + scheduled notifications and the raw device push token. Authorization via `permissions`; server-side push delivery is the app's job. |
| `location` | [`location/`](./client/location) | Device geolocation — one-shot `current()` and continuous `watch()` yielding a `Position`. Permission via `permissions`. |
| `clipboard` | [`clipboard/`](./client/clipboard) | System copy/paste of plain text — `set_text` / `text`. |
| `share` | [`share/`](./client/share) | The system share sheet (outbound) — hand text/url/files to another app. The inverse of `file-picker`. |
| `deep-link` | [`deep-link/`](./client/deep-link) | Inbound URL handling — `initial_link()` + `on_link()` deliver the parsed launch/resume URL (custom scheme / universal / app link). |
| `connectivity` | [`connectivity/`](./client/connectivity) | Network reachability — `current()` snapshot + `watch()` of online/offline and coarse transport. |
| `haptics` | [`haptics/`](./client/haptics) | Tactile feedback — `impact` / `notify` / `selection`. Best-effort, fire-and-forget. |
| `audio` | [`audio/`](./client/audio) | Sound playback — `load(AudioSource)` → a `Sound` you `play()`. The playback peer of the capture SDKs. |

## Navigator SDKs

Navigators are extension SDKs too — they ride `Element::Navigator` and
the per-Backend `NavigatorHandler` registry, rendering as native chrome
per platform (a `UINavigationController`-style stack, a tab bar, a
responsive drawer). An app composes one as its root and registers
screens against it.

| Crate | Path | What it adds |
| --- | --- | --- |
| `stack-navigator` | [`navigators/stack/`](./client/navigators/stack) | Push/pop stack navigation with a native header bar + typed `StackHandle` (`push`/`pop`/`replace`/`reset`). |
| `tab-navigator` | [`navigators/tab/`](./client/navigators/tab) | Flat tab switching across sibling screens; the tab bar itself is author chrome. |
| `drawer-navigator` | [`navigators/drawer/`](./client/navigators/drawer) | Responsive hamburger drawer — modal on narrow viewports, pinned-sidebar on wide (CSS `@media` collapse on web). |

The per-platform glue lives in internal helper crates under
[`navigators/helpers/`](./client/navigators/helpers) —
[`helpers/android/`](./client/navigators/helpers/android),
[`helpers/ios/`](./client/navigators/helpers/ios),
[`helpers/web/`](./client/navigators/helpers/web) — which are **not**
author-facing; the three navigator crates above consume them.

## Server SDKs

Server-tier capabilities live under [`server/`](./server). They run in the
server / worker binary and build on the `server` RPC layer
(`crates/api/server`) — reusing its `State<T>` / `#[ctx]` / `install_state`
dependency injection — the way client SDKs build on `runtime-core`. They ship no
`Element::External` primitive and never compile into the wasm client.

| Crate | Path | What it adds |
| --- | --- | --- |
| `jobs` (+ nested `jobs-macros`) | [`server/jobs/`](./server/jobs) | Background job / queue SDK. `#[job]` defines a unit of deferred work with a typed `enqueue` surface; a worker (`jobs::worker()`, dedicated process or in-process) drains a pluggable backend (`memory` / `redis` / `postgres` / `sqs`) with retries, backoff, and dead-lettering. The CLI auto-spawns the worker in `dev` and adds `idealyst worker`. See [`server/jobs/README.md`](./server/jobs/README.md) and [`crates/sdk/server/jobs/examples/jobs-demo`](./server/jobs/examples/jobs-demo). |
| `pubsub` | [`server/pubsub/`](./server/pubsub) | Publish/subscribe (fan-out) SDK — the broadcast sibling of `jobs`. A typed `Topic<T>` publishes to every current subscriber over a pluggable backend (`memory` / `redis` / `postgres` LISTEN-NOTIFY). Its headline use is **decentralized WebSocket notifications**: a `#[subscription]` body returns `topic.subscribe()`, so a client on one instance receives events published on another. See [`server/pubsub/README.md`](./server/pubsub/README.md) and [`crates/sdk/server/pubsub/examples/pubsub-demo`](./server/pubsub/examples/pubsub-demo). |
| `email` | [`server/email/`](./server/email) | Transactional email SDK. `Email::to(addr).subject(…).template(…)` sends through a pluggable `EmailProvider` (`memory` capture / AWS `ses`), rendering the body from an **idealyst template** to email-safe inline-styled HTML (via [`backend-email`](../backend/email) — theme tokens resolved to literals, no WASM). Email-safe building blocks live in [`idea-ui-mail`](../ui/idea-ui-mail). See [`server/email/README.md`](./server/email/README.md) and [`crates/sdk/server/email/examples/email-demo`](./server/email/examples/email-demo). |
| `idealyst-config` | [`server/config/`](./server/config) | Unified configuration for the server-tier SDKs. **Named connection profiles** (`[connections.<name>]`) let `jobs`/`pubsub`/`email` share one AWS/Redis/Postgres identity — or not — which flat env can't express. `configure_all()` loads + merges `idealyst.toml` (+ per-tool files, `extends` inheritance, env override) and wires every enabled SDK in one call. See [`server/config/README.md`](./server/config/README.md). |

## Testing & verification status

What's covered by automated tests, and — for the SDKs that wrap a native
facility — how far each backend has actually been *exercised* vs. only
*compiled*. This is deliberately honest: a backend that compiles for a
target but has never run on a device says so.

Each SDK's own `README.md` ends with a **`## Testing checklist`** — the
concrete, per-platform manual steps that turn a ⚠️ *compile-checked* backend
into a ✅ *verified* one. The matrix below is the summary; the per-crate
checklist is what you actually run on the device.

**Why two axes.** Much of an SDK's surface is pure logic (framing math,
parsers, builder/macro lowering) that unit tests pin down on any host. But
the part that matters most — does the platform's camera/keychain/biometric
API actually work — only resolves at runtime on real hardware, often behind
JNI/Obj-C symbol resolution that the compiler can't check. So a green
`cargo test` does **not** imply a backend is device-proven; the
"Native verification" column is where that's tracked.

**Legend**

- **Tests** — what `cargo test -p <crate>` exercises:
  - 🧪 *unit* — portable unit tests for the crate's pure logic (run anywhere).
  - 🔌 *integration* — `tests/` integration or recording/SSR snapshot tests.
  - 🖥️ *host/device* — a real-hardware test, `#[ignore]`d by default (run with `-- --ignored`).
  - — *none* — no automated tests yet.
- **Native verification** — how far the platform backends have been run:
  - ✅ *hardware-verified* — confirmed working on a real device/host.
  - 🟢 *compiles, run-exercised in examples* — built into a demo/app and seen working, though not in an automated test.
  - ⚠️ *compile-checked only* — builds for the target, but the native path is **not** yet device-verified (JNI/Obj-C symbols resolve only at runtime).

### Rendered-primitive SDKs (`Element::External`)

| Crate | Tests | Native verification |
| --- | --- | --- |
| `webview` | — none | 🟢 web/iOS/Android compile + run in docs/examples |
| `maps` · `maps-core` · `maps-ios` · `maps-web` | — none | 🟢 iOS (`MKMapView`) + web (OSM iframe) run in examples; core is pure data |
| `video` | — none | 🟢 web/iOS/Android compile + run in examples |
| `svg` | — none | 🟢 web/iOS/Android compile + run in examples |
| `table` | — none | 🟢 web (real `<table>`) + native flex |
| `form` | 🧪 unit (macro/builder lowering) | 🟢 web (`<form>`) + native passthrough |
| `toolbar` | — none | 🟢 macOS (`NSToolbar`); no-op elsewhere |
| `idea-codeblock` | — none | 🟢 runs in the docs site |

### Utility / capability SDKs

| Crate | Tests | Native verification |
| --- | --- | --- |
| `net` | 🧪 unit (cancel tokens, SSE decoder) · 🔌 integration (HTTP transport, WebSocket, EventSource, cancellation) | 🟢 native + web exercised by the integration suite |
| `storage` | — none | 🟢 runs in examples (UserDefaults / SharedPreferences / localStorage) |
| `credentials` | 🧪 unit (unsupported-fallback) · 🖥️ host (Apple Keychain, `#[ignore]`) | ✅ Apple Keychain (host test); ⚠️ **Android Keystore compile-checked only** |
| `biometrics` | 🧪 unit (builders, error Display, WebAuthn payload) | ⚠️ **Android `BiometricPrompt` + Windows Hello compile-checked only**; iOS/macOS/web run-exercised |
| `files` | 🧪 unit (path-escape safety) | 🟢 native fs + web IndexedDB run in examples |
| `microphone` | 🧪 unit (framing math, config builders) · 🖥️ host capture (`#[ignore]`) | ✅ host capture (cpal); 🟢 web/iOS/Android run in `mic-demo` |
| `camera` | 🧪 unit (config builders) · 🖥️ host capture (`#[ignore]`) | ✅ **macOS hardware-verified** (`host_capture` — AVFoundation through the `MediaStream`/`subscribe` path, shared with iOS); 🟢 web compiles/runs in `camera-demo`; ⚠️ **Android Camera2 compile-checked only** |
| `media-stream` | 🧪 unit (frame channel: subscribe/latest, RGBA/BGRA, lifecycle) | n/a — pure Rust, no native backend (the abstraction layer) |
| `screen-recorder` | 🧪 unit (portable) | ⚠️ per-platform capture paths compile-checked |
| `menu` | — none | 🟢 macOS (`NSMenu`) reactive; one-shot elsewhere |
| `i18n` · `i18n-macros` | 🧪 unit (locale, packs, format) · 🔌 macro + compile-fail UI tests | n/a — pure Rust, no native backend |

### Device / platform-integration SDKs

| Crate | Tests | Native verification |
| --- | --- | --- |
| `permissions` | 🧪 unit (status helpers, oneshot bridge) | 🟢 web (Notification / Permissions / geolocation) run-exercised; ⚠️ **Apple (`UNUserNotificationCenter` / `CLLocationManager` / `AVCaptureDevice`) + Android (`checkSelfPermission` + request seam) compile-checked only** |
| `notifications` | 🧪 unit (id resolution, builder) | 🟢 web immediate `Notification` run-exercised; ⚠️ **Apple `UNUserNotificationCenter` + Android `NotificationManager` compile-checked only**; push token + delay-scheduling are documented seams |
| `location` | 🧪 unit (Position mapping, oneshot) | 🟢 web `geolocation` run-exercised; ⚠️ **Apple `CLLocationManager` + Android `LocationManager` compile-checked only**; Android `watch` needs a host `LocationListener` shim |
| `clipboard` | 🧪 unit (error Display) | 🟢 web `navigator.clipboard` run-exercised; ⚠️ **Apple `UIPasteboard`/`NSPasteboard` + Android `ClipboardManager` compile-checked only** |
| `share` | 🧪 unit (builder, empty-guard) | 🟢 web `navigator.share` where supported; ⚠️ **Apple `UIActivityViewController`/`NSSharingServicePicker` + Android `ACTION_SEND` compile-checked only**; Android file-share needs a `FileProvider` seam |
| `deep-link` | 🧪 unit (URL parse, dedupe, dispatch, RAII unsubscribe) | 🟢 web `location.href` run-exercised; ⚠️ **Apple/Android launch-URL forwarding is a host seam (the parse/dispatch core is pure Rust)** |
| `connectivity` | 🧪 unit (snapshot consistency, transport) | 🟢 web `navigator.onLine` run-exercised; ⚠️ **Apple `NWPathMonitor` + Android `ConnectivityManager` compile-checked only**; Android `watch` needs a host `NetworkCallback` shim |
| `haptics` | 🧪 unit (style mapping) | 🟢 web `navigator.vibrate` where supported; ⚠️ **Apple `UIFeedbackGenerator`/`NSHapticFeedbackManager` + Android `Vibrator` compile-checked only** |
| `audio` | 🧪 unit (source/handle, async load) | 🟢 web `HTMLAudioElement` run-exercised; ⚠️ **Apple `AVAudioPlayer` + Android `MediaPlayer` compile-checked only**; desktop = `NotSupported` fallback |

### Navigator SDKs

| Crate | Tests | Native verification |
| --- | --- | --- |
| `stack-navigator` | 🧪 unit · 🔌 recording + SSR snapshot | 🟢 iOS/macOS/Android/web run in `stack-demo-v2` + the docs site |
| `tab-navigator` | 🧪 unit · 🔌 recording + SSR snapshot | 🟢 run in examples |
| `drawer-navigator` | 🧪 unit · 🔌 recording + SSR snapshot | 🟢 run across the website + examples |
| `navigators/helpers/{android,ios,web}` | — none (internal) | exercised transitively via the three navigators |

> **The compile-checked-only backends** (`camera` Android, `credentials`
> Android, `biometrics` Android + Windows) all follow the same JNI/WinRT
> pattern, where the native callback symbols resolve at runtime. Each
> surfaces every failure as a typed error carrying the underlying platform
> message, so device bring-up is a matter of reading those errors — not
> silent breakage. Clearing them to ✅ needs a device run, not more host
> tests.

## Declaring platform permissions (capabilities)

An SDK that needs a device permission declares a **capability** in its own
`Cargo.toml` — it does not hand-edit app manifests:

```toml
[package.metadata.idealyst]
capabilities = ["microphone"]
```

At build time the CLI walks the app's dependency graph, collects every
declared capability, and expands each into the platform artifacts it needs
(iOS/macOS `Info.plist` usage-description keys + entitlements, Android
`<uses-permission>`). The **library declares the requirement; the app
declares the reason** the OS prompt shows:

```toml
[package.metadata.idealyst.app.permissions]
microphone = "Record voice notes"
```

A missing reason falls back to a generic default with a build warning. The
known capabilities and their per-platform mapping live in one registry —
`crates/tools/build/ios/src/capabilities.rs`; add a row there to support a
new one.

## The two SDK shapes

Both shapes are valid; pick by ownership model.

**Single crate with `cfg` gates** (the `webview` pattern). One crate
declares the primitive + per-target `[target.'cfg(...)'.dependencies]`
and ships every Backend impl from the same release. Simpler when
one team owns the SDK and ships all backends in lockstep.

**Umbrella + per-platform leaves** (the `maps` pattern). A core
crate defines the primitive; per-backend crates implement the
per-Backend handler. Justified when backends have independent
maintainers or genuinely heavy disjoint transitive deps.

## Writing your own

`cargo new` a crate that defines a `*Props` struct, registers an
external handler per Backend you support, and exposes a builder
function. The Runtime side is pure data — the substrate-specific
work lives in the per-Backend impls. See
[the third-party primitives doc page](../../websites/docs/src/pages/third_party_primitives.rs)
for the full pattern.

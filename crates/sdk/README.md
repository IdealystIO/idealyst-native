# `sdk/` — third-party extension primitives

The Runtime ships a fixed list of primitives — View, Text, Button,
ScrollView, Pressable, TextInput, … — that every Backend has to
know. But runtime-core also ships **`Element::External`**, an
escape hatch: a tagged primitive variant + per-Backend registry
that lets a third party define their own primitive plus the
Backend impls that render it.

That's what the crates here are. None of them is part of
runtime-core. Each is a self-contained crate that an app opts
into; the framework registers the external handler at backend init
and routes draw / update / event calls through it.

| Crate | Path | What it adds |
| --- | --- | --- |
| `webview` | [`webview/`](./webview) | A `WebView` primitive backed by `WKWebView` on iOS, `android.webkit.WebView` on Android, and `<iframe>` on web. The canonical single-crate `cfg`-gated pattern — one crate ships every backend. |
| `maps` (+ nested `maps-core` / `maps-ios` / `maps-web`) | [`maps/`](./maps) — leaves under [`maps/core/`](./maps/core), [`maps/ios/`](./maps/ios), [`maps/web/`](./maps/web) | A `MapView` primitive. Demonstrates the multi-crate split: a shared core (`maps-core`) + per-backend leaves (`maps-ios` = `MKMapView`, `maps-web` = OSM iframe), nested under the umbrella so the SDK feature reads as one entry. Useful when backends have independent maintainers or wildly different transitive deps. |
| `video` | [`video/`](./video) | A `Video` primitive — `<video>` on web, `AVPlayer` on iOS, `VideoView` on Android, placeholder elsewhere. |
| `svg` | [`svg/`](./svg) | An `Svg` primitive — resolution-independent vector rendering: browser-native on web, `usvg`→CoreGraphics on iOS, `usvg`→`Picture` on Android. |
| `table` | [`table/`](./table) | `Table` / `TableRow` / `TableCell` — real HTML `<table>`/`<tr>`/`<th>`/`<td>` on web, equal-width flex views on native. |
| `form` | [`form/`](./form) | A `Form` container — real `<form>` (Enter-to-submit, autofill) on web, transparent passthrough on native. |
| `toolbar` | [`toolbar/`](./toolbar) | A `Toolbar` window-chrome primitive — `NSToolbar` on macOS, zero-size no-op elsewhere. |
| `codeblock` | [`codeblock/`](./codeblock) | Syntax-highlighted code rendering. Used by the docs site. |

## Utility SDKs (not `Element::External`)

Some crates here add a cross-platform *capability* rather than a
rendered primitive — they have no `*Props` and register no external
handler. They follow the same single-crate `cfg`-gated shape, but the
public surface is a plain Rust API (a trait or a handle), not a tag you
drop into `ui!`.

| Crate | Path | What it adds |
| --- | --- | --- |
| `net` | [`net/`](./net) | Async HTTP client over each platform's native stack. |
| `storage` | [`storage/`](./storage) | Plaintext key-value persistence (preferences, cache). The `AsyncStorage` half. |
| `credentials` | [`credentials/`](./credentials) | **Secure** key-value for secrets — Keychain / Android Keystore / Windows Credential Manager / Linux Secret Service; web errors (use a server httpOnly cookie). The `SecureStore` half. |
| `biometrics` | [`biometrics/`](./biometrics) | Biometric **auth gate** — Face/Touch ID (`LAContext`), Android `BiometricPrompt`, Windows Hello (`UserConsentVerifier`); web maps to WebAuthn (assertion verified server-side). The unopinionated "prove the owner is present" capability. |
| `files` | [`files/`](./files) | Binary blob/file storage by path — real filesystem on native (per-app dir), IndexedDB on web. For recordings, images, downloads. |
| `microphone` | [`microphone/`](./microphone) | Live microphone capture — a raw f32 PCM stream via cpal (desktop/iOS), `getUserMedia`+Web Audio (web), and `AudioRecord`/JNI (Android). |
| `camera` | [`camera/`](./camera) | Live camera capture — a raw RGBA8 frame stream (the video sibling of `microphone`). `AVCaptureSession` (iOS/macOS), `getUserMedia`+`<canvas>` (web), `Camera2`+`ImageReader` via a Kotlin shim (Android). No preview widget — hands you pixels. |
| `screen-recorder` | [`screen-recorder/`](./screen-recorder) | Screen / window frame capture as a raw frame stream. Capability API plus a private-layer `Element::External` overlay. |
| `menu` | [`menu/`](./menu) | OS menu-bar definitions — `NSMenu` / native app menus. A capability API (no rendered primitive); reactivity is full on macOS, one-shot elsewhere. |

## Navigator SDKs

Navigators are extension SDKs too — they ride `Element::Navigator` and
the per-Backend `NavigatorHandler` registry, rendering as native chrome
per platform (a `UINavigationController`-style stack, a tab bar, a
responsive drawer). An app composes one as its root and registers
screens against it.

| Crate | Path | What it adds |
| --- | --- | --- |
| `stack-navigator` | [`navigators/stack/`](./navigators/stack) | Push/pop stack navigation with a native header bar + typed `StackHandle` (`push`/`pop`/`replace`/`reset`). |
| `tab-navigator` | [`navigators/tab/`](./navigators/tab) | Flat tab switching across sibling screens; the tab bar itself is author chrome. |
| `drawer-navigator` | [`navigators/drawer/`](./navigators/drawer) | Responsive hamburger drawer — modal on narrow viewports, pinned-sidebar on wide (CSS `@media` collapse on web). |

The per-platform glue lives in internal helper crates under
[`navigators/helpers/`](./navigators/helpers) —
[`helpers/android/`](./navigators/helpers/android),
[`helpers/ios/`](./navigators/helpers/ios),
[`helpers/web/`](./navigators/helpers/web) — which are **not**
author-facing; the three navigator crates above consume them.

## Testing & verification status

What's covered by automated tests, and — for the SDKs that wrap a native
facility — how far each backend has actually been *exercised* vs. only
*compiled*. This is deliberately honest: a backend that compiles for a
target but has never run on a device says so.

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
| `camera` | 🧪 unit (frame math, config builders) · 🖥️ host capture (`#[ignore]`) | ✅ **macOS hardware-verified** (`host_capture` — AVFoundation, shared with iOS); 🟢 web compiles/runs in `camera-demo`; ⚠️ **Android Camera2 compile-checked only** |
| `screen-recorder` | 🧪 unit (portable) | ⚠️ per-platform capture paths compile-checked |
| `menu` | — none | 🟢 macOS (`NSMenu`) reactive; one-shot elsewhere |
| `i18n` · `i18n-macros` | 🧪 unit (locale, packs, format) · 🔌 macro + compile-fail UI tests | n/a — pure Rust, no native backend |

### Navigator SDKs

| Crate | Tests | Native verification |
| --- | --- | --- |
| `stack-navigator` | 🧪 unit · 🔌 recording + SSR snapshot | 🟢 iOS/macOS/Android/web run in `stack-demo` + the docs site |
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
[the third-party primitives doc page](../../examples/docs/src/pages/third_party_primitives.rs)
for the full pattern.

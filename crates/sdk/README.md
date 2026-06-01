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
| `maps` / `maps-core` / `maps-web` | [`maps/`](./maps), [`maps-core/`](./maps-core), [`maps-web/`](./maps-web) | A `MapView` primitive. Demonstrates the multi-crate split: a shared core + per-backend leaves. Useful when backends have independent maintainers or wildly different transitive deps. |
| `idea-codeblock` | [`idea-codeblock/`](./idea-codeblock) | Syntax-highlighted code rendering. Used by the docs site. |

## Utility SDKs (not `Element::External`)

Some crates here add a cross-platform *capability* rather than a
rendered primitive — they have no `*Props` and register no external
handler. They follow the same single-crate `cfg`-gated shape, but the
public surface is a plain Rust API (a trait or a handle), not a tag you
drop into `ui!`.

| Crate | Path | What it adds |
| --- | --- | --- |
| `net` | [`net/`](./net) | Async HTTP client over each platform's native stack. |
| `storage` | [`storage/`](./storage) | Async key-value persistence. |
| `microphone` | [`microphone/`](./microphone) | Live microphone capture — a raw f32 PCM stream via cpal (desktop/iOS), `getUserMedia`+Web Audio (web), and `AudioRecord`/JNI (Android). |

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

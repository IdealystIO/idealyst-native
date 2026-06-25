# `webview`

A `WebView` primitive for the idealyst framework — embed a live web
document inside your native UI tree. Built on the framework's
`Element::External` extension mechanism, so it's not part of
runtime-core: an app opts in by depending on this crate and calling
`webview::register(&mut backend)` once at bootstrap.

This is the **canonical single-crate, `cfg`-gated** third-party
primitive: one crate ships every backend, selected at compile time via
`[target.'cfg(...)'.dependencies]`. (Contrast the `maps` SDK, which
splits a shared core from per-backend leaf crates.)

```rust,ignore
use webview::prelude::*;
use runtime_core::{signal, Ref};

// App bootstrap — one line per third-party SDK:
let mut backend = WebBackend::new("#app");
webview::register(&mut backend);

// Inside a `ui!` block. `WebView` is an external primitive, so it's
// interpolated as an expression (the macro only recognizes the
// framework's closed first-party set):
let url = signal("https://example.com".to_string());
let wv: Ref<WebViewHandle> = Ref::new();
ui! {
    View {
        { webview::WebView(WebViewProps {
            url: webview::url(move || url.get()),
            on_load: Some(Rc::new(|| log::info!("loaded"))),
            ..Default::default()
        }).bind(wv.clone()) }
    }
}

// Imperative ops at any later point, via the bound handle:
wv.with(|h| h.reload());
wv.with(|h| h.post_message("{\"type\":\"ping\"}"));
```

## What you get

Every backend embeds a real web document and converges on the same
author-observable behavior — a reactive `url`, load/error/message
callbacks, and imperative `reload` / `post_message` / `execute_js` ops.
The *mechanism* differs per platform:

| Target | Mechanism |
| --- | --- |
| Web (wasm32) | `<iframe>`; callbacks are DOM `load` / `error` / `message` listeners |
| iOS | `WKWebView` via raw `msg_send`; callbacks via a `WKNavigationDelegate` + `WKScriptMessageHandler` shim |
| Android | `android.webkit.WebView`; `loadUrl` navigation |
| Other (wgpu desktop, terminal, …) | the framework's `External` "not supported" placeholder |

### Backend caveats

- **Android callbacks** (`on_load` / `on_error` / `on_message`) are a
  no-op in v1 — they need a `WebViewClient` subclass and a
  `@JavascriptInterface` bridge, which would ship as Kotlin shims from
  this crate via `[package.metadata.idealyst.android].runtime_kotlin`.
  Navigation itself works.
- **`execute_js`** returns `Err` on backends that can't do synchronous
  JS eval, and on cross-origin content.
- **`on_error`** on web only covers the iframe's own `error` event;
  network failures *inside* the embedded document don't bubble up.

## Reactive vs. imperative

`url` is reactive: pass a closure reading a `Signal`/`Source` and the
per-backend handler re-navigates on change (it subscribes via
`Effect::new(...)` when it builds the native view — there's no
framework-level update plumbing). Use [`url`] to coerce a `&str`,
`String`, or `Fn() -> String` into the stored closure shape without
thinking about boxing.

Everything else is imperative through the bound `Ref<WebViewHandle>`:
`reload()`, `post_message(&str)`, `execute_js(&str)`. Bind a handle
with `.bind(my_ref.clone())` on the value `WebView(..)` returns.

## Why one crate (not the `maps` split)

This SDK has a single owner and ships every backend from the same
release. Cargo's per-target dependency tables handle the platform deps
cleanly, so the multi-crate split buys nothing here. Prefer this shape
for new SDKs unless backends genuinely have independent maintainers or
heavy disjoint transitive deps.

## iOS WebKit note

We reach `WKWebView` via `class!()` + `msg_send` rather than
`objc2-web-kit`: that crate's v0.2.2 mis-gates the `WKWebView`
re-export to a macOS-only feature, and upgrading to v0.3+ would pull in
objc2 0.6 and conflict with `backend-ios-mobile`'s 0.5. The raw runtime
calls are equivalent at the Obj-C layer. See `Cargo.toml` for the full
rationale.

[`url`]: src/lib.rs

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo build -p webview --target wasm32-unknown-unknown` — web target

**Rendering / behavior**
- [ ] **Web** — `WebView` loads the `url` in an `<iframe>` and renders the page;
  changing the reactive `url` signal re-navigates; `on_load`/`on_error`/`on_message`
  fire as DOM `load`/`error`/`message` listeners; `reload()` and `post_message(...)`
  via the bound handle work; `execute_js(...)` returns `Ok` on same-origin content
  and `Err` cross-origin.
- [ ] **iOS** — ⚠️ not yet device-confirmed. `WKWebView` loads the URL and renders
  the page; reactive `url` re-navigates; `on_load`/`on_message` fire via the
  `WKNavigationDelegate` + `WKScriptMessageHandler` shim; `reload()`/`post_message`
  drive the native view.
- [ ] **Android** — ⚠️ not yet device-confirmed. `android.webkit.WebView` loads the
  URL via `loadUrl` and renders; reactive `url` re-navigates. Confirm the v1 caveat:
  `on_load`/`on_error`/`on_message` callbacks are a **no-op** (verify navigation
  still works and nothing crashes).
- [ ] **macOS / other** — no handler registered; verify the framework's `External`
  "not supported" placeholder renders cleanly (no layout artifact or crash).

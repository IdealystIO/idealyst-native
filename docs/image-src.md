# `image()` sources

One `src` behaves the same on every backend. This page is the contract;
`runtime_shared::image_src` is its parser, and every backend routes through
it.

## What a `src` can be

| `src` | Loads |
|---|---|
| `https://…`, `http://…` | Fetched off the UI thread. `on_load` on success; `on_error` on a network error, a non-2xx status, or bytes that don't decode. Plain `http` still needs the platform's cleartext exception (iOS ATS, Android network-security config). |
| `data:…` | Decoded in place. Base64 (`data:image/png;base64,…`) or percent-encoded (`data:image/svg+xml,%3Csvg…`), scheme in any case. |
| `image_asset(asset)` | The asset's bytes. `embed_asset!` (bytes compiled in) and `asset!` (a file the build ships, see below) both work everywhere. idea-ui's `Image` and `Avatar` take an asset as their `src`. |
| `/path` (root-relative) | Resolved against the image base, see below. |

### Root-relative paths

A `src` starting with a single `/` (`/uploads/x.png`) is joined to the image
base before any backend sees it: `https://api.example.com` + `/uploads/x.png`
→ `https://api.example.com/uploads/x.png`, on every platform.
`server::configure(ClientConfig::new(url))` sets the base; an app without a
server sets it with `runtime_core::set_image_base_url(url)`. Only an absolute
`http(s)://` base counts.

With no base set (`server::configure` with `""`, the usual same-origin web
setup, or nothing configured):

- web, SSR and the wgpu renderer in the browser hand the path to the browser,
  which resolves it against the page origin;
- every other backend rejects it, with a warning naming the src.

A file that ships with the app belongs in `asset!`, not at a `/path`: an asset
loads on every platform with no base.

### Rejected

Anything else is **rejected**: `on_error` fires and a warning is logged
(`image(): rejected src "logo.png": …`), once per distinct src, in release
builds too. That covers:

- document-relative paths — `logo.png`, `./a.png`, `../a.png`. On web they
  would resolve against the current route, so they mean something different
  on every screen. Use a root-relative `/path`, an absolute URL, or an asset.
- a root-relative path with no base, outside a browser (see above).
- scheme-relative `//host/x.png` — spell out `https://`.
- `file:`, `blob:`, `content:`, `ftp:` and every other scheme — each is
  readable on some platforms only.
- an empty `src`.

Image formats: PNG, JPEG, GIF (first frame), WebP and SVG, on every backend.
SVG is rasterized at the size it is displayed, at the screen's pixel density
(re-rasterized when either changes); SVG `<text>` is not drawn on
native (convert text to outlines). In the browser — the DOM web backend and
the wgpu renderer running on the web — the browser decodes every format
(the wgpu renderer draws the decoded image into a canvas and uploads the
pixels), so no Rust decoder ships in the web bundle and SVG `<text>` is
drawn there. Because browser decoding is asynchronous, a `data:` or embedded
`src` settles a moment after mount on the web, as it does for `<img>`.

## Events

- `on_load` fires once per successful load with the natural size; `on_error`
  once per failure. A handler installed after the outcome still receives it.
- Changing `src` supersedes the previous load: a slow earlier response never
  replaces a newer `src`.
- Headless renderers (SSR, email, terminal, CPU) fire no events.

## Bundled assets (`asset!`)

`asset!("images/logo.png")` names `assets/images/logo.png` in the crate that
declares it. The build copies that directory into the app, and each platform
reads it from the same relative place:

| Platform | Location |
|---|---|
| Web | `/assets/<path>` (root-absolute, so nested routes resolve it) |
| iOS, macOS | `assets/<path>` in the app bundle's resources (`NSBundle.mainBundle`) |
| Android | `assets/<path>` in the APK (`AssetManager`) |
| Linux, Windows, wgpu (native) | `<exe dir>/assets/<path>`, then `<cwd>/assets/<path>` (`cargo run` from the project root) |
| wgpu in the browser | `/assets/<path>`, like web |
| SSR | `/assets/<path>` |
| Email | inlined as a `data:` URI, read from the same desktop locations (some clients, Gmail among them, strip `data:` images — an image every reader must see belongs at an `https://` URL) |

An asset path may not be absolute or contain `..`.

What the CLI copies, and where (`build_ios::app_assets` does the copying):

- Only the **app** crate's `assets/` ships. Files in a library crate's
  `assets/` are not copied: paths have no crate prefix, so two crates'
  `images/logo.png` would collide. A library that needs a file at runtime
  uses `embed_asset!`.
- Web: `idealyst build --web` and the full-stack `dev` bundle copy `assets/`
  into the bundle root, even when `[package.metadata.idealyst.app.web].assets`
  is an allowlist that doesn't name it. Plain `idealyst dev --web` serves the
  project root, so `/assets/<path>` already resolves.
- iOS: the simulator `.app` gets `<App>.app/assets/`. Device and App Store
  builds reference the staged folder in the generated Xcode project, and
  xcodebuild copies it to the same place.
- macOS: `<App>.app/Contents/Resources/assets/`. When the app has an
  `assets/` folder, `idealyst run`/`dev --macos` always wraps the binary in
  a `.app`. The bare binary from `idealyst build --macos` gets
  `<exe dir>/assets/`. A bare executable's `NSBundle.mainBundle` resource
  path is its own directory.
- Android: APK entries `assets/<path>`, which the app opens as
  `AssetManager.open("<path>")`. Media formats that are already compressed
  (png, jpg, webp, mp4, …) are stored uncompressed, so `openFd` works on them.
- Linux, Windows, wgpu: `<exe dir>/assets/` next to the built binary.

On native platforms a copy only rewrites files whose size or modification
time changed, and it removes files deleted from `assets/`. The web bundle is
rebuilt from scratch on each build. Symlinks inside `assets/` are skipped,
so a link can't pull a file from outside the folder into an app.

`embed_asset!` needs none of this — the bytes are in the binary — so it is the
right choice for small images, and `asset!` for large ones.

## Not a `src`

Images a server serves behind authentication: `image()` sends no headers on
any platform (a browser `<img>` can't either). Serve such images through
signed URLs, or fetch the bytes yourself and pass a `data:` URI.

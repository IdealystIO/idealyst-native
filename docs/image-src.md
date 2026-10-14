# `image()` sources

One `src` behaves the same on every backend. This page is the contract;
`runtime_shared::image_src` is its parser, and every backend routes through
it.

## What a `src` can be

| `src` | Loads |
|---|---|
| `https://…`, `http://…` | Fetched off the UI thread. `on_load` on success; `on_error` on a network error, a non-2xx status, or bytes that don't decode. Plain `http` still needs the platform's cleartext exception (iOS ATS, Android network-security config). |
| `data:…` | Decoded in place. Base64 (`data:image/png;base64,…`) or percent-encoded (`data:image/svg+xml,%3Csvg…`), scheme in any case. |
| `image_asset(asset)` | The asset's bytes. `embed_asset!` (bytes compiled in) and `asset!` (a file the build ships, see below) both work everywhere. |

Anything else is **rejected**: `on_error` fires, and debug builds log why
(`image(): rejected src "/x.png": …`). That covers:

- relative paths — `/attachments/abc`, `logo.png`, `./a.png`. Native has no
  page to resolve against, so web rejects them too. Build an absolute URL
  (`format!("{}{path}", server::base_url().unwrap())`) or ship the file as an
  asset.
- scheme-relative `//host/x.png` — spell out `https://`.
- `file:`, `blob:`, `content:`, `ftp:` and every other scheme — each is
  readable on some platforms only.
- an empty `src`.

Image formats: PNG, JPEG, GIF (first frame), WebP and SVG, on every backend.
SVG is rasterized at the size it is displayed; SVG `<text>` is not drawn on
native (convert text to outlines).

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

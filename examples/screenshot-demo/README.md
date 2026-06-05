# screenshot-demo

A demo for the **native screen-capture debug utility**. It renders one
distinctive, live screen (title, three coloured badges, a counter with
`+`/`-` buttons). Capture it via the Robot bridge's `screenshot` verb and
you get a PNG of the **real rendered native surface** — including the
current counter value, which proves the capture reflects live state
rather than a static re-render.

## What this exercises

`Backend::capture_screenshot` is implemented natively per backend:

| Backend | Mechanism |
| --- | --- |
| macOS (AppKit) | `-[NSView cacheDisplayInRect:toBitmapImageRep:]` → `NSBitmapImageRep` PNG |
| iOS (UIKit) | `-[UIView drawViewHierarchyInRect:afterScreenUpdates:]` → `UIImagePNGRepresentation` |
| Android | `View.draw(Canvas)` → `Bitmap.compress(PNG)` |
| Web (DOM) | not yet supported — returns an "unsupported" error |

`mount()` registers the live `screenshot` bridge verb automatically when
the backend reports `supports_screenshot()`. `idealyst dev` enables the
`dev` feature, which starts the Robot bridge — so no app code is needed.

### Capture sources

The `screenshot` verb takes an optional `source` arg:

| `source` | What you get |
| --- | --- |
| `client` | The **real native surface** of the running client. Works in `--local` mode (captured in-process) and in runtime-server mode (the server asks the connected client to capture and ships the PNG back over the wire). |
| `replay` | A **wgpu re-render** of the recorded scene model, rasterized server-side. Always available — even with no client attached — but uses the framework's renderer, not the platform's (different fonts/metrics). |
| `auto` *(default)* | Try the real client; fall back to `replay` on error/timeout. |

So in runtime-server mode, `source: client` (or the `auto` default) returns
the genuine on-device pixels; `source: replay` returns the GPU re-render.

## Run it

```sh
# from the repo root or this directory
idealyst dev --macos --local      # or --ios / --android
```

The bridge logs its port on startup, e.g.:

```
[robot-bridge] listening on port 9777
[robot-bridge] registered live app at ~/.idealyst/apps/screenshot-demo-<pid>.json
```

## Capture

### Helper script (auto-discovers the port)

```sh
python3 examples/screenshot-demo/capture.py                 # auto (real client, falls back to replay)
python3 examples/screenshot-demo/capture.py --source client  # force the real native surface
python3 examples/screenshot-demo/capture.py --source replay   # force the wgpu re-render
# saved screenshot.png  reported=2048x1536  ihdr=2048x1536  bytes=109698
```

Pin a port if you set one: `IDEALYST_BRIDGE_PORT=9777 idealyst dev --macos`
then `python3 capture.py --port 9777`. (Drop `--local` to exercise the
runtime-server wire path; `--source client` then captures the real client
over the wire.)

Try the live-state proof: press `+` a few times in the app window, run
`capture.py` again, and the counter value in the PNG changes.

### Raw protocol

The bridge speaks newline-delimited JSON over TCP:

```sh
printf '{"id":1,"cmd":"screenshot","args":{}}\n' | nc 127.0.0.1 9777
```

Response (`ok` payload):

```json
{ "png_base64": "iVBORw0KGgo…", "width": 1600, "height": 1000 }
```

### Via MCP

If you drive the app through the idealyst MCP server, call the
`screenshot` tool (optionally with an `app` filter). It returns the same
`{ png_base64, width, height }` payload.

## Notes

- **Web** currently returns an unsupported error; DOM rasterization needs
  an async path and isn't wired yet.
- Content drawn on a *separate* surface (a `Graphics` primitive's GPU
  layer, video) may not appear — the capture is of the native view
  hierarchy. This limitation is shared across all three native backends
  and documented in each backend's `screenshot.rs`.

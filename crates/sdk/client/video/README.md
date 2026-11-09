# `video`

A `Video` primitive for the idealyst framework — display a media URL **or
a live `MediaStream`** (from `camera` / `screen-recorder`) inside your
native UI tree with the platform's own player. Built on the
framework's third-party scene-primitive mechanism, so it's not part
of runtime-core: an app opts in by depending on this crate and calling
`video::register` to the boot entry once at bootstrap.

Single-crate, `cfg`-gated — one crate ships every backend, selected at
compile time. Mirrors the `webview` SDK's layout (see that crate for
the same rationale on why one crate beats the `maps`-style split).

```rust,ignore
use video::prelude::*;
use runtime_core::{signal, Ref};

// App bootstrap — one line per third-party SDK. `register` IS the boot
// registration seam (it takes the scene `Registry`, not the backend):
// backend_web::newcore::start_in("#app", video::register, app);

// Inside a `ui!` block. `Video` is an external primitive, so it's
// interpolated as an expression:
let src = signal("https://example.com/clip.mp4".to_string());
let v: Ref<VideoHandle> = Ref::new();
ui! {
    view {
        { video::Video(VideoProps {
            // One extensible `source` prop — a VideoSource. Build it with
            // `url(...)` (string / Fn()->String) or `stream(...)` (a live
            // MediaStream / Fn()->Option<MediaStream>). Reactive either way.
            source: video::url(move || src.get()),
            autoplay: true,
            controls: true,
            ..Default::default()
        }).bind(v.clone()) }
    }
}

// A live camera feed instead of a URL — same prop:
//   source: video::stream(camera.open(cfg).await?),
//   source: video::stream(move || stream_sig.get()),   // reactive swap

// Imperative ops at any later point, via the bound handle:
v.with(|h| h.play());
v.with(|h| h.seek(10.0));
```

## What you get

Every backend displays the resolved `source` through the platform's native
player and converges on the same author-observable behavior — a reactive
`source`, `autoplay` / `controls` / `loop_playback` flags, and imperative
`play` / `pause` / `seek` ops. A `source` is a `VideoSource` that resolves
(inside a reactive `Effect`, so it re-populates on signal change) to a
small, platform-agnostic `MediaContent` set: a `Url` (the native player
fetches it) or a live `Stream`. The *mechanism* differs per platform:

| Target | URL mechanism | Live `Stream` mechanism |
| --- | --- | --- |
| Web (wasm32) | `<video>` element (`src`), built and driven through web-glue | `<video>.srcObject` ← the stream's native `MediaStream` (a `web_glue::dom::MediaStream`; zero-copy) |
| iOS | `AVPlayer` + `AVPlayerLayer` in a `UIView` | `AVSampleBufferDisplayLayer` ← each frame's `CMSampleBuffer`, or RGBA frames as a `CGImage` |
| Android | `android.widget.VideoView` (`setVideoURI`) | a camera: a `TextureView` the capture fills directly, no CPU copy. Any other stream, or a device that refuses the preview: RGBA frames into an `ImageView` |
| macOS | `AVPlayer` + `AVPlayerLayer` (an AVKit `AVPlayerView` when `controls`) | per-frame `CGImage` / `IOSurface` layer contents |
| Linux | `gtk::Video` over GTK's GStreamer media backend | RGBA frames into a `gdk::MemoryTexture` paintable |
| Windows | Media Foundation's Media Engine (frame server) → a composition swapchain on a graphics node, sized and fit per `object_fit`; software decode where the adapter has no video decode. `controls` paints a play/pause + seek + time bar with Direct2D. Plays what Windows has codecs for (MP4/H.264/HEVC, WebM/VP9, …); no HLS/DASH | RGBA frames drawn into the same swapchain with Direct2D |
| Other (SSR, wgpu desktop, terminal, …) | the frozen External "not supported" placeholder | — |

### Backend caveats

- **Android live preview is one display at a time.** Two `Video`s showing one camera
  stream: the first gets the `TextureView`, the second shows CPU frames (which turns
  the camera's per-frame conversion on) and takes over the moment the first lets go.
  A later surface rebuilds the capture session, a pause of a few frames.
- **Android `controls` and `loop_playback`** are not yet wired — a
  `MediaController` and an `OnCompletionListener` need Kotlin/Java shim
  classes. This matches the framework's prior built-in Android video
  behavior, where the same params were stubs. `play` / `pause` / `seek`
  and reactive `src` all work.
- **`autoplay`** generally requires a muted start on every platform to
  fire without a user gesture; the per-backend impls pair
  `autoplay = true` with a silent start automatically.

## Reactive vs. imperative

`src` is reactive: pass a closure reading a `Signal`/`Source` and the
per-backend handler swaps the playing clip on change (it subscribes via an
`effect!` when it builds the native view). Use [`src`] to
coerce a `&str`, `String`, or `Fn() -> String` into the stored closure
shape.

`autoplay`, `controls`, and `loop_playback` are static at construction
— a re-render with different values tears down and re-mounts the view,
which is the desired behavior for those flags anyway.

Imperative ops go through the bound `Ref<VideoHandle>`: `play()`,
`pause()`, `seek(seconds)`. Bind a handle with `.bind(my_ref.clone())`
on the value `Video(..)` returns.

## Knowing when there is a picture

`on_first_frame: Some(Rc::new(|| …))` is called once the first frame of the
current source is on screen, and again after each source change (a new URL or
a different stream) once that source's first frame is up. Keep a placeholder
or spinner over the view until it fires:

```rust
let ready = signal(false);
Video(VideoProps {
    source: video::stream(move || camera.get()),
    autoplay: true,
    on_first_frame: Some(Rc::new(move || ready.set(true))),
    ..Default::default()
})
```

It runs from the scheduler right after the frame is shown, never inside a
platform callback, so it may write signals; it is skipped once the `Video`
unmounts. Each backend reports what it already knows:

| Target | First frame of a URL | First frame of a live stream |
| --- | --- | --- |
| Web | `<video>` `loadeddata` | `<video>` `loadeddata` |
| iOS | `AVPlayerLayer.readyForDisplay` | first `CMSampleBuffer` enqueued on the `AVSampleBufferDisplayLayer`, or first `CGImage` set |
| macOS | `readyForDisplay` on the `AVPlayerLayer` / `AVPlayerView` | first `IOSurface` or `CGImage` set as layer contents |
| Android | `VideoView` `MEDIA_INFO_VIDEO_RENDERING_START` | camera preview: the `TextureView`'s first update; otherwise the first bitmap shown |
| Linux | the `GtkMediaStream`'s first `invalidate-contents` | first texture on the live paintable |
| Windows | first presented frame | first presented frame |

The placeholder host (SSR, terminal, …) shows no video and never calls it.

On iOS the stream's `AVSampleBufferDisplayLayer` stays hidden until its first
sample is enqueued: an empty one is opaque black and would cover the views
behind the `Video` (and its own CPU-frame fallback) until then.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet. Tick each
item as you exercise it. This is a display primitive, so
verification is mostly interactive (register the renderer at bootstrap, then
exercise a `Video` in a running app).

**Automated**
- [ ] `cargo test -p video` — portable logic (`source` builders, prop defaults), and the Android preview's transform (`src/android_preview.rs`: every sensor mounting × facing × display rotation × view shape × fit, held to the same picture as a CPU frame)
- [ ] `cargo build -p video --target wasm32-unknown-unknown` — web target
- [x] `cargo test -p video --target wasm32-unknown-unknown` (headless Chrome through the workspace runner) — `tests/web_video.rs`: construction props (autoplay forces the muted property, controls, object-fit), the handle ops through the host node, a stream's native `web_glue::dom::MediaStream` as `srcObject` (any other native-source type is ignored), and the reactive source switching stream → URL → none

**Behavior**
- [ ] **Web** — a `url(..)` source plays in a `<video>`; a `stream(..)` source binds the live `MediaStream` via `.srcObject` (zero-copy); reactive `source` swaps the clip; `autoplay`/`controls`/`loop_playback` and imperative `play`/`pause`/`seek` work.
- [ ] **iOS** — ⚠️ not yet device-confirmed: a `url(..)` source plays via `AVPlayer`/`AVPlayerLayer`; reactive `src` swap, `controls`, `loop_playback`, and imperative `play`/`pause`/`seek` work; live `stream` source is the GPU/compositing phase (unwired).
- [ ] **Android** — ⚠️ not yet device-confirmed: a `url(..)` source plays via `VideoView`; reactive `src`, `play`/`pause`/`seek` work. Note `controls` and `loop_playback` are **not yet wired** (need a `MediaController`/`OnCompletionListener` shim). A camera `stream` shows through a `TextureView` (upright and unmirrored in all four display rotations, cover and contain, clipped by a rounded parent, no CPU frames made); any other stream through an `ImageView`.
- [x] **Windows** — `tests/windows_video.rs` on a Windows 11 VM (the Media Engine opens the URL: duration reaches the handle, `seek` moves `position`), and by hand in `examples/sdk-gallery`: plays, letterboxes, painted controls play/pause/seek, `play`/`pause`/`seek`/`position` ops.
- [ ] **Other (SSR / wgpu desktop / terminal)** — renders the frozen External "not supported" placeholder.

No OS permission of its own — playback uses the platform's native player.

[`src`]: src/lib.rs

# `video`

A `Video` primitive for the idealyst framework — play a media URL
inside your native UI tree with the platform's own player. Built on the
framework's `Element::External` extension mechanism, so it's not part
of runtime-core: an app opts in by depending on this crate and calling
`video::register(&mut backend)` once at bootstrap.

Single-crate, `cfg`-gated — one crate ships every backend, selected at
compile time. Mirrors the `webview` SDK's layout (see that crate for
the same rationale on why one crate beats the `maps`-style split).

```rust,ignore
use video::prelude::*;
use runtime_core::{signal, Ref};

// App bootstrap — one line per third-party SDK:
let mut backend = WebBackend::new("#app");
video::register(&mut backend);

// Inside a `ui!` block. `Video` is an external primitive, so it's
// interpolated as an expression:
let src = signal("https://example.com/clip.mp4".to_string());
let v: Ref<VideoHandle> = Ref::new();
ui! {
    View {
        { video::Video(VideoProps {
            src: video::src(move || src.get()),
            autoplay: true,
            controls: true,
            ..Default::default()
        }).bind(v.clone()) }
    }
}

// Imperative ops at any later point, via the bound handle:
v.with(|h| h.play());
v.with(|h| h.seek(10.0));
```

## What you get

Every backend plays a media URL through the platform's native player
and converges on the same author-observable behavior — a reactive
`src`, `autoplay` / `controls` / `loop_playback` flags, and imperative
`play` / `pause` / `seek` ops. The *mechanism* differs per platform:

| Target | Mechanism |
| --- | --- |
| Web (wasm32) | `<video>` element |
| iOS | `AVPlayer` + `AVPlayerLayer` hosted in a stock `UIView` |
| Android | `android.widget.VideoView` (`setVideoURI` ← `Uri.parse`) |
| Other (wgpu desktop, terminal, …) | the framework's `External` "not supported" placeholder |

### Backend caveats

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
per-backend handler swaps the playing clip on change (it subscribes via
`Effect::new(...)` when it builds the native view). Use [`src`] to
coerce a `&str`, `String`, or `Fn() -> String` into the stored closure
shape.

`autoplay`, `controls`, and `loop_playback` are static at construction
— a re-render with different values tears down and re-mounts the view,
which is the desired behavior for those flags anyway.

Imperative ops go through the bound `Ref<VideoHandle>`: `play()`,
`pause()`, `seek(seconds)`. Bind a handle with `.bind(my_ref.clone())`
on the value `Video(..)` returns.

[`src`]: src/lib.rs

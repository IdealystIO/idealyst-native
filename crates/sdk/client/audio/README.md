# `audio`

Cross-platform sound **playback** — load a sound from bytes, a file path,
or a URL, play it, and get back a thin handle you control. This is the
*playback* peer of the capture SDKs [`microphone`](../microphone) and
[`camera`](../camera): where those turn the device's mic/camera into an
`AudioStream` / `MediaStream`, `audio` takes prepared audio and pushes it
back out the speakers. It does NOT depend on `media-stream` — it's the same
"raw capability, no opinion" shape, on the output side.

```rust
use audio::{load, AudioSource};

# async fn demo() -> Result<(), audio::AudioError> {
// Decode / prepare once…
let sound = load(AudioSource::path("assets/ding.wav")).await?;

// …then play. `play()` returns a controllable `Playback` handle.
let playback = sound.play();
playback.set_volume(0.5);
playback.set_looping(true);

// Dropping the handle stops the sound; `stop()` is the explicit form.
playback.stop();
# Ok(())
# }
```

## What you get

Two free functions and two small handle types:

- `load(source) -> Result<Sound, AudioError>` — **async**; decodes /
  prepares the audio. `AudioSource` is `Bytes(Vec<u8>)` / `Path(PathBuf)` /
  `Url(String)`, with `AudioSource::bytes(..)` / `path(..)` / `url(..)`
  constructors.
- `Sound::play() -> Playback` — starts playback, returning a handle. A
  `Sound` is reusable: call `play()` as many times as you like. Where the
  platform supports it (web, Apple), overlapping calls produce independent,
  mixing voices — good for layering short sound effects. On **Android** one
  `MediaPlayer` is reused per `Sound`, so a second `play()` *restarts* it
  rather than layering (use a `SoundPool`-style pool for polyphonic SFX —
  see *Scope*).
- `Playback` — a **RAII** handle: while it's alive the sound plays (looping
  if asked); **dropping it stops the sound** and releases the platform
  player. `stop(self)` is the explicit equivalent of `drop(playback)`.
  Controls: `pause()`, `resume()`, `set_volume(f32)` (clamped to `[0,1]`),
  `set_looping(bool)`, `is_playing() -> bool`.
- `AudioError` — `Backend(String)` (player/IO failure), `Decode(String)`
  (undecodable audio), `NotSupported` (no player on this platform).

Every backend delivers the **same shape** — the platforms diverge in
mechanism, not in the API you call.

## Per-platform mechanism

| Target | Player | Status |
| --- | --- | --- |
| web (wasm32) | `HTMLAudioElement` (`new Audio()` + blob/URL `src`) | runnable |
| iOS / macOS / tvOS | `AVAudioPlayer` (objc2) | compile-checked only ⚠️ |
| Android | `MediaPlayer` (JNI) | compile-checked only ⚠️ |
| Linux / desktop ALSA·PipeWire | **GStreamer** (`decodebin` pipeline) — streams remote URLs | verified on Linux |
| Windows / other native | none — `load` returns `NotSupported` | honest fallback |

⚠️ *Compile-checked only*: the Apple and Android backends are typed against
their native player APIs and compile for those targets, but the playback
path has not been exercised on a device/simulator from this repo. The
non-obvious platform invariants (AVFoundation copies/retains init data;
`MediaPlayer` is bound to its creating thread; init failure → `Decode`) are
documented inline so a device bring-up has the expectations written down.

The **Linux backend** uses GStreamer (the native Linux media stack), so
remote `http(s)` URLs are **progressively streamed** via `souphttpsrc` —
parity with Android/Apple/web, which all stream remote URLs. In-memory
`Bytes` are fed via `giostreamsrc` + a `gio::MemoryInputStream` (no temp
file). Verified on Linux: headless preroll/decode + looping tests, plus real
remote-URL streaming against a public MP3.

The remaining **Windows / other-native fallback** is deliberate: no player is
pulled in there, to keep the crate dependency-light. A half-working path
would be worse than an honest `NotSupported`; adding a real one later is a
drop-in behind the same API.

`load` is `async` for a uniform surface across the genuinely-async web/URL
paths and the synchronous native decode. A `Bytes` source is staged as
needed per platform (a `Blob` object URL on web, an `NSData` on Apple, a
temp cache file on Android) and cleaned up when the `Sound` drops.

## Permissions

None. Foreground playback needs no OS permission on any platform, so this
crate declares no capability and the CLI injects nothing.

**Background audio is out of scope.** Continuing to play while the app is
backgrounded is a separate capability that requires app config — iOS
`UIBackgroundModes` `audio` (plus an active `AVAudioSession`), an Android
foreground service — and is not handled here. This crate plays in the
foreground.

## Scope

Foreground playback of loaded sounds, with the thin control surface above —
the unopinionated raw capability. Deliberately left to higher-level SDKs
built on top of this one:

- **Mixing / effects / spatialization** — a Web Audio / AVAudioEngine /
  `SoundPool`-and-effects layer.
- **Low-latency SFX pools** — Android `SoundPool`, iOS `AVAudioEngine`
  buffers; true polyphony for tiny overlapping clips.
- **Playlists / queues / gapless** — sequencing is a layer above one sound.
- **Seeking, fades, position callbacks** — richer transport control.
- **Background audio** — see *Permissions*.

The relationship to the capture SDKs: `microphone`/`camera` *produce*
streams; `audio` *consumes* prepared sounds. A future layer that plays an
`AudioStream` live (rather than a loaded `Sound`) would be the natural
bridge between them.

## Testing checklist

Manual verification per backend — an unchecked **native** box means the code
compiles for that target but isn't confirmed on real hardware yet (see the
verification note above). Tick each item as you exercise it.

**Automated**
- [ ] `cargo test -p audio` — `AudioSource` constructors, error `Display`, `load` of garbage bytes is a typed error not a panic
- [ ] `cargo build -p audio --features catalog` — recipes/docs compile
- [ ] `cargo build -p audio --target wasm32-unknown-unknown` — web target

**Behavior**
- [ ] **Web** — `load()` then `play()` produces sound via `HTMLAudioElement`; `pause`/`resume`/`stop`/`set_volume`/`set_looping` behave; a second `play()` layers an independent voice; dropping `Playback` stops it.
- [ ] **iOS** — `load(bytes/path/url)` then `play()` produces sound via `AVAudioPlayer`; the control surface behaves; overlapping `play()` calls mix; dropping `Playback` stops and releases the player.
- [ ] **macOS** — same `AVAudioPlayer` flow (incl. `load(Url)` fetch path).
- [ ] **Android** — `load()` + `play()` produces sound via `MediaPlayer`; a second `play()` *restarts* (one player per `Sound`, no layering); controls + drop-stops behave.
- [x] **Linux** — `load(bytes/path/url)` + `play()` produces sound via GStreamer; a **remote `http(s)` URL streams progressively** (`souphttpsrc`); a second `play()` layers an independent voice; controls + drop-stops behave. (Headless preroll/looping tests pass; remote-URL streaming verified against a public MP3.)
- [ ] **Windows** — `load()` returns `NotSupported` (honest fallback, no player pulled in).

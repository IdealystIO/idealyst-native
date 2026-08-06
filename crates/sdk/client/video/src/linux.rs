//! Linux (GTK4) implementation of the Video SDK.
//!
//! Builds a real `gtk::Video` widget backed by GTK's built-in media
//! pipeline. `gtk::Video` wraps a `GtkMediaControls`-overlaid
//! `GtkPicture`, and `gtk::MediaFile` decodes through GTK's media
//! backend — GStreamer on Linux (`media-gstreamer`), which GTK links
//! itself. So the SDK needs **no** direct `gstreamer-rs` dependency: the
//! `gtk4` crate's typed `Video` / `MediaFile` / `MediaStream` bindings
//! are the whole surface, and the actual codecs come from the system's
//! GStreamer install at runtime (see the module footer for the package
//! note).
//!
//! # Source resolution + reactivity
//!
//! A single `effect!` resolves [`VideoProps::source`] each run and swaps
//! the widget's `MediaStream`. Because `resolve()` runs *inside* the
//! effect, any signal it reads re-fires this and re-populates the view —
//! one mechanism for a reactive URL, a swap-to-none, and (best-effort) a
//! live stream. A `MediaContent::Url` is loaded via
//! [`gtk::MediaFile::for_filename`] for a local filesystem path, or
//! [`gtk::MediaFile::for_file`] over a `gio::File::for_uri` for anything
//! carrying a URI scheme (`http(s)://`, `file://`, `data:`…).
//!
//! # Imperative ops
//!
//! `LinuxNode` keeps its wrapped `gtk::Widget` crate-private and exposes
//! no public accessor, so — unlike every other backend, whose `Node`
//! type *is* (or reveals) the native handle — a third-party SDK can't
//! read the `gtk::Video` back out of the node it's handed at op-dispatch
//! time. We bridge build-time (where we own the widget) to op-time (where
//! we get only `&LinuxNode`) through a thread-local table keyed by the
//! node's stable per-node id. See [`node_id`] for how that id is
//! recovered and the graceful-degradation contract.

use crate::{MediaContent, VideoOps, VideoProps};
use backend_linux::{LinuxBackend, LinuxNode};
use gtk4::{gdk, gio, glib};
use gtk4::prelude::*;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub(crate) static OPS: &dyn VideoOps = &LinuxVideoOps;

// =========================================================================
// FramePaintable — display a LIVE, CPU-pushed `MediaStream` (camera /
// screen-capture) as a `GdkPaintable` on a `gtk::Picture`.
//
// `gtk::Video`/`GtkMediaStream` only render a *media backend*'s frames (a
// `GtkMediaFile` decoding a URL); there is NO supported way to feed a
// `GtkMediaStream` subclass raw pushed frames (`MediaStreamImpl` exposes
// play/pause/seek, not the paintable snapshot). So a live `media_stream`
// feed — which delivers tightly-packed `RGBA8` via `latest()` — is shown by
// uploading each new frame to a `gdk::MemoryTexture` and drawing it here.
// Mirrors the macOS backend's CPU fallback (CGImage → CALayer contents).
// =========================================================================

mod frame_paintable {
    use super::gdk;
    use gtk4::glib;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct Inner {
        /// The most recent frame, drawn every snapshot until replaced.
        texture: RefCell<Option<gdk::MemoryTexture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Inner {
        const NAME: &'static str = "IdealystVideoFramePaintable";
        type Type = FramePaintable;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for Inner {}

    impl PaintableImpl for Inner {
        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            if let Some(tex) = self.texture.borrow().as_ref() {
                // GdkMemoryTexture is itself a GdkPaintable; let it fill the box.
                tex.snapshot(snapshot, width, height);
            }
        }
        fn intrinsic_width(&self) -> i32 {
            self.texture.borrow().as_ref().map(|t| t.width()).unwrap_or(0)
        }
        fn intrinsic_height(&self) -> i32 {
            self.texture.borrow().as_ref().map(|t| t.height()).unwrap_or(0)
        }
    }

    glib::wrapper! {
        /// A trivial `GdkPaintable` that draws the latest pushed frame.
        pub struct FramePaintable(ObjectSubclass<Inner>) @implements gdk::Paintable;
    }

    impl FramePaintable {
        pub fn new() -> Self {
            glib::Object::new()
        }

        /// Swap in a new frame texture and request a repaint. The intrinsic size
        /// changes with the texture, so also invalidate size on the first frame.
        pub fn set_texture(&self, tex: gdk::MemoryTexture) {
            let prev = self.imp().texture.replace(Some(tex));
            let size_changed = match (&prev, self.imp().texture.borrow().as_ref()) {
                (Some(p), Some(n)) => p.width() != n.width() || p.height() != n.height(),
                _ => true,
            };
            if size_changed {
                self.invalidate_size();
            }
            self.invalidate_contents();
        }
    }
}

use frame_paintable::FramePaintable;

thread_local! {
    /// node id → live `gtk::Video`. Populated by [`build_video`] at mount,
    /// cleared by the `on_cleanup` it installs (so a popped screen stops
    /// decoding). The imperative [`VideoOps`] look their target up here by
    /// the id parsed from the `LinuxNode` they're dispatched with. Single-
    /// threaded (GTK main loop) — `gtk::Video` is `!Send`, so a thread-local
    /// is the correct home.
    static VIDEOS: RefCell<HashMap<u64, gtk4::Video>> = RefCell::new(HashMap::new());
}

// v2: no inventory self-registration. The External table these registrars
// fed is gone; an app installs this handler on the scene `Registry` at its
// boot seam instead, and an unregistered payload panics at realize rather
// than silently falling through to a placeholder.

// =========================================================================
// Build + reactive source
// =========================================================================

pub(crate) fn build_video(props: &Rc<VideoProps>, b: &mut LinuxBackend) -> LinuxNode {
    let video = gtk4::Video::new();

    // Static, construction-time props. `Video::set_autoplay` / `set_loop`
    // apply to whatever `MediaStream` the widget currently holds AND to any
    // set later, so setting them once up front covers reactive src swaps.
    video.set_autoplay(props.autoplay);
    video.set_loop(props.loop_playback);

    // `controls`: `gtk::Video` always carries an auto-hiding
    // `GtkMediaControls` overlay — there's no public property to remove it.
    // The overlay fades out when the pointer leaves, so `controls: false`
    // reads close to intent; documented as a known partial mapping.
    let _ = props.controls;

    // Live-frame surface for a CPU-pushed `MediaStream` (camera / screen
    // capture): a `gtk::Picture` drawing a `FramePaintable`, overlaid ON TOP
    // of the `gtk::Video`. It's shown only while a live frame source is active
    // (opaque, so it fully covers the idle video underneath); a URL source
    // hides it and plays through `gtk::Video` as before. This is why a live
    // camera now displays instead of clearing to nothing.
    let picture = gtk4::Picture::new();
    // `object_fit`: `gtk::Picture` supports `content-fit` directly, so the live
    // path honors Cover/Contain (the URL `gtk::Video` still aspect-fits —
    // reaching its private inner `GtkPicture` isn't possible; partial there).
    picture.set_content_fit(match props.object_fit {
        crate::ObjectFit::Cover => gtk4::ContentFit::Cover,
        _ => gtk4::ContentFit::Contain,
    });
    picture.set_visible(false);
    // Fill the widget's Taffy-imposed allocation rather than collapsing to the
    // frame's intrinsic size, so the live surface covers the whole box (the
    // `content-fit` above then decides crop-vs-letterbox WITHIN it).
    picture.set_halign(gtk4::Align::Fill);
    picture.set_valign(gtk4::Align::Fill);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    let frame_paintable = FramePaintable::new();
    picture.set_paintable(Some(&frame_paintable));

    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&video));
    overlay.add_overlay(&picture);
    // The overlaid Picture must be measured/sized to the overlay's full
    // allocation (not clipped to its own natural size).
    overlay.set_measure_overlay(&picture, true);

    // Live-frame pump handle: the `add_tick_callback` id driving `latest()` →
    // texture uploads. Removed when the source switches away from a live stream
    // or the node unmounts, so a hidden/gone Picture stops polling.
    let tick: Rc<RefCell<Option<gtk4::TickCallbackId>>> = Rc::new(RefCell::new(None));

    // One reactive populate effect. Owned by the walker's active scope, so it
    // re-fires when a signal the source reads changes.
    let video_for_effect = video.clone();
    let picture_for_effect = picture.clone();
    let paintable_for_effect = frame_paintable.clone();
    let tick_for_effect = tick.clone();
    let props_for_effect = props.clone();
    let muted = props.muted;
    let autoplay = props.autoplay;
    let loop_playback = props.loop_playback;
    runtime_core::effect!({
        // Any source change stops the previous live pump; the live arm below
        // reinstalls one if still a CPU stream.
        if let Some(id) = tick_for_effect.borrow_mut().take() {
            id.remove();
        }
        match props_for_effect.source.resolve() {
            MediaContent::Url(u) if !u.is_empty() => {
                picture_for_effect.set_visible(false);
                let media = if is_uri(&u) {
                    gtk4::MediaFile::for_file(&gio::File::for_uri(u.as_str()))
                } else {
                    gtk4::MediaFile::for_filename(&u)
                };
                media.set_muted(muted);
                media.set_loop(loop_playback);
                video_for_effect.set_media_stream(Some(&media));
                if autoplay {
                    media.play();
                }
            }
            MediaContent::Stream(s) => {
                // A producer MAY publish a native `gtk::MediaStream` (a fully
                // decoded GTK stream); attach it to `gtk::Video` directly.
                if let Some(stream) = s
                    .native_source()
                    .and_then(|n| n.downcast_ref::<gtk4::MediaStream>().cloned())
                {
                    picture_for_effect.set_visible(false);
                    stream.set_muted(muted);
                    stream.set_loop(loop_playback);
                    video_for_effect.set_media_stream(Some(&stream));
                    if autoplay {
                        stream.play();
                    }
                } else {
                    // CPU frame feed (camera / screen capture): drive the
                    // Picture's paintable from `latest()` on the frame clock.
                    video_for_effect.set_media_stream(gtk4::MediaStream::NONE);
                    picture_for_effect.set_visible(true);
                    *tick_for_effect.borrow_mut() =
                        Some(spawn_frame_pump(&picture_for_effect, &paintable_for_effect, s));
                }
            }
            // Empty URL or no source → clear both surfaces.
            _ => {
                picture_for_effect.set_visible(false);
                video_for_effect.set_media_stream(gtk4::MediaStream::NONE);
            }
        }
    });

    // Register the OVERLAY (Video + live Picture) with the Taffy tree and record
    // the inner Video for imperative ops.
    let node = b.register_external_view(overlay.upcast());
    let id = node_id(&node);
    VIDEOS.with(|m| m.borrow_mut().insert(id, video.clone()));

    // Tear everything down when the Video unmounts (screen pop / navigation).
    let tick_for_cleanup = tick.clone();
    runtime_core::on_cleanup(move || {
        if let Some(id) = tick_for_cleanup.borrow_mut().take() {
            id.remove();
        }
        if let Some(v) = VIDEOS.with(|m| m.borrow_mut().remove(&id)) {
            if let Some(s) = v.media_stream() {
                s.pause();
            }
            v.set_media_stream(gtk4::MediaStream::NONE);
        }
    });

    node
}

/// Install a frame-clock tick that pulls the latest CPU frame from `stream` and
/// uploads it to `paintable` as a `gdk::MemoryTexture`. Deduped on the stream's
/// monotonic `generation()`, so an idle stream costs one atomic load per frame
/// and never re-uploads an unchanged frame — the same shape as the macOS CPU
/// fallback. Runs on the GTK main thread (the frame clock), which is required:
/// `MediaStream` is `!Send` and `latest()` copies under a mutex, so no
/// cross-thread hand-off is needed even though the producer writes frames on a
/// capture thread.
fn spawn_frame_pump(
    driver: &gtk4::Picture,
    paintable: &FramePaintable,
    stream: media_stream::MediaStream,
) -> gtk4::TickCallbackId {
    let paintable = paintable.clone();
    // `add_tick_callback` wants `Fn`, so per-frame mutable state lives behind a
    // RefCell (single-threaded, main loop only).
    let state = RefCell::new((u64::MAX, Vec::<u8>::new()));
    driver.add_tick_callback(move |_widget, _clock| {
        let mut st = state.borrow_mut();
        let gen = stream.generation();
        if gen != st.0 {
            st.0 = gen;
            let (_last, scratch) = &mut *st;
            if let Some((w, h)) = stream.latest(scratch) {
                if w > 0 && h > 0 {
                    let bytes = glib::Bytes::from(&scratch[..]);
                    let tex = gdk::MemoryTexture::new(
                        w as i32,
                        h as i32,
                        // Camera/screen frames are straight (non-premultiplied)
                        // RGBA8, tightly packed (stride = w*4; see the camera
                        // backend's row-padding invariant).
                        gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        (w * 4) as usize,
                    );
                    paintable.set_texture(tex);
                }
            }
        }
        glib::ControlFlow::Continue
    })
}

/// True when `s` carries a URI scheme (`scheme://…` or `data:`), meaning it
/// should load via `gio::File::for_uri` rather than as a filesystem path.
/// A bare local path (`/home/u/clip.mp4`, `clip.mp4`) has no scheme and
/// takes the `MediaFile::for_filename` branch.
fn is_uri(s: &str) -> bool {
    s.contains("://") || s.starts_with("data:")
}

/// A `LinuxNode`'s stable per-node id — the key for the [`VIDEOS`] table,
/// bridging build-time (where we own the `gtk::Video`) and op-time (where
/// we're handed only `&LinuxNode`). Reads the backend's public
/// `LinuxNode::id()` accessor.
fn node_id(node: &LinuxNode) -> u64 {
    node.id()
}

// =========================================================================
// Imperative ops
// =========================================================================

struct LinuxVideoOps;

impl VideoOps for LinuxVideoOps {
    fn play(&self, node: &dyn Any) {
        with_stream(node, |s| s.play());
    }

    fn pause(&self, node: &dyn Any) {
        with_stream(node, |s| s.pause());
    }

    fn seek(&self, node: &dyn Any, seconds: f32) {
        // GtkMediaStream timestamps are in microseconds.
        with_stream(node, |s| {
            if s.is_seekable() {
                s.seek((seconds as f64 * 1_000_000.0) as i64);
            }
        });
    }

    fn set_muted(&self, node: &dyn Any, muted: bool) {
        with_stream(node, |s| s.set_muted(muted));
    }

    fn position(&self, node: &dyn Any) -> f32 {
        with_stream_ret(node, |s| s.timestamp() as f32 / 1_000_000.0).unwrap_or(0.0)
    }

    fn duration(&self, node: &dyn Any) -> f32 {
        // `duration()` is 0 before the stream is prepared and for a live/
        // unbounded stream — both are useless as a scrubber denominator, so
        // the caller reads 0.0 in those cases, matching the web leaf.
        with_stream_ret(node, |s| {
            let d = s.duration();
            if d > 0 {
                d as f32 / 1_000_000.0
            } else {
                0.0
            }
        })
        .unwrap_or(0.0)
    }
}

/// Look up the `gtk::Video` for `node` and, if it currently holds a
/// `MediaStream`, run `f` against it. No-op when the node isn't a
/// `LinuxNode`, isn't in the table (already unmounted, or an id-parse miss),
/// or has no stream attached yet.
fn with_stream(node: &dyn Any, f: impl FnOnce(&gtk4::MediaStream)) {
    with_stream_ret(node, |s| f(s));
}

/// `with_stream` returning a value. `None` when there's no reachable stream.
fn with_stream_ret<R>(node: &dyn Any, f: impl FnOnce(&gtk4::MediaStream) -> R) -> Option<R> {
    let ln = node.downcast_ref::<LinuxNode>()?;
    let id = node_id(ln);
    VIDEOS.with(|m| {
        let m = m.borrow();
        let video = m.get(&id)?;
        let stream = video.media_stream()?;
        Some(f(&stream))
    })
}

#[cfg(test)]
mod tests {
    use super::{is_uri, FramePaintable};
    use gtk4::gdk;
    use gtk4::prelude::*;

    /// Regression: a live camera / screen `MediaStream` produced NOTHING on Linux
    /// (the backend cleared the surface for a CPU-frame stream — no `gtk::Video`
    /// path exists for pushed frames). The fix draws each `latest()` frame through
    /// a `FramePaintable`. This exercises the fix's core data path — a real
    /// `MediaStream` RGBA frame → `gdk::MemoryTexture` → the paintable reports its
    /// size — without needing a live frame clock. Skips if GTK can't init
    /// (headless CI with no display), like the repo's other GTK tests.
    #[test]
    fn frame_paintable_shows_a_pushed_media_stream_frame() {
        if gtk4::init().is_err() {
            eprintln!("SKIP: no display / GTK init failed");
            return;
        }
        // An empty paintable has no intrinsic size (nothing to draw).
        let paintable = FramePaintable::new();
        assert_eq!(paintable.intrinsic_width(), 0);
        assert_eq!(paintable.intrinsic_height(), 0);

        // Push a known 3×2 RGBA frame through a real MediaStream (the producer
        // side the camera backend drives), then upload the latest frame exactly as
        // the pump does.
        let (stream, writer) = media_stream::MediaStream::new();
        let (w, h) = (3u32, 2u32);
        let pixels: Vec<u8> = (0..(w * h))
            .flat_map(|i| [i as u8 * 10, 0, 0, 255])
            .collect();
        writer.write_rgba8(w, h, &pixels);

        let mut scratch = Vec::new();
        let (lw, lh) = stream.latest(&mut scratch).expect("a frame was written");
        assert_eq!((lw, lh), (w, h));
        let bytes = gtk4::glib::Bytes::from(&scratch[..]);
        let tex = gdk::MemoryTexture::new(
            lw as i32,
            lh as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            (lw * 4) as usize,
        );
        paintable.set_texture(tex);

        // The paintable now advertises the frame's size — it will draw it.
        assert_eq!(paintable.intrinsic_width(), w as i32);
        assert_eq!(paintable.intrinsic_height(), h as i32);
    }

    #[test]
    fn is_uri_distinguishes_schemes_from_paths() {
        assert!(is_uri("https://example.com/clip.mp4"));
        assert!(is_uri("http://host/v.webm"));
        assert!(is_uri("file:///home/u/clip.mp4"));
        assert!(is_uri("data:video/mp4;base64,AAAA"));
        assert!(!is_uri("/home/u/clip.mp4"));
        assert!(!is_uri("clip.mp4"));
        assert!(!is_uri("./relative/clip.mkv"));
    }
}

#[cfg(all(test, target_os = "linux", not(target_arch = "wasm32")))]
mod linux_registration_tests {
    /// Regression: the whiteboard's recording preview / any `Video` didn't
    /// play on Linux because `video/linux.rs` had a real `gtk::Video`
    /// backend but no `inventory::submit!`, so it never self-registered and
    /// `Video` fell to the framework's External placeholder. This asserts the
    /// submit exists (drained by `LinuxBackend::new`).
    #[test]
    fn video_handler_auto_registers_on_linux() {
        let count = inventory::iter::<backend_linux::LinuxExternalRegistrar>().count();
        assert!(count >= 1, "video must submit a LinuxExternalRegistrar so Video lowers to gtk::Video");
    }
}

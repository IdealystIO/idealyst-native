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
use gtk4::gio;
use gtk4::prelude::*;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub(crate) static OPS: &dyn VideoOps = &LinuxVideoOps;

thread_local! {
    /// node id → live `gtk::Video`. Populated by [`build_video`] at mount,
    /// cleared by the `on_cleanup` it installs (so a popped screen stops
    /// decoding). The imperative [`VideoOps`] look their target up here by
    /// the id parsed from the `LinuxNode` they're dispatched with. Single-
    /// threaded (GTK main loop) — `gtk::Video` is `!Send`, so a thread-local
    /// is the correct home.
    static VIDEOS: RefCell<HashMap<u64, gtk4::Video>> = RefCell::new(HashMap::new());
}

/// Register the Linux `Video` external handler on `backend`. Call once at
/// app boot (the app's `register_extensions` on Linux) so `Video`
/// elements lower to a native `gtk::Video` instead of the framework's
/// External placeholder.
pub fn register(backend: &mut LinuxBackend) {
    backend.register_external::<VideoProps, _>(|props, b| build_video(props, b));
}

// Self-register at backend construction (no app-side `register` call needed) —
// the mirror of the macOS submit. Without this, `Video` elements fell through
// to the External placeholder on GTK and never played. See
// [[project_inventory_self_registration]].
inventory::submit! {
    backend_linux::LinuxExternalRegistrar(register)
}

// =========================================================================
// Build + reactive source
// =========================================================================

fn build_video(props: &Rc<VideoProps>, b: &mut LinuxBackend) -> LinuxNode {
    let video = gtk4::Video::new();

    // Static, construction-time props. `Video::set_autoplay` / `set_loop`
    // apply to whatever `MediaStream` the widget currently holds AND to any
    // set later, so setting them once up front covers reactive src swaps.
    video.set_autoplay(props.autoplay);
    video.set_loop(props.loop_playback);

    // `controls`: `gtk::Video` always carries an auto-hiding
    // `GtkMediaControls` overlay — there's no public property to remove it
    // (a truly chrome-less surface would need a bare `GtkPicture` + a
    // hand-driven `MediaStream`, a separate widget). The overlay fades out
    // when the pointer leaves, so `controls: false` reads close to intent
    // without a bespoke widget; documented as a known partial mapping.
    let _ = props.controls;

    // `object_fit`: `gtk::Video` aspect-fits its frame inside the box
    // (letterboxed) — exactly `ObjectFit::Contain`, the default. `Cover`
    // (fill + crop) would require reaching into the widget's internal
    // `GtkPicture` to set `content-fit`, which is private structure; left as
    // Contain rather than depend on GTK internals. Documented as partial.
    let _ = props.object_fit;

    // One reactive populate effect. Owned by the walker's active scope
    // (the framework runs this handler inside it), so it survives past
    // handler return and re-fires when a signal the source reads changes.
    let video_for_effect = video.clone();
    let props_for_effect = props.clone();
    let muted = props.muted;
    let autoplay = props.autoplay;
    let loop_playback = props.loop_playback;
    runtime_core::effect!({
        match props_for_effect.source.resolve() {
            MediaContent::Url(u) if !u.is_empty() => {
                let media = if is_uri(&u) {
                    gtk4::MediaFile::for_file(&gio::File::for_uri(u.as_str()))
                } else {
                    gtk4::MediaFile::for_filename(&u)
                };
                // `muted` is honored via the stream (not autoplay-forced —
                // GTK plays unmuted media fine, unlike the web's autoplay
                // gate). `loop` is set on both the widget and the stream so a
                // freshly-attached MediaFile loops from its first play.
                media.set_muted(muted);
                media.set_loop(loop_playback);
                video_for_effect.set_media_stream(Some(&media));
                if autoplay {
                    media.play();
                }
            }
            MediaContent::Stream(s) => {
                // Best-effort: if a live-stream source ever publishes a
                // `gtk::MediaStream` as its native handle, attach it. No Linux
                // producer in the `media-stream` crate hands one today
                // (camera/screen-capture have no GTK MediaStream path yet), so
                // in practice this clears the view — documented as unwired.
                match s
                    .native_source()
                    .and_then(|n| n.downcast_ref::<gtk4::MediaStream>().cloned())
                {
                    Some(stream) => {
                        stream.set_muted(muted);
                        stream.set_loop(loop_playback);
                        video_for_effect.set_media_stream(Some(&stream));
                        if autoplay {
                            stream.play();
                        }
                    }
                    None => video_for_effect.set_media_stream(gtk4::MediaStream::NONE),
                }
            }
            // Empty URL or no source → clear the surface.
            _ => video_for_effect.set_media_stream(gtk4::MediaStream::NONE),
        }
    });

    // Register the widget with the backend's Taffy tree (a flex parent sizes
    // + positions it) and record it for imperative ops.
    let node = b.register_external_view(video.clone().upcast());
    let id = node_id(&node);
    VIDEOS.with(|m| m.borrow_mut().insert(id, video.clone()));

    // Tear the player down when the Video unmounts (screen pop / navigation).
    // Without this the stream keeps decoding — audible after the view is gone
    // — and the table entry leaks. Mirrors the macOS leaf's `on_cleanup`.
    runtime_core::on_cleanup(move || {
        if let Some(v) = VIDEOS.with(|m| m.borrow_mut().remove(&id)) {
            if let Some(s) = v.media_stream() {
                s.pause();
            }
            v.set_media_stream(gtk4::MediaStream::NONE);
        }
    });

    node
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
    use super::is_uri;

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

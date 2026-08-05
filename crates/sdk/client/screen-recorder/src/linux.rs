//! Linux capture backend — **xdg-desktop-portal ScreenCast + PipeWire**.
//!
//! Wayland has no privileged screen-grab API; capture goes through
//! `org.freedesktop.portal.ScreenCast`, the compositor-mediated portal. The
//! user picks *what* to share in the portal's own dialog (there is no
//! programmatic monitor/window targeting — that's the security model), and the
//! portal hands back a **PipeWire** stream. We consume that stream with
//! GStreamer, exactly as the `camera` backend consumes a V4L2 stream — same
//! `appsink → FrameWriter` delivery, only the *source element* differs.
//!
//! ## Portal negotiation (`ashpd`)
//!
//! [`start`] runs the standard ScreenCast handshake through `ashpd` (the same
//! portal crate `file-picker`/`file-export` use):
//!
//! 1. [`Screencast::new`] — construct the D-Bus proxy for the ScreenCast
//!    portal. If the portal service isn't present this is where we learn it,
//!    and map to [`RecorderError::Unsupported`].
//! 2. [`create_session`](Screencast::create_session) — open a screencast
//!    session.
//! 3. [`select_sources`](Screencast::select_sources) — declare which
//!    [`SourceType`]s (MONITOR and/or WINDOW) the picker should offer, the
//!    cursor mode, and single-source selection. The SDK's [`Source`] maps to
//!    these types via [`portal_source_types`].
//! 4. [`start`](Screencast::start) — **this opens the interactive "pick what to
//!    share" dialog**. The await blocks until the user approves or cancels; a
//!    cancel comes back as [`RecorderError::PermissionDenied`]. On approval it
//!    returns the [`Streams`] — each carries a PipeWire **node id** + size.
//! 5. [`open_pipe_wire_remote`](Screencast::open_pipe_wire_remote) — get the
//!    raw PipeWire **fd** for the granted session.
//!
//! ## fd + node id → `pipewiresrc`
//!
//! `pipewiresrc` connects to the PipeWire daemon over the portal-provided fd
//! (`fd` property, a raw `gint`) and subscribes to the granted node (`path`
//! property, the node id as a string). No `pipewire`-rs crate is needed — the
//! GStreamer element consumes the fd directly.
//!
//! **fd lifetime invariant:** the [`OwnedFd`] must outlive the pipeline —
//! `pipewiresrc` keeps using it for the whole capture — so the [`Recording`]
//! handle owns it and only drops it after the pipeline is set to `Null`. The
//! portal [`Session`] (and its proxy) are likewise kept alive in the handle:
//! dropping the session tells the compositor to tear the screencast down.
//!
//! ## Capture pipeline (mirrors `camera`)
//!
//! ```text
//!   pipewiresrc fd=<fd> path=<node> ! videoconvert ! videoscale ! videorate
//!     ! video/x-raw,format=RGBA[,width,height],framerate ! appsink
//! ```
//!
//! `videoconvert` normalizes the compositor's native layout (commonly
//! `BGRx`/`BGRA`) to `RGBA8`. Unlike `camera`, the screen source's native size
//! is compositor-chosen and unknown ahead of time, so `videoscale`/`videorate`
//! are in the chain to *honor* a requested [`size`](RecordingConfig::size) /
//! [`fps`](RecordingConfig::fps) rather than fail negotiation; they are
//! pass-throughs when the source already matches. Each `appsink` sample is
//! mapped and pushed into the [`FrameWriter`] as tightly-packed top-down
//! `RGBA8` — the exact same `MediaStream` frame model every other backend
//! publishes. This map/deliver path ([`deliver_sample`]) is copied from
//! `camera`, and the headless test drives it through `videotestsrc`.
//!
//! ## Frame delivery — CPU `RGBA8`, no zero-copy native source
//!
//! Like the V4L2 `camera` path, this exposes no native surface: [`start`]
//! returns `None` for the native source and the only delivery channel is the
//! CPU `RGBA8` writer. **dmabuf zero-copy** (`pipewiresrc` can negotiate
//! `video/x-raw(memory:DMABuf)`) is a documented follow-on — it slots in behind
//! the same pipeline contract without a public-API change.
//!
//! ## Async state changes + the bus thread (no GLib main loop assumed)
//!
//! Same posture as `audio`/`camera`: [`start`] drives the pipeline to `PLAYING`
//! up front (so a negotiation failure surfaces as a typed error at start time),
//! then a dedicated [`bus_loop`] thread owns the bus and drains errors until
//! `Drop` sets the pipeline to `Null` and joins it (RAII stop).
//!
//! ## Permission
//!
//! The portal *is* the permission — the user grants capture by picking a source
//! in the dialog, exactly like web's `getDisplayMedia`. So [`request_permission`]
//! doesn't pop the dialog; it only verifies the ScreenCast portal is reachable
//! (constructs the proxy + reads `AvailableSourceTypes`) and returns `Ok`,
//! deferring the actual grant to [`start`]. A genuinely absent portal maps to
//! [`RecorderError::Unsupported`], never a panic.
//!
//! ## Deferred on this backend
//!
//! - **Audio.** The ScreenCast portal interface does not carry audio; desktop
//!   audio capture would be a separate PipeWire audio node. This backend
//!   captures **video only** — if a caller requests audio via
//!   [`AudioSource`](crate::AudioSource) we log that it's not yet wired and
//!   proceed with video (audio frames are out of the video-frame callback's
//!   scope anyway). Documented follow-on.
//! - **dmabuf zero-copy** (see above).
//! - **Layer exclusion.** There is no capture-exclusion mechanism on the Linux
//!   portal (no per-window `NSWindowSharingNone` analogue), so a
//!   [`PrivateLayer`](crate::PrivateLayer) overlay is *captured* like any other
//!   content. [`start`] logs this rather than silently dropping the intent, and
//!   [`register`] is a documented no-op.

use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType, Streams};
use ashpd::desktop::{PersistMode, Session};
use ashpd::enumflags2::BitFlags;
use ashpd::WindowIdentifier;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};

use crate::{NativeSource, RecorderError, RecordingConfig, Source};
use media_stream::FrameWriter;

/// The portal proxy + session kept alive for the whole capture. Both are
/// `'static` — `ashpd`'s proxies own their D-Bus connection clone, so keeping
/// them here keeps the screencast session (and thus the PipeWire stream) alive
/// until the [`Recording`] drops.
type PortalProxy = Screencast<'static>;
type PortalSession = Session<'static, Screencast<'static>>;

/// One-time, thread-safe, idempotent `gst::init()`. Stores the outcome so a
/// failed init (missing core libs) is reported honestly on every call. The
/// GStreamer "media cluster" pattern shared with `audio`/`camera`.
fn ensure_gst() -> Result<(), RecorderError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string()))
        .clone()
        .map_err(RecorderError::Platform)
}

/// Make a plain element by factory name; a missing element (plugin not
/// installed — e.g. `gst-plugin-pipewire` absent) is a `Platform` failure.
fn make(name: &str) -> Result<gst::Element, RecorderError> {
    gst::ElementFactory::make(name)
        .build()
        .map_err(|e| RecorderError::Platform(format!("missing GStreamer element `{name}`: {e}")))
}

// ---------------------------------------------------------------------------
// Source mapping + error mapping (pure, unit-tested).
// ---------------------------------------------------------------------------

/// Map the SDK's [`Source`] to the portal [`SourceType`]s the picker offers.
///
/// The portal never lets the app *pick* a specific monitor/window — the user
/// does, in the dialog — so this only constrains *which kinds* the dialog
/// presents:
///
/// - [`Source::FullScreen`] → `Monitor` only (whole display).
/// - [`Source::Window`] → `Window` only. A [`WindowSelector`](crate::WindowSelector)
///   `title_hint` cannot be honored programmatically (the user still picks the
///   window); it's a no-op hint on this backend and [`start`] logs that.
/// - [`Source::ThisApp`] / [`Source::UserChoice`] → `Monitor | Window` (the
///   portal has no "this app" concept, so the user chooses either kind).
fn portal_source_types(source: &Source) -> BitFlags<SourceType> {
    match source {
        Source::FullScreen => SourceType::Monitor.into(),
        Source::Window(_) => SourceType::Window.into(),
        Source::ThisApp | Source::UserChoice => SourceType::Monitor | SourceType::Window,
    }
}

/// Map an `ashpd` portal error to a [`RecorderError`]. A user dismissing the
/// share dialog comes back as a cancelled response → [`RecorderError::PermissionDenied`];
/// everything else is a runtime [`RecorderError::Platform`].
fn map_ashpd_error(err: ashpd::Error) -> RecorderError {
    match err {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => {
            RecorderError::PermissionDenied
        }
        other => RecorderError::Platform(format!("screencast portal: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Pipeline (map/deliver copied from `camera`).
// ---------------------------------------------------------------------------

/// A constructed (not-yet-started) pipeline.
struct Built {
    pipeline: gst::Pipeline,
}

/// Assemble `<source> ! videoconvert ! videoscale ! videorate !
/// appsink(caps=RGBA[,WxH],fps)`, wiring the appsink's `new-sample` callback to
/// push mapped `RGBA8` frames into `writer`. `source` is a configured
/// `pipewiresrc` in production and a `videotestsrc` in the synthetic frame test
/// — the rest of the pipeline (and the whole map/deliver path) is identical,
/// which is the point: the test exercises the real appsink code without the
/// portal.
///
/// `videoscale`/`videorate` are present so a requested size/fps is *honored*
/// (the screen source's native size is compositor-chosen and unknown up front);
/// they pass through when the source already matches.
fn build_pipeline(
    source: gst::Element,
    config: &RecordingConfig,
    writer: FrameWriter,
) -> Result<Built, RecorderError> {
    let pipeline = gst::Pipeline::default();
    let videoconvert = make("videoconvert")?;
    let videoscale = make("videoscale")?;
    let videorate = make("videorate")?;

    // Force RGBA output; pin resolution when the caller asked for one and the
    // target framerate (default 30). `videoscale`/`videorate` upstream satisfy
    // these without failing negotiation on a differently-sized source.
    let mut caps = gst::Caps::builder("video/x-raw").field("format", "RGBA");
    if let Some((w, h)) = config.size {
        caps = caps.field("width", w as i32).field("height", h as i32);
    }
    caps = caps.field("framerate", gst::Fraction::new(config.fps.max(1) as i32, 1));
    let appsink = AppSink::builder().caps(&caps.build()).build();

    // Deliver each decoded frame. `new-sample` runs on the streaming thread.
    let writer_cb = writer;
    appsink.set_callbacks(
        AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                deliver_sample(&sample, &writer_cb);
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    let sink_el = appsink.upcast_ref::<gst::Element>();
    pipeline
        .add_many([&source, &videoconvert, &videoscale, &videorate, sink_el])
        .map_err(|e| RecorderError::Platform(format!("pipeline add failed: {e}")))?;
    gst::Element::link_many([&source, &videoconvert, &videoscale, &videorate, sink_el])
        .map_err(|e| RecorderError::Platform(format!("capture chain link failed: {e}")))?;

    Ok(Built { pipeline })
}

/// Map one `appsink` sample read-only and push it into the writer as
/// tightly-packed top-down `RGBA8`. Copied verbatim from the `camera` backend.
///
/// **Row-padding invariant:** GStreamer rounds a raw-video row stride up to a
/// multiple of 4 bytes; RGBA's 4-byte pixel makes `width * 4` already a
/// multiple of 4, so an RGBA buffer is tightly packed with NO row padding —
/// `buffer.len() == width * height * 4`. The fast path passes the mapped slice
/// straight through; a defensive repack handles the (not-expected) padded case.
fn deliver_sample(sample: &gst::Sample, writer: &FrameWriter) {
    let Some(buffer) = sample.buffer() else { return };
    let Some(caps) = sample.caps() else { return };
    let Some(s) = caps.structure(0) else { return };
    let (Ok(width), Ok(height)) = (s.get::<i32>("width"), s.get::<i32>("height")) else {
        return;
    };
    if width <= 0 || height <= 0 {
        return;
    }
    let Ok(map) = buffer.map_readable() else { return };
    let (w, h) = (width as u32, height as u32);
    let expected = w as usize * h as usize * 4;
    let data = map.as_slice();

    if data.len() == expected {
        // Tightly packed (the RGBA invariant) — the common and only real path.
        writer.write_rgba8(w, h, data);
    } else if data.len() > expected {
        // Defensive: padded rows. Repack row-by-row into a tight RGBA buffer.
        let row = w as usize * 4;
        let stride = data.len() / h as usize;
        if stride >= row {
            let mut tight = Vec::with_capacity(expected);
            for y in 0..h as usize {
                let start = y * stride;
                tight.extend_from_slice(&data[start..start + row]);
            }
            writer.write_rgba8(w, h, &tight);
        }
    }
    // A short buffer (< expected) is dropped rather than delivering a partial
    // frame — `write_rgba8` would reject it anyway.
}

/// Pop the first `Error` on the pipeline's bus into a `Platform` error; falls
/// back to a generic message when the state change left no detail.
fn drain_error(pipeline: &gst::Pipeline) -> RecorderError {
    if let Some(bus) = pipeline.bus() {
        while let Some(msg) =
            bus.timed_pop_filtered(gst::ClockTime::from_mseconds(200), &[gst::MessageType::Error])
        {
            if let gst::MessageView::Error(e) = msg.view() {
                let detail = match e.debug() {
                    Some(d) if !d.is_empty() => format!("{} ({d})", e.error()),
                    _ => e.error().to_string(),
                };
                return RecorderError::Platform(format!("capture pipeline error: {detail}"));
            }
        }
    }
    RecorderError::Platform("capture pipeline failed to reach PLAYING (no error detail)".into())
}

/// The bus loop on the live capture handle's dedicated thread. Drains ongoing
/// errors (never panicking) and exits when `quit` is set. Copied in shape from
/// the `audio`/`camera` backends' `bus_loop`.
fn bus_loop(bus: gst::Bus, quit: Arc<AtomicBool>) {
    loop {
        if quit.load(Ordering::Relaxed) {
            break;
        }
        let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
            continue;
        };
        if let gst::MessageView::Error(e) = msg.view() {
            eprintln!(
                "screen-recorder(linux): capture error: {} ({:?})",
                e.error(),
                e.debug()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Recording handle. Owns the pipeline + bus thread + the portal fd/session;
// `Drop` stops capture (state → `Null`), joins the thread, then releases the
// fd + session — RAII stop, the shape as `camera`'s `StreamHandle`.
// ---------------------------------------------------------------------------

pub(crate) struct Recording {
    pipeline: gst::Pipeline,
    quit: Arc<AtomicBool>,
    bus_thread: Option<JoinHandle<()>>,
    /// The portal PipeWire fd — MUST outlive the pipeline (`pipewiresrc` uses
    /// it for the whole capture). Dropped last, after the pipeline is `Null`.
    _fd: OwnedFd,
    /// Keeps the screencast session (and its D-Bus connection) alive; dropping
    /// it tells the compositor to tear the stream down.
    _session: PortalSession,
    _proxy: PortalProxy,
}

impl Drop for Recording {
    fn drop(&mut self) {
        // Signal the bus thread first, then tear the pipeline down, then join.
        // The fd + session drop after (field order) — the pipeline no longer
        // touches the fd once it's `Null`.
        self.quit.store(true, Ordering::Relaxed);
        let _ = self.pipeline.set_state(gst::State::Null);
        if let Some(thread) = self.bus_thread.take() {
            let _ = thread.join();
        }
    }
}

/// Drive a built pipeline to `PLAYING`, blocking until it either negotiates and
/// runs or fails — so a bad node/caps request is reported at start time, not
/// swallowed. On success spawns the bus thread and returns the running parts.
fn start_pipeline(built: Built) -> Result<(gst::Pipeline, Arc<AtomicBool>, JoinHandle<()>), RecorderError> {
    let pipeline = built.pipeline;

    let bus = pipeline
        .bus()
        .ok_or_else(|| RecorderError::Platform("pipeline has no bus".into()))?;

    if pipeline.set_state(gst::State::Playing).is_err() {
        let err = drain_error(&pipeline);
        let _ = pipeline.set_state(gst::State::Null);
        return Err(err);
    }

    // Block up to 5 s for the async state change to reach PLAYING (pipewiresrc
    // connects to the daemon + negotiates a format). A failure here surfaces
    // now as a typed error rather than a silently dead stream.
    let (result, _current, _pending) = pipeline.state(gst::ClockTime::from_seconds(5));
    if result.is_err() {
        let err = drain_error(&pipeline);
        let _ = pipeline.set_state(gst::State::Null);
        return Err(err);
    }

    let quit = Arc::new(AtomicBool::new(false));
    let bus_thread = {
        let quit = quit.clone();
        std::thread::spawn(move || bus_loop(bus, quit))
    };
    Ok((pipeline, quit, bus_thread))
}

// ---------------------------------------------------------------------------
// Portal negotiation.
// ---------------------------------------------------------------------------

/// Construct the ScreenCast portal proxy and verify the interface is actually
/// present (reading `AvailableSourceTypes` hits the real portal). A missing
/// portal — no service, interface absent — is [`RecorderError::Unsupported`];
/// this is the honest "no backend here" signal, distinct from a runtime
/// `Platform` failure once the portal exists.
async fn probe_portal() -> Result<PortalProxy, RecorderError> {
    let proxy = Screencast::new().await.map_err(portal_unavailable)?;
    // A lightweight property read that only succeeds if the ScreenCast
    // interface is implemented by the running portal.
    proxy
        .available_source_types()
        .await
        .map_err(portal_unavailable)?;
    Ok(proxy)
}

/// Map a portal-construction/probe failure to [`RecorderError::Unsupported`],
/// logging the underlying cause (the public `Unsupported` variant carries no
/// payload). This is the "the ScreenCast portal genuinely isn't here" path,
/// distinct from a runtime `Platform` failure once the portal exists.
fn portal_unavailable(cause: ashpd::Error) -> RecorderError {
    eprintln!("screen-recorder(linux): ScreenCast portal unavailable: {cause}");
    RecorderError::Unsupported
}

// ---------------------------------------------------------------------------
// Public backend contract (mirrors macos.rs / web.rs).
// ---------------------------------------------------------------------------

/// The portal *is* the permission — the user grants capture by picking a source
/// in [`start`]'s dialog (like web's `getDisplayMedia`). So this doesn't pop the
/// dialog; it only verifies the ScreenCast portal is reachable and returns `Ok`,
/// deferring the actual grant to `start`. A genuinely absent portal maps to
/// [`RecorderError::Unsupported`].
pub(crate) async fn request_permission(_source: &Source) -> Result<(), RecorderError> {
    probe_portal().await.map(|_| ())
}

/// Negotiate a screencast through the portal (interactive dialog), open the
/// PipeWire remote, and start the GStreamer capture pipeline. Returns the RAII
/// [`Recording`] and `None` for the native source (no zero-copy surface on this
/// path — see the module note). Frames flow into `writer` as tightly-packed
/// `RGBA8` until the handle drops.
pub(crate) async fn start(
    config: RecordingConfig,
    writer: FrameWriter,
) -> Result<(Recording, Option<NativeSource>), RecorderError> {
    ensure_gst()?;

    // Layer exclusion is unavailable on the Linux portal — log rather than
    // silently drop the intent (a mounted `PrivateLayer` is captured inline).
    if matches!(config.source, Source::ThisApp) {
        eprintln!(
            "screen-recorder(linux): PrivateLayer capture-exclusion is unavailable on the \
             xdg-desktop-portal ScreenCast backend; any overlay is recorded inline."
        );
    }
    // A window title hint can't be honored programmatically — the user still
    // picks the window in the portal dialog.
    if let Source::Window(sel) = &config.source {
        if sel.title_hint.is_some() {
            eprintln!(
                "screen-recorder(linux): WindowSelector.title_hint is a no-op on the portal \
                 backend; the user selects the window in the share dialog."
            );
        }
    }
    // Desktop audio is not carried by the ScreenCast portal — capture video
    // only and say so rather than faking an audio track.
    if !matches!(config.audio, crate::AudioSource::None) {
        eprintln!(
            "screen-recorder(linux): desktop-audio capture is not yet wired on the portal \
             backend; capturing video only."
        );
    }

    // --- Portal handshake (ashpd) -----------------------------------------
    let proxy = probe_portal().await?;
    let session = proxy.create_session().await.map_err(map_ashpd_error)?;

    proxy
        .select_sources(
            &session,
            // Cursor embedded in the frames — the natural default for a
            // screen recording (metadata-only cursor needs a compositor path
            // we don't consume).
            CursorMode::Embedded,
            portal_source_types(&config.source),
            // Single source → single PipeWire stream → one pipeline.
            false,
            None,
            PersistMode::DoNot,
        )
        .await
        .map_err(map_ashpd_error)?;

    // `start` opens the interactive "pick what to share" dialog; the await
    // blocks until the user approves (or cancels → PermissionDenied).
    let streams: Streams = proxy
        .start(&session, &WindowIdentifier::default())
        .await
        .map_err(map_ashpd_error)?
        .response()
        .map_err(map_ashpd_error)?;

    let stream = streams
        .streams()
        .first()
        .ok_or_else(|| RecorderError::Platform("portal granted no PipeWire stream".into()))?;
    let node_id = stream.pipe_wire_node_id();
    // Dev-only trace of what the portal granted — the fastest way to confirm how
    // far a live negotiation got (node id + compositor size). Stripped from
    // release builds.
    #[cfg(debug_assertions)]
    eprintln!(
        "screen-recorder(linux): portal granted PipeWire node {node_id}, size {:?}",
        stream.size()
    );

    // The raw PipeWire fd for the granted session — `pipewiresrc` connects over
    // it. Kept alive in the `Recording` for the pipeline's whole lifetime.
    let fd: OwnedFd = proxy
        .open_pipe_wire_remote(&session)
        .await
        .map_err(map_ashpd_error)?;

    // --- GStreamer capture -------------------------------------------------
    // `pipewiresrc fd=<fd> path=<node_id>` subscribes to the granted node over
    // the portal fd. `path` is the node id as a string (see `gst-inspect-1.0
    // pipewiresrc`); `fd` is the raw descriptor.
    let source = gst::ElementFactory::make("pipewiresrc")
        .property("fd", fd.as_raw_fd())
        .property("path", node_id.to_string())
        .build()
        .map_err(|e| {
            RecorderError::Platform(format!(
                "missing GStreamer element `pipewiresrc` (install gst-plugin-pipewire): {e}"
            ))
        })?;

    let built = build_pipeline(source, &config, writer)?;
    let (pipeline, quit, bus_thread) = start_pipeline(built)?;

    Ok((
        Recording {
            pipeline,
            quit,
            bus_thread: Some(bus_thread),
            _fd: fd,
            _session: session,
            _proxy: proxy,
        },
        None,
    ))
}

// ---------------------------------------------------------------------------
// Private layer — documented no-op on Linux (no capture-exclusion mechanism).
// ---------------------------------------------------------------------------

/// Install the `PrivateLayer` external handler — a **no-op** on Linux.
///
/// The xdg-desktop-portal ScreenCast path has no per-window capture-exclusion
/// mechanism (no `NSWindowSharingNone` / `PixelCopy`-exclusion analogue), so an
/// overlay subtree cannot be hidden from the recording; [`start`] logs that a
/// mounted layer is captured inline.
///
/// v2: the `Backend` mega-trait is gone and payload handlers install on a
/// `runtime_scene::Registry`. The Linux recorder needs no scene handler —
/// it captures through the portal, not through a mounted node — so this
/// stays a no-op and is generic over the host so app boot code can call
/// `screen_recorder::register(&mut registry)` unconditionally.
pub fn register<H>(_registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_scene::Host + 'static,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioSource, PixelFormat, WindowSelector};
    use media_stream::{MediaStream, VideoFrame};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// **The core proof**: the real appsink map/deliver path produces a mapped,
    /// correctly-shaped `RGBA8` frame — exercised WITHOUT the portal by
    /// substituting `videotestsrc` for `pipewiresrc`. Everything downstream
    /// (`videoconvert ! videoscale ! videorate ! appsink(RGBA) ! new-sample !
    /// deliver_sample ! write_rgba8`) is byte-for-byte the production path. If
    /// this delivers a `64x48` `Rgba8` frame whose `data.len() ==
    /// width*height*4`, the capture backend genuinely works; it fails against
    /// the old `Unsupported` skeleton (which never wired a pipeline at all).
    #[test]
    fn videotestsrc_delivers_mapped_rgba_frame() {
        ensure_gst().expect("gst init");

        let (stream, writer) = MediaStream::new();
        let got: Arc<Mutex<Option<(u32, u32, usize)>>> = Arc::new(Mutex::new(None));
        let sub = {
            let got = got.clone();
            stream.subscribe(move |f: &VideoFrame| {
                if f.format == PixelFormat::Rgba8 && f.data.len() == f.byte_len() {
                    *got.lock().unwrap() = Some((f.width, f.height, f.data.len()));
                }
            })
        };

        let source = make("videotestsrc").expect("videotestsrc element");
        let config = RecordingConfig::new().size(64, 48);
        let built = build_pipeline(source, &config, writer).expect("synthetic pipeline builds");
        let (pipeline, quit, bus_thread) =
            start_pipeline(built).expect("synthetic pipeline must start");

        // Wait (polling) for the first frame to be mapped and fanned out.
        let deadline = Instant::now() + Duration::from_secs(5);
        let observed = loop {
            if let Some(v) = *got.lock().unwrap() {
                break Some(v);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        drop(sub);
        // RAII stop must not hang or panic (mirrors `Recording::drop`).
        quit.store(true, Ordering::Relaxed);
        let _ = pipeline.set_state(gst::State::Null);
        let _ = bus_thread.join();

        let (w, h, len) = observed.expect("a synthetic RGBA frame must be delivered");
        assert_eq!((w, h), (64, 48), "delivered frame carries the negotiated size");
        assert_eq!(len, 64 * 48 * 4, "delivered frame is tightly-packed RGBA8");
    }

    /// The `Source` → portal `SourceType` mapping must be honest: full-screen is
    /// monitor-only, a window source is window-only, and the app/user-choice
    /// sources offer both kinds (the portal has no "this app" concept).
    #[test]
    fn source_maps_to_portal_types() {
        assert_eq!(
            portal_source_types(&Source::FullScreen),
            BitFlags::from(SourceType::Monitor)
        );
        assert_eq!(
            portal_source_types(&Source::Window(WindowSelector { title_hint: None })),
            BitFlags::from(SourceType::Window)
        );
        let both: BitFlags<SourceType> = SourceType::Monitor | SourceType::Window;
        assert_eq!(portal_source_types(&Source::ThisApp), both);
        assert_eq!(portal_source_types(&Source::UserChoice), both);
    }

    /// Error mapping must be honest: a cancelled portal response (user dismissed
    /// the share dialog) is `PermissionDenied`; any other portal failure is
    /// `Platform`.
    #[test]
    fn cancelled_maps_to_permission_denied() {
        let cancelled =
            ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled);
        assert!(
            matches!(map_ashpd_error(cancelled), RecorderError::PermissionDenied),
            "a cancelled share dialog must map to PermissionDenied"
        );
    }

    /// A default config carries no audio, so `start` must not think audio is
    /// requested. Guards the audio-deferral branch's condition (video-only).
    #[test]
    fn default_config_requests_no_audio() {
        assert!(matches!(RecordingConfig::new().audio, AudioSource::None));
    }

    /// **Live capture through the real portal + PipeWire.** Ignored by default:
    /// the portal pops an interactive "share your screen" dialog that CANNOT be
    /// approved non-interactively — a human must click "Share" in the compositor
    /// prompt. Run on a real Wayland/X session with:
    ///
    /// ```sh
    /// cargo test -p screen-recorder --target x86_64-unknown-linux-gnu \
    ///     -- --ignored --nocapture live_portal_delivers_a_real_frame
    /// ```
    ///
    /// then approve the dialog. Proves the full `ashpd` negotiation +
    /// `pipewiresrc` capture path delivers real RGBA frames.
    #[tokio::test]
    #[ignore = "requires a human to approve the interactive xdg-desktop-portal share dialog"]
    async fn live_portal_delivers_a_real_frame() {
        // ashpd's zbus proxies (default async-io features) run on their own
        // reactor thread, so a plain current-thread tokio runtime drives them.
        let (stream, writer) = MediaStream::new();
        let arrived = Arc::new(Mutex::new(false));
        let sub = {
            let arrived = arrived.clone();
            stream.subscribe(move |f: &VideoFrame| {
                if f.data.len() == f.byte_len() {
                    *arrived.lock().unwrap() = true;
                }
            })
        };

        let (recording, _native) = start(RecordingConfig::new(), writer)
            .await
            .expect("live portal capture must start after the user approves the dialog");

        // A compositor may only emit frames on damage, so give a static screen
        // a generous window (move a window / wiggle the cursor to force frames).
        let deadline = Instant::now() + Duration::from_secs(10);
        while !*arrived.lock().unwrap() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let ok = *arrived.lock().unwrap();
        drop(sub);
        drop(recording); // RAII stop must not hang or panic.
        assert!(ok, "the live portal capture must deliver at least one RGBA frame");
    }
}

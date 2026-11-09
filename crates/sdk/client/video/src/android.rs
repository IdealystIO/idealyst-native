//! Android implementation of the Video SDK.
//!
//! Mounts a `FrameLayout` host. The source decides the child it hosts:
//!
//! - **URL** → an `android.widget.VideoView` child. `setVideoURI` takes an
//!   `android.net.Uri` (`Uri.parse(String)`); reactive URL changes flow
//!   through an `effect!`. `start`/`pause`/`seekTo` drive the ops.
//! - **Live `MediaStream` with a [`PreviewSource`]** (a camera) → a `TextureView`
//!   child whose `Surface` goes to the capture: Camera2 fills it, the GPU composites
//!   it, and no frame passes through this process. Nothing is polled per frame, and
//!   no CPU frames are made on this display's account.
//! - **Any other live `MediaStream`** → an `ImageView` child whose `Bitmap` is
//!   replaced each frame from the stream's tightly-packed RGBA8 frames. We poll the
//!   stream's latest frame on the framework's frame loop (which runs on the
//!   main/UI looper, so the view updates are UI-thread-safe) and hand the
//!   bytes to the Kotlin shim. The universal CPU path — works for ANY
//!   MediaStream (camera, screen, a compositor's output).
//!
//! A preview source can also land on that CPU path, holding a subscription so the
//! capture makes the frames: when the device refused the preview (for good), or while
//! another `Video` holds the source (one display at a time; this one takes over the
//! moment the other lets go, so a screen replacing another in one transition does
//! not stay on the slow path). See [`Showing`].
//!
//! The `FrameLayout` + child management lives in the `RustVideoFrameSink`
//! Kotlin shim (shipped via `[package.metadata.idealyst.android].runtime_kotlin`).
//!
//! ## VERIFICATION
//!
//! Compile-checked for `aarch64-linux-android`, with `javap` checking each JNI
//! descriptor against the compiled shim; **not yet device-verified** (the Kotlin path,
//! the `TextureView` and the bitmap upload run only on a device — same posture as
//! the camera SDK's Android backend). The transform math is host-tested
//! ([`crate::android_preview`]).

use crate::android_preview::{preview_transform, PreviewPlacement};
use crate::{FirstFrame, MediaContent, VideoOps, VideoProps};
// `backend-android-mobile`'s `[lib].name` is `backend_android`
// (preserved historically so `System.loadLibrary("backend_android")`
// keeps working).
use backend_android::{with_jni_env, AndroidBackend};
use jni::objects::{GlobalRef, JClass, JObject, JValue};
use jni::sys::{jboolean, jfloatArray, jint, jlong, JNI_FALSE};
use jni::JNIEnv;
use media_stream::{MediaStream, PreviewClaim, PreviewSource, Subscription};
use runtime_vocabulary::style_attach::on_teardown;
use runtime_world::effect;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

const SINK: &str = "io/idealyst/video/RustVideoFrameSink";

pub(crate) static OPS: &dyn VideoOps = &AndroidVideoOps;

runtime_tls::thread_local! {
    /// Preview id → the claim its `TextureView`'s surface attaches through. The
    /// shim's surface callbacks name an id and find what is still current here: a
    /// callback for a preview that has been replaced finds nothing. Main thread.
    static CLAIMS: RefCell<HashMap<u64, PreviewClaim>> = RefCell::new(HashMap::new());
    static NEXT_PREVIEW: Cell<u64> = const { Cell::new(1) };
    /// Id → the `Video`'s first-frame state, for the shim's first-frame callbacks:
    /// preview ids (a `TextureView`'s first update) and display ids (a `VideoView`'s
    /// rendering start) share the one counter above, so they never collide. Weak:
    /// the `Video` owns its state. Main thread.
    static FIRST_FRAMES: RefCell<HashMap<u64, Weak<FirstFrame>>> = RefCell::new(HashMap::new());
}

/// A fresh id from the shared preview/display counter.
fn next_id() -> u64 {
    NEXT_PREVIEW.with(|next| next.replace(next.get() + 1))
}

/// A shim callback: the view registered under `id` put its first frame on screen.
fn first_frame_shown(id: u64) {
    let first = FIRST_FRAMES.with(|m| m.borrow().get(&id).and_then(Weak::upgrade));
    if let Some(first) = first {
        first.frame_painted();
    }
}

/// What this `Video` is doing about its live stream.
enum Showing {
    /// No stream, or a URL.
    Nothing,
    /// The stream has no preview source: poll its CPU frames.
    Frames,
    /// This display holds `source`, and a `TextureView` shows it under this id.
    Preview { source: PreviewSource, id: u64 },
    /// `source` can't be shown natively (refused, or another display holds it), so
    /// the subscription makes the capture produce CPU frames for this display.
    PreviewFrames { source: PreviewSource, _demand: Subscription },
}

/// One `Video`'s stream display, advanced once per frame on the UI thread. Holds no
/// clone of the stream: a stream that stops being the source has to be able to drop,
/// which is what turns the camera off.
struct StreamDisplay {
    host: GlobalRef,
    cover: bool,
    showing: Showing,
    last_gen: u64,
    scratch: Vec<u8>,
    first: Rc<FirstFrame>,
}

impl StreamDisplay {
    fn new(host: GlobalRef, cover: bool, first: Rc<FirstFrame>) -> Self {
        Self {
            host,
            cover,
            showing: Showing::Nothing,
            last_gen: u64::MAX,
            scratch: Vec::new(),
            first,
        }
    }

    fn tick(&mut self, stream: Option<&MediaStream>) {
        let Some(stream) = stream else {
            self.clear();
            return;
        };
        let source = stream
            .native_source()
            .and_then(|native| native.downcast::<PreviewSource>().ok());
        match source {
            None => {
                if !matches!(self.showing, Showing::Frames) {
                    self.clear();
                    self.showing = Showing::Frames;
                }
                self.show_latest(stream);
            }
            Some(source) => {
                self.follow_preview(stream, &source);
            }
        }
    }

    /// Advance a stream with a preview source: only the transitions cost anything.
    fn follow_preview(&mut self, stream: &MediaStream, source: &PreviewSource) {
        let same = match &self.showing {
            Showing::Preview { source: held, .. } | Showing::PreviewFrames { source: held, .. } => {
                held.is_same_source(source)
            }
            _ => false,
        };
        if !same {
            self.clear();
        }
        match &self.showing {
            Showing::Preview { .. } => {
                if source.is_refused() {
                    self.clear();
                    self.show_preview_frames(stream, source);
                }
            }
            Showing::PreviewFrames { .. } => {
                if let Some(claim) = source.try_claim() {
                    self.clear();
                    self.show_preview(source, claim);
                } else {
                    self.show_latest(stream);
                }
            }
            _ => match source.try_claim() {
                Some(claim) => self.show_preview(source, claim),
                None => self.show_preview_frames(stream, source),
            },
        }
    }

    fn show_preview(&mut self, source: &PreviewSource, claim: PreviewClaim) {
        // Nothing reads CPU frames here now, so a frame's worth of memory goes.
        self.scratch = Vec::new();
        let id = next_id();
        CLAIMS.with(|claims| claims.borrow_mut().insert(id, claim));
        FIRST_FRAMES.with(|m| m.borrow_mut().insert(id, Rc::downgrade(&self.first)));
        let geometry = source.geometry();
        let host = &self.host;
        let cover = self.cover;
        with_jni_env(|env| {
            let shown = env.call_static_method(
                SINK,
                "showPreview",
                "(Landroid/widget/FrameLayout;JIIIZZ)V",
                &[
                    JValue::Object(host.as_obj()),
                    JValue::Long(id as jlong),
                    JValue::Int(geometry.width as jint),
                    JValue::Int(geometry.height as jint),
                    JValue::Int(geometry.sensor_orientation),
                    JValue::Bool(geometry.front_facing as jboolean),
                    JValue::Bool(cover as jboolean),
                ],
            );
            if shown.is_err() {
                // A shim that can't show this means no surface and no watchdog: refuse
                // here, or this display stays blank waiting for a frame.
                let _ = env.exception_clear();
                eprintln!("video: showPreview failed; using CPU frames");
                CLAIMS.with(|claims| {
                    if let Some(claim) = claims.borrow().get(&id) {
                        claim.refuse();
                    }
                });
            }
        });
        self.showing = Showing::Preview { source: source.clone(), id };
    }

    fn show_preview_frames(&mut self, stream: &MediaStream, source: &PreviewSource) {
        let demand = stream.subscribe(|_: &media_stream::VideoFrame| {});
        self.showing = Showing::PreviewFrames { source: source.clone(), _demand: demand };
        self.show_latest(stream);
    }

    /// The CPU path: copy a new frame, if there is one, to the `ImageView`.
    fn show_latest(&mut self, stream: &MediaStream) {
        let generation = stream.generation();
        if generation == self.last_gen {
            return;
        }
        self.last_gen = generation;
        if let Some((w, h)) = stream.latest(&mut self.scratch) {
            if show_frame(&self.host, &self.scratch, w, h, self.cover) {
                self.first.frame_painted();
            }
        }
    }

    /// Stop showing anything native. The claim goes first: it stops the capture naming
    /// the surface before removing the view releases it. The CPU path's last frame
    /// stays up, as it always has between streams.
    fn clear(&mut self) {
        if let Showing::Preview { id, .. } = &self.showing {
            FIRST_FRAMES.with(|m| m.borrow_mut().remove(id));
            let claim = CLAIMS.with(|claims| claims.borrow_mut().remove(id));
            drop(claim);
            let host = &self.host;
            with_jni_env(|env| {
                let hidden = env.call_static_method(
                    SINK,
                    "hidePreview",
                    "(Landroid/widget/FrameLayout;)V",
                    &[JValue::Object(host.as_obj())],
                );
                if hidden.is_err() {
                    let _ = env.exception_clear();
                }
            });
        }
        self.showing = Showing::Nothing;
        self.last_gen = u64::MAX;
    }
}

/// Build the native Android video host view for `props`. Called by the
/// `Registry<AndroidBackend>` mount handler in lib.rs.
pub(crate) fn build_video(props: &Rc<VideoProps>, b: &mut AndroidBackend) -> GlobalRef {
    // The host is a FrameLayout; the Kotlin shim adds a VideoView (URL) or
    // ImageView (stream) child as needed.
    let host = b.with_jni(|env, ctx| {
        let class = env
            .find_class("android/widget/FrameLayout")
            .expect("find_class android/widget/FrameLayout");
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(ctx.as_obj())],
            )
            .expect("new FrameLayout(Context)");
        backend_android_core::helpers::apply_default_layout_params(env, &local);
        env.new_global_ref(local).expect("new_global_ref")
    });

    // `on_first_frame`: one state per `Video`, reached from the shim's callbacks
    // by id — this display id for the `VideoView`, a preview id per `TextureView`.
    let first = FirstFrame::new(props);
    let display_id = next_id();
    FIRST_FRAMES.with(|m| m.borrow_mut().insert(display_id, Rc::downgrade(&first)));

    // URL path — reactive Effect (efficient; fires only on change).
    let autoplay = props.autoplay;
    // object-fit: Cover → fill the box (no black letterbox); Contain → aspect-fit.
    let url_fill = matches!(props.object_fit, crate::ObjectFit::Cover);
    let host_for_url = host.clone();
    let props_for_url = props.clone();
    let first_run = std::cell::Cell::new(true);
    effect(move || {
        let url = match props_for_url.source.resolve() {
            MediaContent::Url(u) => u,
            _ => return,
        };
        set_video_uri(&host_for_url, &url, url_fill, display_id);
        if first_run.replace(false) && autoplay {
            start(&host_for_url);
        }
    });

    // Stream path, on the UI looper. Only a stream on the CPU path does any work per
    // frame. object-fit: Cover → fill and crop, Contain → letterbox. Static.
    let cover = matches!(props.object_fit, crate::ObjectFit::Cover);
    let display = Rc::new(RefCell::new(StreamDisplay::new(host.clone(), cover, first.clone())));
    let props_for_stream = props.clone();
    let display_for_frames = display.clone();
    runtime_vocabulary::scoped_scheduling::raf_loop_scoped(move || {
        let content = props_for_stream.source.resolve();
        // Before `tick`, so a frame shown for a new source counts as its first.
        first.track(&content);
        let stream = match &content {
            MediaContent::Stream(stream) => Some(stream),
            _ => None,
        };
        display_for_frames.borrow_mut().tick(stream);
    });
    // The loop is gone with the scope, so the unmount has to let the source go itself:
    // another display may be waiting for it, and the capture must stop naming a surface
    // that is about to be released.
    on_teardown(move || {
        display.borrow_mut().clear();
        FIRST_FRAMES.with(|m| m.borrow_mut().remove(&display_id));
    });

    host
}

// ============================================================================
// The shim's preview callbacks.
// ============================================================================

/// `RustVideoFrameSink.nativePreviewSurfaceAvailable` — the `TextureView`'s surface
/// exists, sized to the preview. Hand it to the capture.
///
/// # Safety
/// Called by the JVM on the main thread with a valid `surface`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_video_RustVideoFrameSink_nativePreviewSurfaceAvailable(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
    surface: JObject,
) {
    trampoline("nativePreviewSurfaceAvailable", || {
        CLAIMS.with(|claims| {
            if let Some(claim) = claims.borrow().get(&(id as u64)) {
                // SAFETY: a live `Surface` local reference, for this call on this
                // thread; the driver keeps its own reference.
                unsafe { claim.attach(surface.as_raw().cast()) };
            }
        });
    });
}

/// `RustVideoFrameSink.nativePreviewSurfaceDestroyed` — the surface is going away. Keep
/// the claim: the same view can get a new one if it is re-attached to a window.
///
/// # Safety
/// Called by the JVM on the main thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_video_RustVideoFrameSink_nativePreviewSurfaceDestroyed(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
) {
    trampoline("nativePreviewSurfaceDestroyed", || {
        CLAIMS.with(|claims| {
            if let Some(claim) = claims.borrow().get(&(id as u64)) {
                claim.detach();
            }
        });
    });
}

/// `RustVideoFrameSink.nativePreviewStalled` — on screen with no frame after a long
/// wait. Refusing the source moves this display to CPU frames on its next tick.
///
/// # Safety
/// Called by the JVM on the main thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_video_RustVideoFrameSink_nativePreviewStalled(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
) {
    trampoline("nativePreviewStalled", || {
        CLAIMS.with(|claims| {
            if let Some(claim) = claims.borrow().get(&(id as u64)) {
                claim.refuse();
            }
        });
    });
}

/// `RustVideoFrameSink.nativePreviewFirstFrame` — the preview's `TextureView` drew its
/// first camera frame.
///
/// # Safety
/// Called by the JVM on the main thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_video_RustVideoFrameSink_nativePreviewFirstFrame(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
) {
    trampoline("nativePreviewFirstFrame", || first_frame_shown(id as u64));
}

/// `RustVideoFrameSink.nativeVideoRenderingStart` — the `VideoView` of the display
/// registered under `id` rendered its first frame (`MEDIA_INFO_VIDEO_RENDERING_START`).
///
/// # Safety
/// Called by the JVM on the main thread.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_video_RustVideoFrameSink_nativeVideoRenderingStart(
    _env: JNIEnv,
    _class: JClass,
    id: jlong,
) {
    trampoline("nativeVideoRenderingStart", || first_frame_shown(id as u64));
}

/// `RustVideoFrameSink.nativePreviewTransform` — `[scale_x, scale_y, rotation_degrees]`
/// for a preview, from the host-tested [`preview_transform`]. Called on each change of
/// view size or display rotation, never per frame. Null if the array can't be made.
///
/// # Safety
/// Called by the JVM with a valid `env`/`class`.
#[no_mangle]
pub extern "system" fn Java_io_idealyst_video_RustVideoFrameSink_nativePreviewTransform(
    env: JNIEnv,
    _class: JClass,
    buffer_width: jint,
    buffer_height: jint,
    sensor_orientation: jint,
    front: jboolean,
    display_rotation: jint,
    view_width: jint,
    view_height: jint,
    cover: jboolean,
) -> jfloatArray {
    let mut result = std::ptr::null_mut();
    trampoline("nativePreviewTransform", || {
        let t = preview_transform(PreviewPlacement {
            buffer: (buffer_width as f32, buffer_height as f32),
            sensor_orientation,
            front: front != JNI_FALSE,
            display_rotation,
            view: (view_width as f32, view_height as f32),
            cover: cover != JNI_FALSE,
        });
        if let Ok(array) = env.new_float_array(3) {
            if env
                .set_float_array_region(&array, 0, &[t.scale_x, t.scale_y, t.rotation_degrees])
                .is_ok()
            {
                result = array.into_raw();
            }
        }
    });
    result
}

/// A panic must not cross into the JVM.
fn trampoline(name: &str, body: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).is_err() {
        eprintln!("video: panic in {name} trampoline; aborting");
        std::process::abort();
    }
}

// Pin the exports so the linker keeps them in the app `cdylib`'s dynamic symbol table
// (the JVM resolves them by `dlsym`).
#[used]
static KEEP_SURFACE_AVAILABLE: extern "system" fn(JNIEnv, JClass, jlong, JObject) =
    Java_io_idealyst_video_RustVideoFrameSink_nativePreviewSurfaceAvailable;
#[used]
static KEEP_SURFACE_DESTROYED: extern "system" fn(JNIEnv, JClass, jlong) =
    Java_io_idealyst_video_RustVideoFrameSink_nativePreviewSurfaceDestroyed;
#[used]
static KEEP_STALLED: extern "system" fn(JNIEnv, JClass, jlong) =
    Java_io_idealyst_video_RustVideoFrameSink_nativePreviewStalled;
#[used]
static KEEP_PREVIEW_FIRST_FRAME: extern "system" fn(JNIEnv, JClass, jlong) =
    Java_io_idealyst_video_RustVideoFrameSink_nativePreviewFirstFrame;
#[used]
static KEEP_VIDEO_RENDERING_START: extern "system" fn(JNIEnv, JClass, jlong) =
    Java_io_idealyst_video_RustVideoFrameSink_nativeVideoRenderingStart;
#[used]
static KEEP_TRANSFORM: extern "system" fn(
    JNIEnv,
    JClass,
    jint,
    jint,
    jint,
    jboolean,
    jint,
    jint,
    jint,
    jboolean,
) -> jfloatArray = Java_io_idealyst_video_RustVideoFrameSink_nativePreviewTransform;

/// Push one RGBA8 frame to the host's stream ImageView (Kotlin makes the
/// Bitmap + setImageBitmap). Called on the UI thread from the frame loop. `true`
/// when the shim took the frame.
fn show_frame(host: &GlobalRef, rgba: &[u8], width: u32, height: u32, cover: bool) -> bool {
    let mut shown = false;
    with_jni_env(|env| {
        let needed = (width as usize) * (height as usize) * 4;
        if width == 0 || height == 0 || rgba.len() < needed {
            return;
        }
        // Hand Kotlin a direct ByteBuffer that VIEWS our RGBA slice — no
        // `byte[]` allocation and no Rust→JVM copy (the old
        // `byte_array_from_slice` did both, ~8 MB/frame at 1080p). Kotlin
        // copies straight into the Bitmap (`copyPixelsFromBuffer`)
        // synchronously, before this call returns.
        //
        // SAFETY: `rgba` outlives this synchronous `showFrame` call; the
        // direct buffer only aliases the slice for the call's duration, and
        // Kotlin reads (never retains) it.
        let buf = match unsafe { env.new_direct_byte_buffer(rgba.as_ptr() as *mut u8, needed) } {
            Ok(b) => b,
            Err(_) => return,
        };
        shown = env
            .call_static_method(
                SINK,
                "showFrame",
                "(Landroid/widget/FrameLayout;Ljava/nio/ByteBuffer;IIZ)V",
                &[
                    JValue::Object(host.as_obj()),
                    JValue::Object(&buf),
                    JValue::Int(width as i32),
                    JValue::Int(height as i32),
                    JValue::Bool(cover as u8),
                ],
            )
            .is_ok();
        if !shown {
            let _ = env.exception_clear();
        }
    });
    shown
}

/// Ensure the host has a VideoView child and point it at `src`. `fill` selects
/// object-fit cover (stretch to fill) vs the stock aspect-fit (Contain); the view
/// reports its rendering start under `display_id`.
fn set_video_uri(host: &GlobalRef, src: &str, fill: bool, display_id: u64) {
    with_jni_env(|env| {
        let Some(video_view) = ensure_video_view(env, host, fill, display_id) else {
            return;
        };
        let Ok(uri_class) = env.find_class("android/net/Uri") else {
            return;
        };
        let Ok(java_src) = env.new_string(src) else {
            return;
        };
        let Ok(call) = env.call_static_method(
            &uri_class,
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&JObject::from(java_src))],
        ) else {
            return;
        };
        let Ok(uri) = call.l() else { return };
        let _ = env.call_method(
            &video_view,
            "setVideoURI",
            "(Landroid/net/Uri;)V",
            &[JValue::Object(&uri)],
        );
    });
}

/// `RustVideoFrameSink.ensureVideoView(host, fill, id)` → the (created-if-needed)
/// VideoView child, reporting its rendering start under `display_id`.
fn ensure_video_view<'a>(
    env: &mut jni::JNIEnv<'a>,
    host: &GlobalRef,
    fill: bool,
    display_id: u64,
) -> Option<JObject<'a>> {
    env.call_static_method(
        SINK,
        "ensureVideoView",
        "(Landroid/widget/FrameLayout;ZJ)Landroid/widget/VideoView;",
        &[
            JValue::Object(host.as_obj()),
            JValue::Bool(fill as u8),
            JValue::Long(display_id as jlong),
        ],
    )
    .ok()?
    .l()
    .ok()
}

/// `RustVideoFrameSink.videoView(host)` → the existing VideoView child (or
/// `None`), for imperative ops that shouldn't create one.
fn existing_video_view<'a>(env: &mut jni::JNIEnv<'a>, host: &GlobalRef) -> Option<JObject<'a>> {
    let view = env
        .call_static_method(
            SINK,
            "videoView",
            "(Landroid/widget/FrameLayout;)Landroid/widget/VideoView;",
            &[JValue::Object(host.as_obj())],
        )
        .ok()?
        .l()
        .ok()?;
    (!view.is_null()).then_some(view)
}

fn start(host: &GlobalRef) {
    with_jni_env(|env| {
        if let Some(vv) = existing_video_view(env, host) {
            let _ = env.call_method(&vv, "start", "()V", &[]);
        }
    });
}

// ============================================================================
// Imperative ops — route to the host's VideoView child.
// ============================================================================

struct AndroidVideoOps;

impl VideoOps for AndroidVideoOps {
    fn play(&self, node: &dyn Any) {
        let Some(host) = node.downcast_ref::<GlobalRef>() else { return };
        start(host);
    }

    fn pause(&self, node: &dyn Any) {
        let Some(host) = node.downcast_ref::<GlobalRef>() else { return };
        with_jni_env(|env| {
            if let Some(vv) = existing_video_view(env, host) {
                let _ = env.call_method(&vv, "pause", "()V", &[]);
            }
        });
    }

    fn seek(&self, node: &dyn Any, seconds: f32) {
        let Some(host) = node.downcast_ref::<GlobalRef>() else { return };
        with_jni_env(|env| {
            if let Some(vv) = existing_video_view(env, host) {
                // VideoView.seekTo(int milliseconds).
                let _ = env.call_method(
                    &vv,
                    "seekTo",
                    "(I)V",
                    &[JValue::Int((seconds * 1000.0) as i32)],
                );
            }
        });
    }

    fn position(&self, node: &dyn Any) -> f32 {
        let Some(host) = node.downcast_ref::<GlobalRef>() else { return 0.0 };
        with_jni_env(|env| {
            existing_video_view(env, host)
                .and_then(|vv| env.call_method(&vv, "getCurrentPosition", "()I", &[]).ok())
                .and_then(|v| v.i().ok())
                .map(|ms| ms as f32 / 1000.0)
                .unwrap_or(0.0)
        })
    }

    fn duration(&self, node: &dyn Any) -> f32 {
        let Some(host) = node.downcast_ref::<GlobalRef>() else { return 0.0 };
        with_jni_env(|env| {
            existing_video_view(env, host)
                .and_then(|vv| env.call_method(&vv, "getDuration", "()I", &[]).ok())
                .and_then(|v| v.i().ok())
                // VideoView returns -1 until prepared.
                .map(|ms| if ms > 0 { ms as f32 / 1000.0 } else { 0.0 })
                .unwrap_or(0.0)
        })
    }
}

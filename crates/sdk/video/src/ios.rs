//! iOS implementation of the Video SDK.
//!
//! Hosts an `AVPlayerLayer` inside a plain `UIView`. Rather than
//! subclass UIView, we mount a stock UIView, build an `AVPlayer` from
//! the URL, wrap it in an `AVPlayerLayer`, and add the player layer
//! as a CALayer sublayer of the view's root layer.
//!
//! Layout: AVPlayerLayer's frame doesn't auto-track the host UIView's
//! bounds in practice (CALayer's `autoresizingMask` is documented but
//! unreliable here — video renders as a tiny tile in the top-left if
//! the layer is left at its construction-time CGRectZero). We sync the
//! layer's frame from the host view's bounds inside an `Effect::new`
//! callback that re-runs every reactive frame; the framework's
//! scheduler ticks Effects per frame, so the layer follows the view
//! within one frame of any resize.
//!
//! AVFoundation is accessed through `objc2`'s raw `msg_send!` /
//! `msg_send_id!` macros plus `class!(...)`. We deliberately don't
//! pull in `objc2-av-foundation` — its 0.3.x line requires a newer
//! `objc2` major than `backend-ios-mobile`'s pinned `objc2 = "0.5"`,
//! and the handful of selectors we need are trivial to send by hand.
//!
//! Loop playback is implemented via `NSNotificationCenter` listening
//! for `AVPlayerItemDidPlayToEndTimeNotification`; on receipt we seek
//! the player back to zero and resume.
//!
//! Per-mount state lifetime: the AVPlayer, AVPlayerLayer, and the
//! optional notification observer are retained in a thread-local
//! side-table keyed by the host view's pointer. Entries are never
//! removed — the same leak pattern the framework's prior built-in
//! Video impl used. SDK v2 can swap this for an Obj-C associated
//! object attached to the view (releases on dealloc); v1 keeps the
//! simpler model so the surface ships now.

use crate::{MediaContent, VideoOps, VideoProps};
// `backend-ios-mobile`'s `[lib].name` is `backend_ios` — historical
// staticlib filename preserved across the package rename.
use backend_ios::{IosBackend, IosNode};
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{CGRect, NSObject, NSString};
use objc2_ui_kit::UIView;
use runtime_core::effect;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::Rc;

pub(crate) static OPS: &dyn VideoOps = &IosVideoOps;

// AVFoundation is a system framework that ships with iOS; force the
// linker to pull it in regardless of whether some other dependency
// has already requested it. Without this directive, building this
// crate as a standalone staticlib would succeed but the final link
// step would fail looking for AVPlayer/AVPlayerLayer.
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

// For the live-stream zero-copy path: enqueue capture `CMSampleBuffer`s into an
// `AVSampleBufferDisplayLayer`. A live preview has no media timebase, so each
// buffer is tagged `kCMSampleAttachmentKey_DisplayImmediately` to render the
// instant it's enqueued (otherwise the layer waits on a timebase that never
// advances and shows nothing). These are the few C symbols that requires.
#[allow(non_upper_case_globals)]
#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    /// The per-sample attachments array (a `CFArray` of `CFMutableDictionary`),
    /// created if absent when the second arg is non-zero.
    fn CMSampleBufferGetSampleAttachmentsArray(
        sbuf: *const std::ffi::c_void,
        create_if_necessary: u8,
    ) -> *const std::ffi::c_void;
    static kCMSampleAttachmentKey_DisplayImmediately: *const std::ffi::c_void;
}

#[allow(non_upper_case_globals)]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetValueAtIndex(arr: *const std::ffi::c_void, idx: isize) -> *const std::ffi::c_void;
    fn CFDictionarySetValue(
        dict: *const std::ffi::c_void,
        key: *const std::ffi::c_void,
        value: *const std::ffi::c_void,
    );
    static kCFBooleanTrue: *const std::ffi::c_void;
}

use media_stream::SurfaceSource;

// CoreMedia `CMTime` mirror. objc2 doesn't re-export the CoreMedia
// types and we deliberately don't pull in `core-media-sys` for the
// sake of two struct layouts. The shape MUST match CoreMedia's
// definition (`<CoreMedia/CMTime.h>`) exactly — same field order,
// same widths, same `repr(C)` layout — or the AVPlayer call will
// read garbage and seek to a nonsense offset.
#[repr(C)]
#[derive(Copy, Clone)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// `Encode` is what objc2 needs to pass CMTime into `msg_send!` as a
// by-value argument. Encoding mirrors `@encode(CMTime)` on Apple's
// compiler: `{?=qiIq}` for the anonymous-struct layout
// (value:q timescale:i flags:I epoch:q).
unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[
            Encoding::LongLong,
            Encoding::Int,
            Encoding::UInt,
            Encoding::LongLong,
        ],
    );
}
unsafe impl RefEncode for CMTime {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

const CM_TIME_FLAG_VALID: u32 = 1;

// CoreGraphics RGBA8 → CGImage bridge (build a CGImage from a
// MediaStream's frame bytes to push into a CALayer's `contents`) is shared
// with the macOS backend — pure CoreGraphics, identical on both platforms.
// See `crate::cg_image`.
use crate::cg_image::{cgimage_from_rgba, CGImageRelease};

/// Per-video state retained in the side-table. `loop_observer` is held
/// purely for its retention side-effect — releasing it removes the
/// notification observer.
struct VideoEntry {
    player: Retained<NSObject>,
    #[allow(dead_code)]
    player_layer: Retained<NSObject>,
    #[allow(dead_code)]
    loop_observer: Option<Retained<NSObject>>,
}

/// Register the Video handler against an `IosBackend`. One-line call from
/// the app's bootstrap.
pub fn register(backend: &mut IosBackend) {
    backend.register_external::<VideoProps, _>(|props, b| build_video(props, b));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_ios::IosExternalRegistrar(register)
}

/// Resolve a source to a URL for the AVPlayer path. A live `Stream` source
/// has no native iOS player binding yet (that's the GPU/compositing phase);
/// it resolves to no URL, leaving the player idle.
fn resolved_url(props: &VideoProps) -> Option<String> {
    match props.source.resolve() {
        MediaContent::Url(u) => Some(u),
        MediaContent::Stream(_) | MediaContent::None => None,
    }
}

fn build_video(props: &Rc<VideoProps>, b: &mut IosBackend) -> IosNode {
    // Plain UIView — no subclass needed. UIView's CALayer hosts the
    // AVPlayerLayer; Taffy drives the UIView's bounds via the regular
    // apply_frames path.
    let view: Retained<UIView> = unsafe {
        msg_send_id![
            msg_send_id![objc2::class!(UIView), alloc],
            initWithFrame: CGRect::default()
        ]
    };

    let initial_src = resolved_url(props).unwrap_or_default();
    let player = build_player(&initial_src);
    let player_layer = build_player_layer(&player, props.object_fit);

    // Add the AVPlayerLayer as a sublayer of the view's root layer.
    let view_layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
    let _: () = unsafe { msg_send![&view_layer, addSublayer: &*player_layer] };

    // AVSampleBufferDisplayLayer for the live-stream (camera / screen-recorder)
    // path: capture `CMSampleBuffer`s are enqueued straight into it — zero CPU
    // copy, no swizzle, no CGImage. A sibling sublayer of the AVPlayerLayer; a
    // given video element is either a URL (player) or a Stream (this layer),
    // never both, so they don't overlap — the unused one stays empty.
    let display_layer: Retained<NSObject> = unsafe {
        msg_send_id![
            msg_send_id![objc2::class!(AVSampleBufferDisplayLayer), alloc],
            init
        ]
    };
    let sbdl_gravity = NSString::from_str(av_layer_gravity(props.object_fit));
    let _: () = unsafe { msg_send![&display_layer, setVideoGravity: &*sbdl_gravity] };
    let _: () = unsafe { msg_send![&view_layer, addSublayer: &*display_layer] };
    // The stream layer is the TOP sublayer (over the AVPlayerLayer). An empty
    // AVSampleBufferDisplayLayer is opaque, so it must start hidden — otherwise it
    // covers the player layer and a URL (recorded-file) preview renders blank. The
    // per-frame raf below unhides it only for `Stream` sources.
    let _: () = unsafe { msg_send![&display_layer, setHidden: true] };

    // Honor the explicit `muted` flag (NOT tied to autoplay — iOS plays
    // unmuted autoplay fine, unlike the web). A recording preview wants its
    // audio; a silent background loop sets `muted: true`.
    let _: () = unsafe { msg_send![&player, setMuted: props.muted] };
    if props.autoplay {
        let _: () = unsafe { msg_send![&player, play] };
    }

    let loop_observer = if props.loop_playback {
        Some(install_loop_observer(&player))
    } else {
        None
    };

    // Register the player layer in the side-table keyed by the host
    // view's pointer. `IosVideoOps::play/pause/seek` look the player up
    // by this key when the user calls the imperative ops via the
    // VideoHandle.
    let key = &*view as *const UIView as usize;
    PLAYER_TABLE.with(|t| {
        t.borrow_mut().insert(
            key,
            VideoEntry {
                player: player.clone(),
                player_layer: player_layer.clone(),
                loop_observer,
            },
        );
    });

    // Tear the player down on unmount (the preview pops, by any route). Without
    // this the AVPlayer keeps playing — audible in the background — and the
    // PLAYER_TABLE entry + its loop observer leak. `on_cleanup` fires on scope
    // drop. Pause halts the rate; nil-ing the item releases the audio/decode
    // pipeline; removing the notification observer is required (the center
    // retains the token, and its block retains the player). Mirrors macOS.
    runtime_core::on_cleanup(move || {
        let Some(entry) = PLAYER_TABLE.with(|t| t.borrow_mut().remove(&key)) else {
            return;
        };
        unsafe {
            let _: () = msg_send![&*entry.player, pause];
            let nil_item: *const AnyObject = std::ptr::null();
            let _: () = msg_send![&*entry.player, replaceCurrentItemWithPlayerItem: nil_item];
            if let Some(obs) = &entry.loop_observer {
                let center: Retained<NSObject> =
                    msg_send_id![objc2::class!(NSNotificationCenter), defaultCenter];
                let _: () = msg_send![&center, removeObserver: &**obs];
            }
        }
    });

    b.register_external_view(&view);

    // Layer-frame sync happens in the per-frame raf below (see "Size both
    // sublayers" there). AVPlayerLayer doesn't auto-track its parent's bounds,
    // and a reactive `Effect` reading `bounds` imperatively tracks no signal —
    // it fires ONCE at build (0×0) and never again, leaving the layer 0×0 and
    // invisible. The raf re-syncs every frame, robust to layout + rotation.
    let player_layer_for_raf = player_layer.clone();

    // Reactive src. Load a new item whenever the resolved URL CHANGES to a
    // non-empty value, tracking the last-loaded URL. (A `first_run`-skip flag is
    // wrong: the URL often resolves asynchronously — a recorded-file path lands
    // AFTER mount — so the effect's first run already sees the real URL and would
    // skip it, never loading anything. `replaceCurrentItemWithPlayerItem:`
    // preserves the AVPlayerLayer + sublayer attachment, so swaps don't disrupt
    // layout.) `last_url` starts as whatever the player was built with, so an
    // unchanged URL doesn't double-load.
    let player_for_src = player.clone();
    let props_clone = props.clone();
    let last_url = RefCell::new(initial_src.clone());
    effect!({
        let url = resolved_url(&props_clone).unwrap_or_default();
        if url.is_empty() || url == *last_url.borrow() {
            return;
        }
        if let Some(item) = build_player_item(&url) {
            let _: () = unsafe {
                msg_send![&player_for_src, replaceCurrentItemWithPlayerItem: &*item]
            };
            *last_url.borrow_mut() = url;
        }
    });

    // Live MediaStream display. A `Stream` source carries no AVPlayer URL, so
    // we poll the stream's latest frame on the main thread (via the
    // scope-tied frame loop) and push it to the view's CALayer `contents` as
    // a CGImage. This is the universal CPU path — it works for ANY
    // MediaStream (camera, screen, a compositor's output). Performant native
    // paths (AVSampleBufferDisplayLayer / a Metal texture) are the GPU phase;
    // this renders correctly and stays simple.
    {
        let view_layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
        // Aspect-preserving fill, never the default stretch — drives the CPU
        // CGImage fallback's root layer (contain → letterbox, cover → crop).
        let gravity = NSString::from_str(contents_gravity(props.object_fit));
        let _: () = unsafe { msg_send![&view_layer, setContentsGravity: &*gravity] };

        let view_for_stream = view.clone();
        let display_layer_for_stream = display_layer.clone();
        let props_for_stream = props.clone();
        let mut last_gen: u64 = u64::MAX;
        let mut last_native_gen: u64 = u64::MAX;
        let mut scratch: Vec<u8> = Vec::new();
        runtime_core::raf_loop_scoped(move || {
            let source = props_for_stream.source.resolve();
            let is_stream = matches!(source, MediaContent::Stream(_));
            // Size both sublayers to the host view's bounds EVERY frame, and show
            // the stream layer ONLY for `Stream` sources. The two layers don't
            // auto-track the host UIView's bounds (a freshly-added sublayer stays
            // at its 0×0 construction frame and is INVISIBLE), so we sync per
            // frame. The stream layer is opaque and sits OVER the AVPlayerLayer, so
            // for a URL source it must be hidden or it blanks the video. Both fixes
            // are why the recorded-file preview rendered dark before. CATransaction
            // with actions disabled so the resize doesn't implicitly animate.
            unsafe {
                let bounds: CGRect = msg_send![&*view_for_stream, bounds];
                let txn = objc2::class!(CATransaction);
                let _: () = msg_send![txn, begin];
                let _: () = msg_send![txn, setDisableActions: true];
                let _: () = msg_send![&*player_layer_for_raf, setFrame: bounds];
                let _: () = msg_send![&*display_layer_for_stream, setFrame: bounds];
                let _: () = msg_send![&*display_layer_for_stream, setHidden: !is_stream];
                let _: () = msg_send![txn, commit];
            }

            let MediaContent::Stream(stream) = source else {
                return;
            };

            // Zero-copy fast-path. If the producer published a native handle (a
            // capture `CMSampleBuffer`), enqueue it straight into the
            // AVSampleBufferDisplayLayer: no swizzle, no CGImage, no per-frame
            // upload — the GPU decodes/displays the buffer directly. This is the
            // iOS analogue of the macOS IOSurface→CALayer.contents path.
            if let Some(native) = stream.native_source() {
                if let Some(surf) = native.downcast_ref::<SurfaceSource>() {
                    unsafe {
                        // (The display layer is already sized to the view at the
                        // top of this raf, for all source kinds.)

                        // A failed layer ignores all enqueues until flushed
                        // (e.g. after a transient decode error) — recover before
                        // the readiness check, since flush restores readiness.
                        const STATUS_FAILED: isize = 2;
                        let status: isize =
                            msg_send![&*display_layer_for_stream, status];
                        if status == STATUS_FAILED {
                            let _: () = msg_send![&*display_layer_for_stream, flush];
                        }

                        let generation = surf.generation();
                        if generation == last_native_gen {
                            return;
                        }
                        // Don't enqueue when the layer can't accept media yet —
                        // it would be silently dropped. Retry next tick WITHOUT
                        // consuming the generation.
                        let ready: bool =
                            msg_send![&*display_layer_for_stream, isReadyForMoreMediaData];
                        if !ready {
                            return;
                        }
                        last_native_gen = generation;

                        // `acquire` adds a retain so a concurrent capture-queue
                        // `publish` can't free the buffer before we enqueue it.
                        let sbuf = surf.acquire();
                        if sbuf.is_null() {
                            return;
                        }
                        // Tag DisplayImmediately so the live frame renders now
                        // (no media timebase on a preview).
                        let attachments =
                            CMSampleBufferGetSampleAttachmentsArray(sbuf, 1u8);
                        if !attachments.is_null() {
                            let dict = CFArrayGetValueAtIndex(attachments, 0);
                            if !dict.is_null() {
                                CFDictionarySetValue(
                                    dict,
                                    kCMSampleAttachmentKey_DisplayImmediately,
                                    kCFBooleanTrue,
                                );
                            }
                        }
                        let _: () = msg_send![
                            &*display_layer_for_stream,
                            enqueueSampleBuffer: sbuf as *const AnyObject
                        ];
                        // The layer retains the buffer; drop our acquire-retain.
                        surf.release(sbuf);
                    }
                    return;
                }
            }

            // CPU fallback (a stream with no native source): copy the latest
            // frame into a CGImage and push it to the root layer's contents.
            let generation = stream.generation();
            if generation == last_gen {
                return;
            }
            last_gen = generation;
            let Some((w, h)) = stream.latest(&mut scratch) else {
                return;
            };
            // Move the pixels out; the CGImage's provider takes ownership.
            let pixels = std::mem::take(&mut scratch);
            let image = unsafe { cgimage_from_rgba(pixels, w as usize, h as usize) };
            if image.is_null() {
                return;
            }
            unsafe {
                let layer: Retained<NSObject> = msg_send_id![&view_for_stream, layer];
                let _: () = msg_send![&layer, setContents: image as *const AnyObject];
                CGImageRelease(image); // the layer holds its own reference
            }
        });
    }

    IosNode::View(view)
}

// =============================================================================
// Helpers (private)
// =============================================================================

fn build_nsurl(s: &str) -> Option<Retained<NSObject>> {
    // A `file://` path MUST go through `fileURLWithPath:`, not `URLWithString:`.
    // `URLWithString:` parses an already-percent-encoded URL and returns nil on a
    // raw path containing a space or other reserved character — and the canonical
    // recordings store lives under "Application Support" (a space), so a
    // `file://…/Application Support/…recording.mp4` string yields nil, the
    // AVPlayer gets no item, and the preview renders blank. `fileURLWithPath:`
    // percent-encodes the path itself, so file playback is robust to any path.
    if let Some(path) = s.strip_prefix("file://") {
        let ns_path = NSString::from_str(path);
        unsafe { msg_send_id![objc2::class!(NSURL), fileURLWithPath: &*ns_path] }
    } else {
        let ns_str = NSString::from_str(s);
        unsafe { msg_send_id![objc2::class!(NSURL), URLWithString: &*ns_str] }
    }
}

fn build_player_item(src: &str) -> Option<Retained<NSObject>> {
    let url = build_nsurl(src)?;
    let item: Retained<NSObject> = unsafe {
        msg_send_id![
            msg_send_id![objc2::class!(AVPlayerItem), alloc],
            initWithURL: &*url
        ]
    };
    Some(item)
}

fn build_player(src: &str) -> Retained<NSObject> {
    // Empty URL → construct a player with no item; src can be populated
    // later via the reactive src Effect. Without this guard,
    // `URLWithString:""` returns nil and `playerWithURL:nil` crashes
    // AVPlayer's designated initializer.
    let url = build_nsurl(src);
    match url {
        Some(url) => unsafe {
            msg_send_id![objc2::class!(AVPlayer), playerWithURL: &*url]
        },
        None => unsafe { msg_send_id![objc2::class!(AVPlayer), new] },
    }
}

fn build_player_layer(
    player: &Retained<NSObject>,
    object_fit: crate::ObjectFit,
) -> Retained<NSObject> {
    let layer: Retained<NSObject> = unsafe {
        msg_send_id![objc2::class!(AVPlayerLayer), playerLayerWithPlayer: &**player]
    };
    // Aspect-preserving: ResizeAspect (contain) vs ResizeAspectFill (cover).
    let gravity = NSString::from_str(av_layer_gravity(object_fit));
    let _: () = unsafe { msg_send![&layer, setVideoGravity: &*gravity] };
    layer
}

/// `AVLayerVideoGravity*` string for an [`crate::ObjectFit`] — used by the
/// AVPlayerLayer (URL) and AVSampleBufferDisplayLayer (stream) paths.
fn av_layer_gravity(fit: crate::ObjectFit) -> &'static str {
    match fit {
        crate::ObjectFit::Cover => "AVLayerVideoGravityResizeAspectFill",
        crate::ObjectFit::Contain => "AVLayerVideoGravityResizeAspect",
    }
}

/// `CALayer.contentsGravity` string for an [`crate::ObjectFit`] — used by the
/// CPU CGImage fallback's root layer.
fn contents_gravity(fit: crate::ObjectFit) -> &'static str {
    match fit {
        crate::ObjectFit::Cover => "resizeAspectFill",
        crate::ObjectFit::Contain => "resizeAspect",
    }
}

/// Register an NSNotificationCenter observer for
/// `AVPlayerItemDidPlayToEndTimeNotification` against any AVPlayerItem
/// this player is currently using; when the notification fires we seek
/// back to zero and resume. Returns the opaque observer token (retained
/// so it stays alive for the lifetime of the video).
fn install_loop_observer(player: &Retained<NSObject>) -> Retained<NSObject> {
    let center: Retained<NSObject> = unsafe {
        msg_send_id![objc2::class!(NSNotificationCenter), defaultCenter]
    };
    let name = NSString::from_str("AVPlayerItemDidPlayToEndTimeNotification");
    // nil `object:` → observe end-of-play for ANY AVPlayerItem this
    // player is currently using (we don't hold a stable reference to
    // the item across `replaceCurrentItemWithPlayerItem:`).
    let nil_obj: *const AnyObject = std::ptr::null();
    let nil_queue: *const AnyObject = std::ptr::null();

    let player_for_block = player.clone();
    let block = block2::StackBlock::new(move |_note: NonNull<NSObject>| {
        // kCMTimeZero == {0, 1, kCMTimeFlags_Valid, 0}
        let zero = CMTime {
            value: 0,
            timescale: 1,
            flags: CM_TIME_FLAG_VALID,
            epoch: 0,
        };
        let _: () = unsafe { msg_send![&player_for_block, seekToTime: zero] };
        let _: () = unsafe { msg_send![&player_for_block, play] };
    });
    let block = block.copy();

    unsafe {
        msg_send_id![
            &center,
            addObserverForName: &*name,
            object: nil_obj,
            queue: nil_queue,
            usingBlock: &*block
        ]
    }
}

// =============================================================================
// VideoOps impl — drives play/pause/seek from `VideoHandle` calls.
// =============================================================================

struct IosVideoOps;

impl VideoOps for IosVideoOps {
    fn play(&self, node: &dyn Any) {
        let Some(player) = lookup_player(node) else { return };
        let _: () = unsafe { msg_send![&*player, play] };
    }

    fn pause(&self, node: &dyn Any) {
        let Some(player) = lookup_player(node) else { return };
        let _: () = unsafe { msg_send![&*player, pause] };
    }

    fn seek(&self, node: &dyn Any, seconds: f32) {
        let Some(player) = lookup_player(node) else { return };
        // Mirror `CMTimeMakeWithSeconds(seconds, 600)` — 600 is
        // AVFoundation's recommended preferred-timescale for video so
        // the result divides common framerates evenly. Building the
        // struct directly avoids linking CoreMedia's helper symbol.
        let timescale = 600i32;
        let value = (seconds as f64 * timescale as f64).round() as i64;
        let t = CMTime {
            value,
            timescale,
            flags: CM_TIME_FLAG_VALID,
            epoch: 0,
        };
        let _: () = unsafe { msg_send![&*player, seekToTime: t] };
    }
}

fn lookup_player(node: &dyn Any) -> Option<Retained<NSObject>> {
    let ios_node = node.downcast_ref::<IosNode>()?;
    let IosNode::View(view) = ios_node else { return None };
    let key = &**view as *const UIView as usize;
    PLAYER_TABLE.with(|t| t.borrow().get(&key).map(|e| e.player.clone()))
}

thread_local! {
    /// view-pointer → AVPlayer side-table. Populated by `build_video`
    /// at mount; entries are never removed (see file-level docs for
    /// the leak rationale).
    static PLAYER_TABLE: RefCell<HashMap<usize, VideoEntry>> =
        RefCell::new(HashMap::new());
}

//! macOS (AppKit) implementation of the Video SDK.
//!
//! Covers BOTH source kinds, mirroring iOS:
//!
//! - **`MediaContent::Url`** (a recorded file / remote URL): an `AVPlayer`
//!   driving an `AVPlayerLayer` sublayer of the host view's root layer. This is
//!   what makes the whiteboard's recording preview play — before it, a URL
//!   source no-op'd and the preview showed only its dark stage box.
//! - **`MediaContent::Stream`** (a live `camera` / `screen-recorder` feed): the
//!   universal CPU CGImage path (and IOSurface zero-copy fast-path) pushing
//!   frames into the root CALayer's `contents`.
//!
//! A given video element is one or the other; the unused layer stays empty, so
//! the two paths coexist without overlapping — same design as the iOS module.
//!
//! Mechanism notes:
//! - AppKit's `NSView` is **not** layer-backed by default — we `setWantsLayer:
//!   true` before touching `.layer`, or it's nil (see
//!   `project_macos_appkit_uikit_diffs`, gotcha #3).
//! - `AVPlayerLayer` doesn't auto-track its host view's bounds; we size it from
//!   the view's `bounds` every frame in a `raf_loop_scoped` (an `Effect`
//!   reading `bounds` imperatively tracks no signal, so it would fire once at
//!   0×0 during build and leave the layer invisible).
//! - AVFoundation is reached via raw `msg_send!` + `class!(...)` (no
//!   `objc2-av-foundation`, whose 0.3 line needs a newer objc2 than this
//!   crate's pinned 0.5) and force-linked below.

use crate::cg_image::{cgimage_from_rgba, CGImageRelease};
use crate::{MediaContent, VideoOps, VideoProps};
use backend_macos::{MacosBackend, MacosNode};
use media_stream::SurfaceSource;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::{CGRect, NSObject, NSString};
use runtime_core::effect;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::Rc;

pub(crate) static OPS: &dyn VideoOps = &MacosVideoOps;

// AVFoundation ships with macOS; force the linker to pull it in (the AVPlayer /
// AVPlayerLayer selectors below are sent dynamically, so nothing else
// references the framework at link time). Mirrors the iOS module.
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

// CoreMedia `CMTime` mirror (objc2 doesn't re-export it and we don't pull in
// core-media-sys for two struct layouts). Field order/widths MUST match
// `<CoreMedia/CMTime.h>` exactly or AVPlayer reads garbage on `seekToTime:`.
#[repr(C)]
#[derive(Copy, Clone)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

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

/// Per-video state retained in the side-table. `loop_observer` is held purely
/// for its retention side-effect — releasing it removes the notification
/// observer.
struct VideoEntry {
    player: Retained<AnyObject>,
    #[allow(dead_code)]
    player_layer: Retained<AnyObject>,
    #[allow(dead_code)]
    loop_observer: Option<Retained<NSObject>>,
}

thread_local! {
    /// view-pointer → AVPlayer side-table. Populated by `build_video` at mount;
    /// entries are never removed (matches the iOS module's v1 leak model — SDK
    /// v2 swaps to an associated object that releases on dealloc).
    static PLAYER_TABLE: RefCell<HashMap<usize, VideoEntry>> =
        RefCell::new(HashMap::new());
}

/// Register the Video handler against a `MacosBackend`. One-line call from
/// the app's bootstrap.
pub fn register(backend: &mut MacosBackend) {
    backend.register_external::<VideoProps, _>(|props, b| build_video(props, b));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_macos::MacosExternalRegistrar(register)
}

fn build_video(props: &Rc<VideoProps>, b: &mut MacosBackend) -> MacosNode {
    // Plain NSView — no subclass needed. Taffy drives the view's frame via the
    // regular apply_frames path; the root CALayer hosts the AVPlayerLayer
    // sublayer (URL) and/or the stream's frames as its `contents`.
    let view: Retained<NSView> = unsafe {
        let alloc = b.mtm().alloc::<NSView>();
        msg_send_id![alloc, init]
    };

    // AppKit gotcha #3: NSView is not layer-backed by default — set wantsLayer
    // BEFORE reaching for `.layer` (else it's nil).
    let _: () = unsafe { msg_send![&view, setWantsLayer: true] };

    // Register with the backend's Taffy tree so a flex parent sizes + positions
    // the view. Without this it lays out as 0×0.
    b.register_external_view(&view);

    // ---- URL / AVPlayer path -------------------------------------------------
    // Build an AVPlayer + AVPlayerLayer and add the layer as a sublayer of the
    // view's root layer. A Stream source resolves to no URL, so the player just
    // stays idle (and the layer empty) for the camera case.
    {
        let initial_src = resolved_url(props).unwrap_or_default();
        let player = build_player(&initial_src);
        let player_layer = build_player_layer(&player, props.object_fit);

        let view_layer: Retained<AnyObject> = unsafe { msg_send_id![&view, layer] };
        let _: () = unsafe { msg_send![&view_layer, addSublayer: &*player_layer] };

        // Autoplay: muted (mirrors the web/iOS "autoplay = silent autoplay"
        // expectation) + play.
        if props.autoplay {
            let _: () = unsafe { msg_send![&player, setMuted: true] };
            let _: () = unsafe { msg_send![&player, play] };
        }

        let loop_observer = if props.loop_playback {
            Some(install_loop_observer(&player))
        } else {
            None
        };

        let key = &*view as *const NSView as usize;
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

        // Size the AVPlayerLayer to the view's bounds EVERY frame. The layer
        // doesn't auto-track its parent; an Effect reading `bounds` imperatively
        // tracks no signal (fires once at 0×0 → invisible video), so the raf
        // loop is the reliable home for sizing. CATransaction disables the
        // implicit frame-change animation.
        let player_layer_for_size = player_layer.clone();
        let view_for_size = view.clone();
        runtime_core::raf_loop_scoped(move || unsafe {
            let bounds: CGRect = msg_send![&*view_for_size, bounds];
            let txn = objc2::class!(CATransaction);
            let _: () = msg_send![txn, begin];
            let _: () = msg_send![txn, setDisableActions: true];
            let _: () = msg_send![&*player_layer_for_size, setFrame: bounds];
            let _: () = msg_send![txn, commit];
        });

        // Reactive src. Load a new item whenever the resolved URL CHANGES to a
        // non-empty value, tracking the last-loaded URL. A `first_run`-skip flag is
        // wrong: the URL often resolves asynchronously (a recorded-file path lands
        // AFTER mount), so the effect's first run already sees the real URL and
        // would skip it, leaving the player empty and the preview blank.
        // `replaceCurrentItemWithPlayerItem:` keeps the layer attachment, so swaps
        // don't disrupt layout. `last_url` starts as whatever the player was built
        // with, so an unchanged URL doesn't double-load.
        let player_for_src = player.clone();
        let props_for_src = props.clone();
        let last_url = RefCell::new(initial_src.clone());
        effect!({
            let url = resolved_url(&props_for_src).unwrap_or_default();
            if url.is_empty() || url == *last_url.borrow() {
                return;
            }
            if let Some(item) = build_player_item(&url) {
                let _: () = unsafe {
                    msg_send![&player_for_src, replaceCurrentItemWithPlayerItem: &*item]
                };
                *last_url.borrow_mut() = url;
                if props_for_src.autoplay {
                    let _: () = unsafe { msg_send![&player_for_src, play] };
                }
            }
        });
    }

    // ---- Live MediaStream display path (unchanged) ---------------------------
    {
        let view_layer: Retained<AnyObject> = unsafe { msg_send_id![&view, layer] };
        // Aspect-preserving fill, never the default stretch. `resizeAspect`
        // letterboxes (contain); `resizeAspectFill` crops to fill (cover).
        let gravity = NSString::from_str(match props.object_fit {
            crate::ObjectFit::Cover => "resizeAspectFill",
            crate::ObjectFit::Contain => "resizeAspect",
        });
        let _: () = unsafe { msg_send![&view_layer, setContentsGravity: &*gravity] };

        let view_for_stream = view.clone();
        let props_for_stream = props.clone();
        let mut last_gen: u64 = u64::MAX;
        let mut last_native_gen: u64 = u64::MAX;
        let mut scratch: Vec<u8> = Vec::new();
        runtime_core::raf_loop_scoped(move || {
            let MediaContent::Stream(stream) = props_for_stream.source.resolve() else {
                return;
            };

            // Zero-copy fast-path. If the producer published a native IOSurface
            // source (the screen-recorder does), set each frame's surface
            // straight as the layer's `contents`: no BGRA→RGBA swizzle, no
            // CGImage, no per-frame texture upload — CoreAnimation displays the
            // IOSurface's GPU texture directly. This is what eliminates the
            // preview's CPU cost and end-to-end latency.
            if let Some(native) = stream.native_source() {
                if let Some(surf) = native.downcast_ref::<SurfaceSource>() {
                    let generation = surf.generation();
                    if generation == last_native_gen {
                        return;
                    }
                    last_native_gen = generation;
                    // `acquire` hands back an extra retain so the surface can't
                    // be freed by a concurrent capture-queue `publish` between
                    // here and `setContents:`. The layer takes its own retain;
                    // we release ours immediately after.
                    let surface = surf.acquire();
                    if surface.is_null() {
                        return;
                    }
                    unsafe {
                        let layer: Retained<AnyObject> =
                            msg_send_id![&view_for_stream, layer];
                        let _: () =
                            msg_send![&layer, setContents: surface as *const AnyObject];
                        surf.release(surface);
                    }
                    return;
                }
            }

            // CPU fallback (camera, or any RGBA-only stream with no native
            // source): copy the latest frame into a CGImage and push it.
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
                let layer: Retained<AnyObject> = msg_send_id![&view_for_stream, layer];
                let _: () = msg_send![&layer, setContents: image as *const AnyObject];
                CGImageRelease(image); // the layer holds its own reference
            }
        });
    }

    MacosNode::View(view)
}

// =============================================================================
// Helpers (private) — mirror the iOS module.
// =============================================================================

/// Resolve a source to a URL for the AVPlayer path. A live `Stream` source has
/// no native player binding (that's the GPU/compositing phase); it resolves to
/// no URL, leaving the player idle.
fn resolved_url(props: &VideoProps) -> Option<String> {
    match props.source.resolve() {
        MediaContent::Url(u) => Some(u),
        MediaContent::Stream(_) | MediaContent::None => None,
    }
}

fn build_nsurl(s: &str) -> Option<Retained<AnyObject>> {
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

fn build_player_item(src: &str) -> Option<Retained<AnyObject>> {
    let url = build_nsurl(src)?;
    let item: Retained<AnyObject> = unsafe {
        msg_send_id![
            msg_send_id![objc2::class!(AVPlayerItem), alloc],
            initWithURL: &*url
        ]
    };
    Some(item)
}

fn build_player(src: &str) -> Retained<AnyObject> {
    // Empty URL → a player with no item (populated later via the reactive src
    // Effect). `URLWithString:""` returns nil and `playerWithURL:nil` crashes
    // AVPlayer's designated initializer, so guard it.
    match build_nsurl(src) {
        Some(url) => unsafe { msg_send_id![objc2::class!(AVPlayer), playerWithURL: &*url] },
        None => unsafe { msg_send_id![objc2::class!(AVPlayer), new] },
    }
}

fn build_player_layer(
    player: &Retained<AnyObject>,
    object_fit: crate::ObjectFit,
) -> Retained<AnyObject> {
    let layer: Retained<AnyObject> = unsafe {
        msg_send_id![objc2::class!(AVPlayerLayer), playerLayerWithPlayer: &**player]
    };
    let gravity = NSString::from_str(av_layer_gravity(object_fit));
    let _: () = unsafe { msg_send![&layer, setVideoGravity: &*gravity] };
    layer
}

/// `AVLayerVideoGravity*` string for an [`crate::ObjectFit`].
fn av_layer_gravity(fit: crate::ObjectFit) -> &'static str {
    match fit {
        crate::ObjectFit::Cover => "AVLayerVideoGravityResizeAspectFill",
        crate::ObjectFit::Contain => "AVLayerVideoGravityResizeAspect",
    }
}

/// Observe `AVPlayerItemDidPlayToEndTimeNotification` (nil object → any item the
/// player uses); on receipt seek to zero and resume. Returns the retained
/// observer token (kept alive for the video's lifetime).
fn install_loop_observer(player: &Retained<AnyObject>) -> Retained<NSObject> {
    let center: Retained<NSObject> =
        unsafe { msg_send_id![objc2::class!(NSNotificationCenter), defaultCenter] };
    let name = NSString::from_str("AVPlayerItemDidPlayToEndTimeNotification");
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

fn lookup_player(node: &dyn Any) -> Option<Retained<AnyObject>> {
    let macos_node = node.downcast_ref::<MacosNode>()?;
    let MacosNode::View(view) = macos_node else {
        return None;
    };
    let key = &**view as *const NSView as usize;
    PLAYER_TABLE.with(|t| t.borrow().get(&key).map(|e| e.player.clone()))
}

// =============================================================================
// VideoOps impl — drives play/pause/seek from `VideoHandle` calls (URL path).
// Stream sources have no AVPlayer entry, so the lookups no-op for them.
// =============================================================================

struct MacosVideoOps;

impl VideoOps for MacosVideoOps {
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
        // Mirror `CMTimeMakeWithSeconds(seconds, 600)` — 600 divides common
        // framerates evenly; building the struct avoids linking CoreMedia.
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

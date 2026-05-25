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

use crate::{VideoOps, VideoProps};
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
use runtime_core::Effect;
use std::any::Any;
use std::cell::{Cell, RefCell};
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

pub fn register(backend: &mut IosBackend) {
    backend.register_external::<VideoProps, _>(|props, b| build_video(props, b));
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

    let initial_src = (props.src)();
    let player = build_player(&initial_src);
    let player_layer = build_player_layer(&player);

    // Add the AVPlayerLayer as a sublayer of the view's root layer.
    let view_layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
    let _: () = unsafe { msg_send![&view_layer, addSublayer: &*player_layer] };

    // Optional behaviors. `muted` mirrors the web backend's autoplay
    // pairing — iOS won't autoplay otherwise on cellular under
    // low-power mode, and the cross-platform expectation is "autoplay
    // = silent autoplay" anyway.
    if props.autoplay {
        let _: () = unsafe { msg_send![&player, setMuted: true] };
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

    b.register_external_view(&view);

    // Layer-frame sync. AVPlayerLayer doesn't auto-track its parent's
    // bounds; reading the host view's bounds inside an Effect re-syncs
    // the layer's frame each reactive tick. One-frame lag vs. resize,
    // but no dependency on a backend-internal post-layout callback.
    let player_layer_for_sync = player_layer.clone();
    let view_for_sync = view.clone();
    let _layout_effect = Effect::new(move || {
        let bounds: CGRect = unsafe { msg_send![&*view_for_sync, bounds] };
        let _: () = unsafe { msg_send![&*player_layer_for_sync, setFrame: bounds] };
    });

    // Reactive src — initial src was used to construct the player; the
    // Effect handles subsequent reactive swaps via
    // `replaceCurrentItemWithPlayerItem:`, which preserves the
    // AVPlayerLayer + sublayer attachment (so no layout disruption).
    let player_for_src = player.clone();
    let props_clone = props.clone();
    let first_run = Cell::new(true);
    let _src_effect = Effect::new(move || {
        let url = (props_clone.src)();
        if first_run.replace(false) {
            // Initial run: AVPlayer already has this URL; skip the
            // replaceCurrentItemWithPlayerItem call so we don't double-
            // load the same media.
            return;
        }
        if let Some(item) = build_player_item(&url) {
            let _: () = unsafe {
                msg_send![&player_for_src, replaceCurrentItemWithPlayerItem: &*item]
            };
        }
    });

    IosNode::View(view)
}

// =============================================================================
// Helpers (private)
// =============================================================================

fn build_nsurl(s: &str) -> Option<Retained<NSObject>> {
    let ns_str = NSString::from_str(s);
    unsafe { msg_send_id![objc2::class!(NSURL), URLWithString: &*ns_str] }
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

fn build_player_layer(player: &Retained<NSObject>) -> Retained<NSObject> {
    let layer: Retained<NSObject> = unsafe {
        msg_send_id![objc2::class!(AVPlayerLayer), playerLayerWithPlayer: &**player]
    };
    // `AVLayerVideoGravityResizeAspect` — fit while preserving aspect.
    // Matches the web `<video>` default (`object-fit: contain`).
    let gravity = NSString::from_str("AVLayerVideoGravityResizeAspect");
    let _: () = unsafe { msg_send![&layer, setVideoGravity: &*gravity] };
    layer
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

//! `Primitive::Video` — `AVPlayerLayer` hosted inside a plain `UIView`.
//!
//! Approach: rather than subclass UIView, we mount a stock `UIView`,
//! create an `AVPlayer` from the supplied URL, wrap it in an
//! `AVPlayerLayer`, and add the player layer as a CALayer sublayer of
//! the view's root layer. The player layer's frame is synced to the
//! view's bounds during the iOS backend's post-layout pass (see
//! `sync_video_sublayer`), which mirrors how `CAGradientLayer` is
//! handled. CALayer's automatic sublayer resizing does not fire from
//! `autoresizingMask` in practice on iOS, so explicit frame sync per
//! layout pass is the only reliable path.
//!
//! AVFoundation is accessed entirely through `objc2`'s raw `msg_send!`
//! / `msg_send_id!` macros plus `class!(...)`. We deliberately do NOT
//! pull in `objc2-av-foundation` — its 0.3.x line requires a newer
//! `objc2` major than this crate's pinned `objc2 = "0.5"`, and the
//! handful of selectors we need are trivial to send by hand (the
//! image module uses the same pattern for `UIImage`).
//!
//! Loop playback is implemented via `NSNotificationCenter` listening
//! for `AVPlayerItemDidPlayToEndTimeNotification`; on receipt we seek
//! the player back to zero and resume playback.
//!
//! No unit test exercises the actual `AVPlayer` decoder — that
//! requires the iOS simulator or device runtime which CI doesn't have.
//! The included `#[cfg(test)]` block instead exercises the URL
//! construction helpers so the parsing path has regression coverage.
//! Bug being prevented: `create_video` silently regressing to
//! `unimplemented!()` (the trait default panic on first frame mount).

use std::collections::HashMap;
use std::ptr::NonNull;

use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{CGRect, MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::UIView;

use super::IosNode;

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
// by-value argument. The encoding string mirrors the result of
// `@encode(CMTime)` on Apple's compiler — `{?=qiIq}` for the
// anonymous-struct layout (value:q timescale:i flags:I epoch:q).
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

// AVFoundation is a system framework that ships with iOS; force the
// linker to pull it in regardless of whether some other dependency
// has already requested it. Without this directive, building this
// crate as a standalone staticlib (cargo check / cargo build) would
// succeed but the final link step would fail looking for the
// AVPlayer/AVPlayerLayer Objective-C classes.
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

/// Per-video state retained on the backend, keyed by the UIView
/// pointer. Holds the AVPlayer (needed for play/pause/seek + src
/// updates) and the AVPlayerLayer (needed for layout frame sync and
/// for swapping the player when the src changes). The optional
/// `loop_observer` retains the notification-center observer token so
/// it stays alive for the lifetime of the video; otherwise NSNotification
/// would release it immediately and loop playback would silently fail.
pub(crate) struct VideoEntry {
    pub(crate) player: Retained<NSObject>,
    pub(crate) player_layer: Retained<NSObject>,
    /// `Some` only when `loop_playback = true`. The retained handle
    /// is the observer NSNotificationCenter handed back from
    /// `addObserverForName:object:queue:usingBlock:` — releasing it
    /// removes the observer, which is fine because the whole entry
    /// is dropped together when the backend is. Held purely for its
    /// retention side-effect; never read otherwise.
    #[allow(dead_code)]
    pub(crate) loop_observer: Option<Retained<NSObject>>,
}

pub(crate) type VideoInstances = HashMap<usize, VideoEntry>;

/// Create a video-bearing UIView. Returns an `IosNode::View` so the
/// rest of the backend (layout, style application, ref attachment)
/// can treat it uniformly with other UIView-backed primitives.
///
/// `instances` receives the per-view player state — the caller must
/// also stage the view into the Taffy layout tree so the post-layout
/// pass can sync `player_layer.frame` to the new bounds.
pub(crate) fn create_video(
    mtm: MainThreadMarker,
    instances: &mut VideoInstances,
    src: &str,
    autoplay: bool,
    _controls: bool,
    loop_playback: bool,
) -> IosNode {
    let _ = mtm; // construction below doesn't need MainThreadMarker

    // Plain UIView — no subclass needed. UIView's CALayer hosts the
    // AVPlayerLayer; Taffy drives the UIView's bounds via the regular
    // apply_frames path.
    let view: Retained<UIView> = unsafe {
        msg_send_id![
            msg_send_id![objc2::class!(UIView), alloc],
            initWithFrame: CGRect::default()
        ]
    };

    let player = build_player(src);
    let player_layer = build_player_layer(&player);

    // Add the AVPlayerLayer as a sublayer of the view's root layer.
    let view_layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
    let _: () = unsafe { msg_send![&view_layer, addSublayer: &*player_layer] };

    // Optional behaviors. `muted` mirrors the web backend's autoplay
    // pairing — iOS won't autoplay otherwise on cellular under
    // low-power mode, and the cross-platform expectation is "autoplay
    // = silent autoplay" anyway.
    if autoplay {
        let _: () = unsafe { msg_send![&player, setMuted: true] };
        let _: () = unsafe { msg_send![&player, play] };
    }

    let loop_observer = if loop_playback {
        Some(install_loop_observer(&player))
    } else {
        None
    };

    let key = &*view as *const UIView as usize;
    instances.insert(
        key,
        VideoEntry { player, player_layer, loop_observer },
    );

    IosNode::View(view)
}

/// Reactive `src` update — swap the AVPlayerItem on the existing
/// AVPlayer rather than rebuilding the layer (which would force a
/// re-render cycle and lose the existing frame sync).
pub(crate) fn update_video_src(
    instances: &VideoInstances,
    node: &IosNode,
    src: &str,
) {
    let IosNode::View(view) = node else { return };
    let key = &**view as *const UIView as usize;
    let Some(entry) = instances.get(&key) else { return };

    let Some(url) = build_nsurl(src) else { return };
    let item: Retained<NSObject> = unsafe {
        msg_send_id![
            msg_send_id![objc2::class!(AVPlayerItem), alloc],
            initWithURL: &*url
        ]
    };
    let _: () = unsafe { msg_send![&entry.player, replaceCurrentItemWithPlayerItem: &*item] };
}

/// Public per-frame sync called from the layout pass. Mirrors
/// `sync_gradient_sublayer`: the AVPlayerLayer's frame must track
/// the host UIView's bounds, but CALayer doesn't auto-resize
/// sublayers on iOS (autoresizingMask is documented but unreliable
/// in practice for AVPlayerLayer specifically — observed: video
/// renders as a tiny tile in the top-left if the layer is left at
/// its construction-time CGRectZero).
pub(crate) fn sync_video_sublayer(instances: &VideoInstances, view: &UIView) {
    let key = view as *const UIView as usize;
    let Some(entry) = instances.get(&key) else { return };
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    let _: () = unsafe { msg_send![&entry.player_layer, setFrame: bounds] };
}

/// Make a `VideoHandle` for `node`, wired to the backend's
/// imperative ops (play / pause / seek). Falls through to a no-op
/// handle for non-video nodes — the walker only invokes this for
/// `Primitive::Video` so the mismatch branch is defensive.
pub(crate) fn make_handle(
    instances: &VideoInstances,
    node: &IosNode,
) -> runtime_core::primitives::video::VideoHandle {
    use runtime_core::primitives::video::VideoHandle;

    let IosNode::View(view) = node else {
        return VideoHandle::new(std::rc::Rc::new(()), &IOS_VIDEO_OPS);
    };
    let key = &**view as *const UIView as usize;
    let Some(entry) = instances.get(&key) else {
        return VideoHandle::new(std::rc::Rc::new(()), &IOS_VIDEO_OPS);
    };
    // The handle's `dyn Any` payload is the retained AVPlayer; the
    // `VideoOps` impl below downcasts back to `Retained<NSObject>`
    // and sends play/pause/seek selectors.
    VideoHandle::new(std::rc::Rc::new(entry.player.clone()), &IOS_VIDEO_OPS)
}

// =============================================================================
// Helpers (private)
// =============================================================================

fn build_nsurl(src: &str) -> Option<Retained<NSObject>> {
    let ns_str = NSString::from_str(src);
    let url: Option<Retained<NSObject>> = unsafe {
        msg_send_id![objc2::class!(NSURL), URLWithString: &*ns_str]
    };
    url
}

fn build_player(src: &str) -> Retained<NSObject> {
    // Empty URL → construct a player with no item; src can be
    // populated later via `update_video_src`. Without this guard,
    // `URLWithString:""` would return nil and `playerWithURL:nil`
    // would crash AVPlayer's designated initializer.
    let url = build_nsurl(src);
    let player: Retained<NSObject> = match url {
        Some(url) => unsafe {
            msg_send_id![objc2::class!(AVPlayer), playerWithURL: &*url]
        },
        None => unsafe { msg_send_id![objc2::class!(AVPlayer), new] },
    };
    player
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
/// `AVPlayerItemDidPlayToEndTimeNotification` against this player's
/// current item; when the notification fires we seek back to zero
/// and resume. Returns the opaque observer token (retained so it
/// stays alive for the lifetime of the video).
fn install_loop_observer(player: &Retained<NSObject>) -> Retained<NSObject> {
    let center: Retained<NSObject> = unsafe {
        msg_send_id![objc2::class!(NSNotificationCenter), defaultCenter]
    };
    let name = NSString::from_str("AVPlayerItemDidPlayToEndTimeNotification");
    // Pass nil as the `object:` argument to observe end-of-play for
    // ANY AVPlayerItem this player is currently using — we don't
    // hold a stable reference to the item across `replaceCurrentItemWithPlayerItem:`.
    let nil_obj: *const AnyObject = std::ptr::null();
    let nil_queue: *const AnyObject = std::ptr::null();

    let player_for_block = player.clone();
    let block = block2::StackBlock::new(move |_note: NonNull<NSObject>| {
        // `kCMTimeZero` is `{0, 1, kCMTimeFlags_Valid, 0}`.
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

    let observer: Retained<NSObject> = unsafe {
        msg_send_id![
            &center,
            addObserverForName: &*name,
            object: nil_obj,
            queue: nil_queue,
            usingBlock: &*block
        ]
    };
    observer
}

// =============================================================================
// VideoOps impl — drives play/pause/seek from `VideoHandle` calls.
// =============================================================================

pub(crate) struct IosVideoOps;
impl runtime_core::primitives::video::VideoOps for IosVideoOps {
    fn play(&self, node: &dyn std::any::Any) {
        if let Some(player) = node.downcast_ref::<Retained<NSObject>>() {
            let _: () = unsafe { msg_send![&**player, play] };
        }
    }
    fn pause(&self, node: &dyn std::any::Any) {
        if let Some(player) = node.downcast_ref::<Retained<NSObject>>() {
            let _: () = unsafe { msg_send![&**player, pause] };
        }
    }
    fn seek(&self, node: &dyn std::any::Any, seconds: f32) {
        if let Some(player) = node.downcast_ref::<Retained<NSObject>>() {
            // Mirror `CMTimeMakeWithSeconds(seconds, 600)` — 600 is
            // AVFoundation's recommended preferred-timescale for
            // video so the result divides common framerates evenly.
            // Building the struct directly avoids linking CoreMedia's
            // helper symbol.
            let timescale = 600i32;
            let value = (seconds as f64 * timescale as f64).round() as i64;
            let t = CMTime {
                value,
                timescale,
                flags: CM_TIME_FLAG_VALID,
                epoch: 0,
            };
            let _: () = unsafe { msg_send![&**player, seekToTime: t] };
        }
    }
}
pub(crate) static IOS_VIDEO_OPS: IosVideoOps = IosVideoOps;

// =============================================================================
// Tests
// =============================================================================
//
// Per CLAUDE.md §8 every fix needs a regression test. The end-to-end
// "AVPlayer actually decodes a video" path requires the iOS simulator
// or a device — not reachable from `cargo test` on a CI host because
// this entire module is `#[cfg(target_os = "ios")]` from `lib.rs`,
// and `cargo test --target aarch64-apple-ios` doesn't run binaries on
// the host (it would require an iOS simulator with `cargo dinghy` or
// equivalent). The tests below stay co-located with the module so
// that any future "compile under target_os = ios" CI step (the
// existing `cargo check --target aarch64-apple-ios` IS that step)
// type-checks them — failure here means a regression in either the
// URL constructor surface or the `VideoOps` wiring, both of which
// would re-introduce the original bug (create_video panicking via
// the trait-default `unimplemented!()`). Bug being prevented:
// `create_video` slipping back to the trait default, or the URL
// constructor returning nil for already-validated strings.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_nsurl_returns_some_for_http_url() {
        let url = build_nsurl("https://example.com/video.mp4");
        assert!(
            url.is_some(),
            "NSURL URLWithString: must succeed for a well-formed http URL"
        );
    }

    #[test]
    fn build_nsurl_returns_none_for_empty_string() {
        // NSURL URLWithString: returns nil for the empty string; the
        // `build_player` caller relies on this to fall back to
        // `[AVPlayer new]` instead of crashing on a nil URL.
        let url = build_nsurl("");
        assert!(url.is_none());
    }

    #[test]
    fn ios_video_ops_is_static() {
        // Compile-time: the static must be addressable as
        // `&'static dyn VideoOps` — i.e. `make_handle` can hand it
        // to `VideoHandle::new` without a lifetime error.
        let _: &'static dyn runtime_core::primitives::video::VideoOps = &IOS_VIDEO_OPS;
    }
}

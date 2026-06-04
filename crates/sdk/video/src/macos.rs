//! macOS (AppKit) implementation of the Video SDK.
//!
//! Scope: the **live `MediaStream` display path** only (the camera case).
//! A `MediaStream` (from `camera` / `screen-recorder`) is not a URL, so this
//! module deliberately does not port the iOS `AVPlayer`/URL path. A
//! `MediaContent::Url` source no-ops here for now (see the TODO in
//! [`build_video`]).
//!
//! Mechanism: a plain, layer-backed `NSView` hosts the frames. Unlike UIKit,
//! AppKit's `NSView` is **not** layer-backed by default — we must
//! `setWantsLayer: true` before touching `.layer`, or the layer is nil (see
//! the `project_macos_appkit_uikit_diffs` memory, gotcha #3). We set the
//! root layer's `contentsGravity = "resizeAspect"` (aspect-fit, matching the
//! iOS path) and run the same `raf_loop_scoped` stream→CGImage→`setContents`
//! loop iOS uses. The CoreGraphics RGBA→CGImage conversion is shared via
//! [`crate::cg_image`] — it's identical CoreGraphics on both platforms.

use crate::cg_image::{cgimage_from_rgba, CGImageRelease};
use crate::{MediaContent, VideoOps, VideoProps};
use backend_macos::{MacosBackend, MacosNode};
use media_stream::SurfaceSource;
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::NSString;
use std::any::Any;
use std::rc::Rc;

pub(crate) static OPS: &dyn VideoOps = &MacosVideoOps;

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
    // Plain NSView — no subclass needed. Taffy drives the view's frame via
    // the regular apply_frames path; the root CALayer hosts the stream's
    // frames as its `contents`.
    let view: Retained<NSView> = unsafe {
        let alloc = b.mtm().alloc::<NSView>();
        msg_send_id![alloc, init]
    };

    // AppKit gotcha #3: NSView is not layer-backed by default. `.layer` is
    // nil until `setWantsLayer: true`, so set it BEFORE reaching for the
    // layer below.
    let _: () = unsafe { msg_send![&view, setWantsLayer: true] };

    // Register with the backend's Taffy tree so a flex parent sizes +
    // positions the view. Without this it lays out as 0×0.
    b.register_external_view(&view);

    // Live MediaStream display. A `Stream` source has no AVPlayer URL; we
    // poll the stream's latest frame on the main thread (via the scope-tied
    // frame loop) and push it to the view's root CALayer `contents` as a
    // CGImage. This is the universal CPU path — works for ANY MediaStream
    // (camera, screen, a compositor's output). A faster native path
    // (AVSampleBufferDisplayLayer / a Metal texture) is the GPU phase; this
    // renders correctly and stays simple.
    //
    // TODO: AVPlayer URL path — a `MediaContent::Url` source no-ops on macOS
    // for now (the camera widget this unblocks is a Stream, not a URL).
    {
        let view_layer: Retained<AnyObject> = unsafe { msg_send_id![&view, layer] };
        // Aspect-preserving fill mode, never the default stretch. `resizeAspect`
        // letterboxes (contain); `resizeAspectFill` crops to fill (cover). Drives
        // both the IOSurface fast-path and the CGImage fallback (same root layer).
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
// VideoOps impl — play/pause/seek are no-ops on macOS for now.
//
// The live MediaStream path is display-only (the stream owns its own
// lifecycle); there is no AVPlayer to drive. Imperative ops are wired only
// once the AVPlayer/URL path lands (see the TODO in `build_video`). Until
// then they degrade silently rather than panicking — matching `VideoOps`'
// default no-op contract.
// =============================================================================

struct MacosVideoOps;

impl VideoOps for MacosVideoOps {
    fn play(&self, _node: &dyn Any) {}
    fn pause(&self, _node: &dyn Any) {}
    fn seek(&self, _node: &dyn Any, _seconds: f32) {}
}

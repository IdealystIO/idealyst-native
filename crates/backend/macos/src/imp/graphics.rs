//! `Primitive::Graphics` — wgpu Surface backed by a CAMetalLayer
//! attached to a layer-backed NSView. Mirrors the iOS UIView+
//! CAMetalLayer pattern at
//! `crates/backend/ios/mobile/src/imp/graphics.rs`.
//!
//! ## How NSView + CAMetalLayer works
//!
//! NSView's layer is opt-in: you set `wantsLayer = true` and AppKit
//! creates a CALayer for you. The subclass override `-makeBackingLayer`
//! lets you return a different `CALayer` subclass — we return a
//! fresh `CAMetalLayer` instance, which is what wgpu's `metal`
//! backend expects to wrap.
//!
//! ## raw_window_handle bridge
//!
//! wgpu reaches the layer via `raw_window_handle::AppKitWindowHandle`
//! pointing at the NSView. Same shape as iOS's `UiKitWindowHandle` —
//! the platform-specific handle type changes but the call surface
//! is identical.

use runtime_core::primitives::graphics::{
    GraphicsSurface, OnLost, OnReady, OnReadyEvent, OnResize,
};
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{declare_class, mutability, ClassType, DeclaredClass};
use objc2_app_kit::NSView;
use objc2_foundation::{CGFloat, CGRect, MainThreadMarker, NSObject};
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, DisplayHandle, HandleError,
    HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use super::callbacks::CallbackTarget;
use super::MacosNode;

// =========================================================================
// MetalView — NSView subclass whose backing layer is a CAMetalLayer.
//
// NSView layer-backing is via `setWantsLayer:` + an automatically
// constructed `CALayer`. The subclass override `-makeBackingLayer`
// is called by AppKit when `wantsLayer` is enabled to ask the view
// what layer class it wants. Return a fresh `CAMetalLayer` and
// AppKit assigns it as the view's layer (no further setLayer:
// call needed).
// =========================================================================

declare_class!(
    pub(crate) struct MetalView;

    unsafe impl ClassType for MetalView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacMetalView";
    }

    impl DeclaredClass for MetalView {
        type Ivars = ();
    }

    unsafe impl MetalView {
        /// AppKit calls this when `wantsLayer` is enabled to build
        /// the view's backing layer. Returns a freshly-allocated
        /// `CAMetalLayer` cast to `CALayer` — AppKit treats the
        /// pointer opaquely, and wgpu's metal backend recognises
        /// the layer's actual class when it queries via
        /// `raw_window_handle`.
        #[method_id(makeBackingLayer)]
        fn make_backing_layer(&self) -> Retained<NSObject> {
            let cls: &AnyClass = AnyClass::get("CAMetalLayer")
                .expect("CAMetalLayer class not registered \
                         — is QuartzCore linked into the binary?");
            unsafe {
                let allocated: *mut AnyObject = msg_send![cls, alloc];
                let inited: *mut AnyObject = msg_send![allocated, init];
                Retained::from_raw(inited.cast::<NSObject>())
                    .expect("CAMetalLayer init returned nil")
            }
        }
    }
);

impl MetalView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// raw_window_handle provider — bridges the NSView pointer to wgpu.
// =========================================================================

struct MacosSurfaceProvider {
    /// Raw NSView pointer. `*mut c_void` rather than
    /// `Retained<NSView>` because wgpu's surface init wants a
    /// non-`Send`+`Sync`-bounded handle; we mark this `Send + Sync`
    /// manually with the safety contract that the view's layer
    /// lifetime outlives the surface (which the framework's
    /// `release_graphics` enforces by dropping the IosNode → the
    /// underlying NSView → wgpu Surface owned by the surface's
    /// `Drop` impl in order).
    view: *mut std::ffi::c_void,
}

unsafe impl Send for MacosSurfaceProvider {}
unsafe impl Sync for MacosSurfaceProvider {}

impl HasWindowHandle for MacosSurfaceProvider {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = AppKitWindowHandle::new(
            NonNull::new(self.view).expect("null NSView pointer"),
        );
        Ok(unsafe { WindowHandle::borrow_raw(handle.into()) })
    }
}

impl HasDisplayHandle for MacosSurfaceProvider {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(AppKitDisplayHandle::new().into()) })
    }
}

// =========================================================================
// Public entry point
// =========================================================================

pub(crate) fn create_graphics(
    mtm: MainThreadMarker,
    callback_targets: &mut Vec<Retained<NSObject>>,
    on_ready: OnReady,
    _on_resize: OnResize,
    _on_lost: OnLost,
) -> MacosNode {
    // Build the Metal-backed view + flip its layer on.
    let metal_view = MetalView::new(mtm);
    let view: Retained<NSView> = unsafe {
        Retained::cast(metal_view)
    };
    let _: () = unsafe { msg_send![&view, setWantsLayer: true] };

    // Match the NSView backing-store density to the screen.
    // CAMetalLayer's contentsScale controls how many physical
    // pixels back each point. Without this, retina displays
    // render the surface at 1× and look blurry.
    let screen_scale: CGFloat = unsafe {
        let screen_cls: &AnyClass = AnyClass::get("NSScreen")
            .expect("NSScreen class not registered");
        let main_screen: *mut AnyObject = msg_send![screen_cls, mainScreen];
        if main_screen.is_null() {
            1.0
        } else {
            msg_send![main_screen, backingScaleFactor]
        }
    };
    let layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
    let _: () = unsafe { msg_send![&layer, setContentsScale: screen_scale] };

    // Build the surface provider — wgpu reads through this to
    // create an NSView-pointed Metal Surface.
    let view_ptr = &*view as *const NSView as *mut std::ffi::c_void;
    let provider = Arc::new(MacosSurfaceProvider { view: view_ptr });
    let surface = GraphicsSurface::new(provider);

    // Defer the on_ready callback to the next runloop turn — the
    // view's frame isn't known until AppKit has laid it out at
    // least once. Same pattern as iOS: NSTimer 0-delay via
    // `performSelector:withObject:afterDelay:` (a.k.a. the
    // run-loop's main-queue dispatch). The CallbackTarget bridges
    // a Rust `Fn` to an Obj-C `-(IBAction)invoke` selector.
    let view_clone = view.clone();
    let on_ready_cell: Rc<RefCell<Option<OnReady>>> = Rc::new(RefCell::new(Some(on_ready)));
    let ready_callback: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(mut cb) = on_ready_cell.borrow_mut().take() {
            let frame: CGRect = unsafe { msg_send![&view_clone, frame] };
            let scale: CGFloat = unsafe {
                let layer: Retained<NSObject> = msg_send_id![&view_clone, layer];
                msg_send![&layer, contentsScale]
            };
            let w = (frame.size.width * scale).max(1.0) as u32;
            let h = (frame.size.height * scale).max(1.0) as u32;
            cb(OnReadyEvent {
                surface: surface.clone(),
                size: (w, h),
            });
        }
    });
    let target = CallbackTarget::new(mtm, ready_callback);
    // `invoke:` (with the colon — macOS's CallbackTarget method
    // takes a sender arg, distinguishing it from the iOS variant's
    // `invoke` no-arg shape). performSelector:withObject:afterDelay:
    // passes the `withObject:` as the sender arg.
    let sel = objc2::sel!(invoke:);
    let _: () = unsafe {
        msg_send![
            &target,
            performSelector: sel,
            withObject: std::ptr::null::<NSObject>(),
            afterDelay: 0.0 as CGFloat
        ]
    };
    // Retain the target so AppKit can dispatch the deferred call.
    // The framework eventually drops the GraphicsHandle, which
    // drops this Vec entry, which drops the Retained, which lets
    // the Obj-C release count fall to zero.
    let obj: Retained<NSObject> = unsafe {
        Retained::cast::<NSObject>(target)
    };
    callback_targets.push(obj);

    MacosNode::View(view)
}

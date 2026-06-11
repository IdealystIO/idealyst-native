use runtime_core::primitives::graphics::{
    GraphicsSurface, OnLost, OnReady, OnReadyEvent, OnResize,
};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, CGRect, MainThreadMarker, NSObject};
use objc2_ui_kit::{UIColor, UIView};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle,
    UiKitDisplayHandle, UiKitWindowHandle, WindowHandle,
};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use super::callbacks::{CallbackTarget, MetalView};
use super::IosNode;

/// raw_window_handle bridge for wgpu.
struct IosSurfaceProvider {
    /// Non-owning pointer to the backing `UIView` (a `MetalView`). The
    /// owning `Retained<UIView>` lives on the returned `IosNode` (and is
    /// also captured by the ready callback), which is kept alive for at
    /// least as long as the `GraphicsSurface` built from this provider —
    /// so the pointer never dangles while wgpu holds the surface.
    view: *mut std::ffi::c_void,
}

// SAFETY: wgpu may read the surface provider from another thread when
// creating/recreating the surface, but all this type exposes is the raw
// `UIView` pointer for `raw-window-handle`; it never mutates the UIView
// or sends ObjC messages off the main thread. The pointer's validity is
// guaranteed by the owning `Retained<UIView>` outliving the surface (see
// the field doc above). UIKit object *construction* and mutation stay on
// the main thread in `create_graphics`.
unsafe impl Send for IosSurfaceProvider {}
unsafe impl Sync for IosSurfaceProvider {}

impl HasWindowHandle for IosSurfaceProvider {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = UiKitWindowHandle::new(
            NonNull::new(self.view).expect("null UIView pointer"),
        );
        Ok(unsafe { WindowHandle::borrow_raw(handle.into()) })
    }
}

impl HasDisplayHandle for IosSurfaceProvider {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(UiKitDisplayHandle::new().into()) })
    }
}

pub(crate) fn create_graphics(
    mtm: MainThreadMarker,
    callback_targets: &mut Vec<Retained<NSObject>>,
    on_ready: OnReady,
    _on_resize: OnResize,
    _on_lost: OnLost,
) -> IosNode {
    let metal_view = MetalView::new(mtm);
    let view: Retained<UIView> = Retained::into_super(metal_view);

    let clear = unsafe { UIColor::clearColor() };
    view.setBackgroundColor(Some(&clear));
    let _: () = unsafe { msg_send![&view, setOpaque: false] };

    let layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
    let screen_scale: CGFloat = unsafe {
        let screen: Retained<NSObject> = msg_send_id![objc2::class!(UIScreen), mainScreen];
        msg_send![&screen, scale]
    };
    let _: () = unsafe { msg_send![&layer, setContentsScale: screen_scale] };

    let view_ptr = &*view as *const UIView as *mut std::ffi::c_void;
    let provider = Arc::new(IosSurfaceProvider { view: view_ptr });
    let surface = GraphicsSurface::new(provider);

    let view_clone = view.clone();
    let on_ready_cell: Rc<RefCell<Option<OnReady>>> = Rc::new(RefCell::new(Some(on_ready)));
    let ready_callback: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(mut cb) = on_ready_cell.borrow_mut().take() {
            let frame: CGRect = unsafe { msg_send![&view_clone, frame] };
            let scale: CGFloat = unsafe { msg_send![&view_clone, contentScaleFactor] };
            let w = (frame.size.width * scale).max(1.0) as u32;
            let h = (frame.size.height * scale).max(1.0) as u32;
            eprintln!("[ios-backend] create_graphics on_ready firing: {}x{} (frame: {}x{}, scale: {})", w, h, frame.size.width, frame.size.height, scale);
            cb(OnReadyEvent {
                surface: surface.clone(),
                size: (w, h),
                // iOS rides canvas-native (no vello yet); 1.0 until iOS GPU canvas.
                scale: 1.0,
            });
            eprintln!("[ios-backend] on_ready callback returned");
        }
    });
    let target = CallbackTarget::new(mtm, ready_callback);
    let sel = objc2::sel!(invoke);
    let _: () = unsafe {
        msg_send![&target, performSelector: sel, withObject: std::ptr::null::<NSObject>(), afterDelay: 0.0 as CGFloat]
    };
    // Retain the target
    let obj: Retained<NSObject> = unsafe {
        let ptr = Retained::as_ptr(&target) as *mut NSObject;
        Retained::retain(ptr).unwrap()
    };
    callback_targets.push(obj);

    IosNode::View(view)
}

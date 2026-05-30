use runtime_core::primitives::graphics::{
    GraphicsSurface, OnLost, OnReady, OnResize,
};
use objc2::rc::Retained;
use objc2::{msg_send, msg_send_id};
use objc2_foundation::{CGFloat, MainThreadMarker, NSObject};
use objc2_ui_kit::{UIColor, UIView};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle,
    UiKitDisplayHandle, UiKitWindowHandle, WindowHandle,
};
use std::ptr::NonNull;
use std::sync::Arc;

use super::callbacks::MetalView;
use super::IosNode;

/// raw_window_handle bridge for wgpu.
struct IosSurfaceProvider {
    view: *mut std::ffi::c_void,
}

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
    _callback_targets: &mut Vec<Retained<NSObject>>,
    on_ready: OnReady,
    on_resize: OnResize,
    on_lost: OnLost,
) -> IosNode {
    let metal_view = MetalView::new(mtm);

    let clear = unsafe { UIColor::clearColor() };
    metal_view.setBackgroundColor(Some(&clear));
    let _: () = unsafe { msg_send![&metal_view, setOpaque: false] };

    let layer: Retained<NSObject> = unsafe { msg_send_id![&metal_view, layer] };
    let screen_scale: CGFloat = unsafe {
        let screen: Retained<NSObject> = msg_send_id![objc2::class!(UIScreen), mainScreen];
        msg_send![&screen, scale]
    };
    let _: () = unsafe { msg_send![&layer, setContentsScale: screen_scale] };

    let view_ptr = &*metal_view as *const MetalView as *const UIView as *mut std::ffi::c_void;
    let provider = Arc::new(IosSurfaceProvider { view: view_ptr });
    let surface = GraphicsSurface::new(provider);

    // All three callbacks live on the `MetalView` ivars (see
    // `imp::callbacks::MetalViewIvars`). The view's overridden
    // `layoutSubviews` fires `on_ready` on first non-zero bounds and
    // `on_resize` on subsequent size changes; the overridden
    // `willMoveToSuperview:` fires `on_lost` when the view is
    // removed from its parent. Together they replace the previous
    // one-shot `performSelector:withDelay:0` → `Option::take()`
    // shape that consumed `on_ready` (and dropped its captures)
    // after first fire, which was the cause of the leaked-Rc
    // keepalive the website's `Simulator` needs to mount on iOS.
    metal_view.install_callbacks(on_ready, on_resize, on_lost, surface);

    IosNode::View(Retained::into_super(metal_view))
}

//! iOS entry point for the shared `hello` app.
//!
//! The Swift host calls `ios_main(root_view)` once from
//! `viewDidLoad`. This builds an `IosBackend` and calls
//! `framework_core::render` with the shared `hello::app()` tree
//! underneath the provided root UIView.
//!
//! The returned `Owner` is stashed in a thread-local so it lives for
//! the duration of the app. Dropping it tears the tree down.

#![cfg(target_os = "ios")]

use backend_ios::IosBackend;
use objc2::rc::Retained;
use objc2_foundation::MainThreadMarker;
use objc2_ui_kit::UIView;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    /// Holds the framework's `Owner` for the active mount.
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
}

/// C-exported entry point called by the Swift host.
///
/// `root_view` must be a valid pointer to a `UIView` that the framework
/// will populate with its view tree. The caller retains ownership of
/// the root view; the framework adds subviews to it.
///
/// # Safety
/// - Must be called on the main thread.
/// - `root_view` must be a non-null, valid `UIView *`.
#[no_mangle]
pub unsafe extern "C" fn ios_main(root_view: *mut std::ffi::c_void) {
    // Install a panic hook that prints to stderr so the Xcode console
    // shows the message before abort.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("RUST PANIC: {}", info);
    }));

    // Safety: this function's contract requires it to be called on the
    // main thread — the Swift host calls it from viewDidLoad.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };

    // Convert the raw pointer into a Retained<UIView>. The Swift side
    // keeps its own strong reference, so we use `retain` (not `from_raw`)
    // to bump the ref count rather than taking ownership.
    let view: Retained<UIView> = unsafe {
        Retained::retain(root_view as *mut UIView)
            .expect("ios_main: root_view must be non-null")
    };

    // Tear down any previous mount.
    OWNER.with(|slot| slot.borrow_mut().take());

    let mut backend = IosBackend::new(mtm);
    backend.set_host_root(view);
    let backend = Rc::new(RefCell::new(backend));

    let owner = framework_core::render(backend, hello::app());

    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}

/// Tear down the active mount.
#[no_mangle]
pub unsafe extern "C" fn ios_teardown() {
    OWNER.with(|slot| slot.borrow_mut().take());
}

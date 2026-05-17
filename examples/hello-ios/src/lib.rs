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

    /// Holds the robot bridge handle (when robot feature is on).
    #[cfg(feature = "robot")]
    static ROBOT_BRIDGE: RefCell<Option<framework_core::robot::bridge::BridgeHandle>> = const { RefCell::new(None) };
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

    // Robot: start stdio capture FIRST so every subsequent
    // eprintln/println/NSLog is recorded into the in-memory log
    // ring buffer. The Xcode console still gets the bytes too —
    // the capture mirrors back to the original fds.
    #[cfg(feature = "robot")]
    {
        framework_core::robot::logs::start_stdio_capture();
        framework_core::robot::logs::push("ios", "ios_main starting");
    }

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
    // Install a global weak self-ref so the navigator dispatch
    // closures can re-run layout after pushes/replaces/selects
    // (they don't otherwise capture the backend).
    backend_ios::install_global_self(Rc::downgrade(&backend));

    let owner = framework_core::render(backend, hello::app());

    // Dump initial render profiler stats
    #[cfg(feature = "debug-stats")]
    {
        extern "C" { fn NSLog(fmt: *const objc2_foundation::NSString, ...); }
        let fmt = objc2_foundation::NSString::from_str("%@");

        let events = framework_core::debug::take_events();
        let summary = framework_core::debug::component_summary(&events);
        let msg = objc2_foundation::NSString::from_str(
            &format!("[profiler] Initial render: {} events, {} components", events.len(), summary.len())
        );
        unsafe { NSLog(&*fmt, &*msg) };

        for (name, stats) in &summary {
            let msg = objc2_foundation::NSString::from_str(
                &format!("[profiler]   {} — calls: {}, total: {}µs, max: {}µs",
                    name, stats.call_count, stats.total_inclusive_us, stats.max_inclusive_us)
            );
            unsafe { NSLog(&*fmt, &*msg) };
        }

        let counters = framework_core::debug::take_phase_counters();
        for (phase, counter) in &counters {
            let msg = objc2_foundation::NSString::from_str(
                &format!("[profiler]   phase {} — calls: {}, total: {}µs, max: {}µs",
                    phase, counter.call_count, counter.total_us, counter.max_us)
            );
            unsafe { NSLog(&*fmt, &*msg) };
        }
    }

    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));

    // Robot: start the bridge TCP listener and a polling timer.
    #[cfg(feature = "robot")]
    {
        use framework_core::robot::bridge;
        let handle = bridge::start(bridge::DEFAULT_PORT);
        ROBOT_BRIDGE.with(|slot| *slot.borrow_mut() = Some(handle));
        robot_start_poll_timer();
    }
}

/// Tear down the active mount.
#[no_mangle]
pub unsafe extern "C" fn ios_teardown() {
    OWNER.with(|slot| slot.borrow_mut().take());
}

/// Start a background thread that polls for robot commands every 50ms
/// and dispatches them on the main thread via GCD.
#[cfg(feature = "robot")]
fn robot_start_poll_timer() {
    extern "C" {
        // The actual exported symbol for the main queue.
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }

    extern "C" fn do_poll(_ctx: *mut std::ffi::c_void) {
        // This runs on the main thread, where the Robot registry lives.
        robot_poll_commands();
    }

    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        unsafe {
            dispatch_async_f(
                &_dispatch_main_q as *const _ as *const std::ffi::c_void,
                std::ptr::null_mut(),
                do_poll,
            );
        }
    });
}

/// Drain pending robot commands and execute them on the UI thread.
#[cfg(feature = "robot")]
fn robot_poll_commands() {
    ROBOT_BRIDGE.with(|slot| {
        if let Some(ref handle) = *slot.borrow() {
            handle.poll();
        }
    });
}

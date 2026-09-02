//! Pre-commit hook: run deferred work at the end of the current runloop
//! turn but **before** Core Animation commits the frame.
//!
//! # Why this exists (the bug prevented)
//!
//! Backend work that must land before the next paint is normally
//! deferred with `dispatch_async(main_queue, …)` — the canonical "do
//! this on the next main-thread turn". That is the right tool for
//! *eventually*, and the wrong one for *before the user sees
//! anything*: main-queue blocks are drained by the runloop as a
//! source, and the runloop commits the CoreAnimation transaction in a
//! `kCFRunLoopBeforeWaiting` observer that fires **first**. So a
//! dispatched block always paints one frame late.
//!
//! For most deferred work one frame is invisible. For the iOS layout
//! pass it is not: a screen mounted by a navigator push is handed to
//! UIKit with every child still at its parent's origin (nothing has
//! run Taffy over it yet), so the pushed transition's opening frames
//! render the whole screen collapsed into the top-left corner. Icons
//! are what you actually see — an icon's CALayer carries its own 24x24
//! bounds, so it draws at full size inside a zero-size view, while a
//! zero-size label draws nothing.
//!
//! Registering our own observer at a **lower order** than CoreAnimation's
//! commit observer closes the gap: the work runs in the same turn that
//! scheduled it, and the frames it writes are part of the very first
//! commit that shows the new views.
//!
//! # This does not replace the dispatch backstop
//!
//! An observer only fires while the runloop is turning. Work scheduled
//! from *inside* the commit phase — UIKit's `layoutSubviews` is the one
//! that matters here, which is where rotation reports a new viewport —
//! arrives after this turn's observer has already run, and nothing
//! would wake the runloop to fire the next one. Callers therefore keep
//! their `dispatch_async` as the guarantee of progress and treat this
//! hook as the opportunistic early drain. Both paths funnel into one
//! idempotent, coalesced drain function; whichever gets there first
//! does the work.

use std::cell::Cell;

thread_local! {
    /// The work to run before each commit. Single slot, like
    /// [`crate::dispatch_hook`] — one backend, one drain function.
    static HOOK: Cell<Option<fn()>> = const { Cell::new(None) };
    /// Whether the runloop observer has been created and added. The
    /// observer is permanent (it lives as long as the process) and
    /// costs one `Cell` read per runloop turn when no hook is set.
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Install `f` as the pre-commit hook and register the runloop
/// observer that fires it (idempotent — later calls just replace the
/// function).
///
/// Returns `false` without installing anything when called off the
/// main runloop: the observer's callout runs on the main thread and
/// reads a main-thread-local slot, so an off-main install would set a
/// slot that never fires and silently disable the caller's own
/// fallback. Callers that can run on any thread must keep their
/// `dispatch_async` path regardless of this return value.
pub fn install_pre_commit_hook(f: fn()) -> bool {
    if !on_main_runloop() {
        return false;
    }
    set_hook(f);
    if claim_observer_slot() {
        install_observer();
    }
    true
}

/// Set the hook slot. Split out from [`install_pre_commit_hook`] so the
/// slot semantics can be exercised without a main runloop — libtest
/// runs every test on a worker thread, where the install is (correctly)
/// refused.
fn set_hook(f: fn()) {
    HOOK.with(|h| h.set(Some(f)));
}

/// Claim the right to create the observer, returning `true` to exactly
/// one caller. A second observer would run the drain twice per turn for
/// the life of the process.
fn claim_observer_slot() -> bool {
    !INSTALLED.with(|i| i.replace(true))
}

/// Remove the hook. The observer stays registered and becomes a no-op —
/// CFRunLoopObserver removal is not worth the bookkeeping for a slot
/// that is installed once at boot.
pub fn clear_pre_commit_hook() {
    HOOK.with(|h| h.set(None));
}

/// Fire the hook if one is installed. Separate from the callout so the
/// slot semantics are testable without a running runloop.
fn fire() {
    if let Some(f) = HOOK.with(|h| h.get()) {
        f();
    }
}

// ===========================================================================
// CoreFoundation runloop-observer FFI
// ===========================================================================

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
mod ffi {
    use std::ffi::c_void;

    pub type CFRunLoopRef = *mut c_void;
    pub type CFRunLoopObserverRef = *mut c_void;
    pub type CFStringRef = *const c_void;
    pub type CFAllocatorRef = *const c_void;
    pub type CFIndex = isize;
    pub type CFOptionFlags = usize;
    pub type Boolean = u8;

    pub const K_CF_RUN_LOOP_BEFORE_WAITING: CFOptionFlags = 1 << 5;
    pub const K_CF_RUN_LOOP_EXIT: CFOptionFlags = 1 << 7;

    /// CoreAnimation registers its commit observer on the main runloop
    /// at order 2_000_000, for `BeforeWaiting | Exit`. Observers run in
    /// ascending order, so anything below that number runs while the
    /// transaction is still open and its writes land in the same
    /// commit. The margin is arbitrary — it just needs to stay under
    /// CoreAnimation's number, and leaves room for a later hook that
    /// wants to run either side of ours.
    pub const CA_COMMIT_ORDER: CFIndex = 2_000_000;
    pub const PRE_COMMIT_ORDER: CFIndex = CA_COMMIT_ORDER - 1_000;

    #[repr(C)]
    pub struct CFRunLoopObserverContext {
        pub version: CFIndex,
        pub info: *mut c_void,
        pub retain: Option<extern "C" fn(*const c_void) -> *const c_void>,
        pub release: Option<extern "C" fn(*const c_void)>,
        pub copy_description: Option<extern "C" fn(*const c_void) -> CFStringRef>,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub static kCFRunLoopCommonModes: CFStringRef;
        pub fn CFRunLoopGetMain() -> CFRunLoopRef;
        pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        pub fn CFRunLoopObserverCreate(
            allocator: CFAllocatorRef,
            activities: CFOptionFlags,
            repeats: Boolean,
            order: CFIndex,
            callout: extern "C" fn(CFRunLoopObserverRef, CFOptionFlags, *mut c_void),
            context: *mut CFRunLoopObserverContext,
        ) -> CFRunLoopObserverRef;
        pub fn CFRunLoopAddObserver(
            rl: CFRunLoopRef,
            observer: CFRunLoopObserverRef,
            mode: CFStringRef,
        );
    }
}

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
fn on_main_runloop() -> bool {
    unsafe { ffi::CFRunLoopGetCurrent() == ffi::CFRunLoopGetMain() }
}

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
fn install_observer() {
    use std::ffi::c_void;

    extern "C" fn callout(
        _observer: ffi::CFRunLoopObserverRef,
        _activity: ffi::CFOptionFlags,
        _info: *mut c_void,
    ) {
        // CoreFoundation is C and a Rust panic unwinding back into it
        // is undefined behavior. `catch_unwind` here only buys us a
        // readable message before we abort — crash-loud is the project
        // policy, same as the layout-pass dispatch trampoline.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(fire));
        if let Err(payload) = result {
            let msg = if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = payload.downcast_ref::<&'static str>() {
                (*s).to_string()
            } else {
                "<non-string panic payload>".to_string()
            };
            eprintln!("[backend-apple-core] pre-commit hook panic: {msg}");
            std::process::abort();
        }
    }

    // Observe both activities CoreAnimation observes: `BeforeWaiting`
    // is the normal end of a turn, `Exit` covers a nested runloop
    // unwinding (a modal alert, a tracking loop) which also commits.
    let activities = ffi::K_CF_RUN_LOOP_BEFORE_WAITING | ffi::K_CF_RUN_LOOP_EXIT;
    let mut context = ffi::CFRunLoopObserverContext {
        version: 0,
        info: std::ptr::null_mut(),
        retain: None,
        release: None,
        copy_description: None,
    };
    unsafe {
        let observer = ffi::CFRunLoopObserverCreate(
            std::ptr::null(),
            activities,
            1, // repeats — this fires for the life of the process
            ffi::PRE_COMMIT_ORDER,
            callout,
            &mut context,
        );
        if observer.is_null() {
            // Nothing to do but leave the caller on its dispatch
            // fallback; that path is still correct, just a frame late.
            return;
        }
        ffi::CFRunLoopAddObserver(ffi::CFRunLoopGetMain(), observer, ffi::kCFRunLoopCommonModes);
        // Deliberately leaked: the observer must outlive this call and
        // is never removed. One allocation for the life of the process.
    }
}

// Non-Apple hosts (the crate's pure-`std` modules are compiled there
// for unit tests) get a slot that never fires.
#[cfg(not(any(target_os = "ios", target_os = "tvos", target_os = "macos")))]
fn on_main_runloop() -> bool {
    true
}

#[cfg(not(any(target_os = "ios", target_os = "tvos", target_os = "macos")))]
fn install_observer() {}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static FIRED: Cell<u32> = const { Cell::new(0) };
    }

    fn bump() {
        FIRED.with(|f| f.set(f.get() + 1));
    }

    /// Slot semantics, mirroring [`crate::dispatch_hook`]'s: silent
    /// before install, fires after, silent again once cleared.
    #[test]
    fn hook_fires_only_between_install_and_clear() {
        clear_pre_commit_hook();
        FIRED.with(|f| f.set(0));

        fire();
        assert_eq!(FIRED.with(|f| f.get()), 0, "no-op before install");

        set_hook(bump);
        fire();
        fire();
        assert_eq!(FIRED.with(|f| f.get()), 2, "fires once per call");

        clear_pre_commit_hook();
        fire();
        assert_eq!(FIRED.with(|f| f.get()), 2, "silent after clear");
    }

    /// Exactly one caller may create the observer, however many times
    /// the hook is installed.
    #[test]
    fn only_the_first_caller_creates_the_observer() {
        INSTALLED.with(|i| i.set(false));

        assert!(claim_observer_slot(), "first caller creates it");
        assert!(!claim_observer_slot(), "second caller must not");
        assert!(!claim_observer_slot(), "and neither does any later one");
    }

    /// Off the main runloop the install is refused outright, so the
    /// caller keeps its own dispatch fallback rather than handing work
    /// to a slot that would never fire. libtest gives us a worker
    /// thread, which is exactly that case.
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    #[test]
    fn install_is_refused_off_the_main_runloop() {
        assert!(!on_main_runloop(), "libtest runs tests on a worker thread");
        assert!(
            !install_pre_commit_hook(bump),
            "refused, and the caller is told so"
        );
    }

    /// The whole point of the module: our observer must sort ahead of
    /// CoreAnimation's commit, or the work still lands a frame late.
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
    #[test]
    fn pre_commit_order_beats_core_animations_commit() {
        assert!(
            ffi::PRE_COMMIT_ORDER < ffi::CA_COMMIT_ORDER,
            "observers run in ascending order; ours must precede the commit"
        );
    }
}

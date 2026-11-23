//! macOS: an IOKit power assertion of type `PreventUserIdleDisplaySleep`
//! (listed by `pmset -g assertions`). The system drops a process's
//! assertions when it exits.

use std::cell::Cell;
use std::ffi::c_void;

use objc2_foundation::NSString;

type AssertionId = u32;

/// `kIOPMAssertionLevelOn` (`IOPMLib.h`).
const LEVEL_ON: u32 = 255;
/// `kIOPMNullAssertionID`.
const NULL_ASSERTION: AssertionId = 0;
/// `kIOReturnSuccess`.
const SUCCESS: i32 = 0;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(
        kind: *const c_void,
        level: u32,
        name: *const c_void,
        id: *mut AssertionId,
    ) -> i32;
    fn IOPMAssertionRelease(id: AssertionId) -> i32;
}

runtime_tls::thread_local! {
    static HELD: Cell<AssertionId> = const { Cell::new(NULL_ASSERTION) };
}

pub(crate) fn set(on: bool) {
    let held = HELD.with(Cell::get);
    if on == (held != NULL_ASSERTION) {
        return;
    }
    if !on {
        unsafe { IOPMAssertionRelease(held) };
        HELD.with(|h| h.set(NULL_ASSERTION));
        return;
    }
    // `kIOPMAssertionTypePreventUserIdleDisplaySleep`. NSString is toll-free
    // bridged to the CFString the call takes; it copies both.
    let kind = NSString::from_str("PreventUserIdleDisplaySleep");
    let name = NSString::from_str(&crate::keep_awake_labels().reason);
    let mut id = NULL_ASSERTION;
    let made = unsafe {
        IOPMAssertionCreateWithName(
            (&*kind as *const NSString).cast(),
            LEVEL_ON,
            (&*name as *const NSString).cast(),
            &mut id,
        )
    };
    if made == SUCCESS {
        HELD.with(|h| h.set(id));
    }
}

pub(crate) fn is_supported() -> bool {
    true
}

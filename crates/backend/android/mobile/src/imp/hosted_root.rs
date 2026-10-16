//! Registry of stack-presenter screens laid out against their outlet
//! (screen view key → host view key). The decision of how they are
//! computed lives in `crate::hosted_root_policy`; this is only the
//! state, thread-local like `virtual_grid`'s `CELL_BOXES` because the
//! presenter reaches it without a `&mut AndroidBackend` in hand.

use std::cell::RefCell;
use std::collections::HashMap;

runtime_tls::thread_local! {
    static HOSTED: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
}

pub(crate) fn host(screen: usize, host: usize) {
    HOSTED.with(|m| m.borrow_mut().insert(screen, host));
}

pub(crate) fn unhost(screen: usize) {
    HOSTED.with(|m| m.borrow_mut().remove(&screen));
}

pub(crate) fn host_of(screen: usize) -> Option<usize> {
    HOSTED.with(|m| m.borrow().get(&screen).copied())
}

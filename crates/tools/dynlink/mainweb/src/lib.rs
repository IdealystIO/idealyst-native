// The ULTIMATE realistic proof: a REAL WebBackend main (the actual DOM
// renderer) hosts a non-bindgen side and mounts the side-built UI into the
// real DOM — exactly how Element::Lazy's web handler roots a WebBackend
// at the placeholder container.
use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use dynlink_shared::DYNLINK_COUNTER;
use runtime_core::signal;
use runtime_core::{text, view, IntoElement, Element};
use wasm_bindgen::prelude::*;

#[no_mangle]
pub extern "C" fn host_reserve(size: usize) -> *mut u8 {
    unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size, 16).unwrap()) }
}

// Reference the shared static so main DEFINES + exports it (--export-all only
// exports DEFINED symbols). The side imports `GOT.mem.DYNLINK_COUNTER`; if
// main never touches it, it isn't linked into main and the side can't resolve
// it. Principle for the real pipeline: main must reference any shared static
// the side imports (the core statics — ARENA/DLMALLOC/etc. — already are,
// since main runs the reactive system + walker).
#[no_mangle]
pub extern "C" fn main_bump() -> i32 {
    let c = &DYNLINK_COUNTER.0;
    c.set(c.get() + 1);
    c.get()
}

#[no_mangle]
pub extern "C" fn main_signal() -> i32 {
    let s = signal!(100i32);
    s.get()
}

thread_local! {
    // mount() / render() return Owners that must outlive the page.
    static OWNERS: RefCell<Vec<runtime_core::Owner>> = const { RefCell::new(Vec::new()) };
}

fn install_runtime() {
    backend_web::install_scheduler();
    backend_web::install_time_source();
}

// Boot the real WebBackend and mount an "always loaded" shell into #app.
#[wasm_bindgen]
pub fn boot() {
    install_runtime();
    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    backend_web::install_global_self(&backend);
    let owner = runtime_core::mount(backend, || {
        view(vec![text("main shell (always loaded)").into_element()]).into_element()
    });
    OWNERS.with(|o| o.borrow_mut().push(owner));
}

// Mount a side-built Element into #lazy-slot via a REAL WebBackend rooted
// there — the same mechanism Element::Lazy uses on web.
#[no_mangle]
pub extern "C" fn main_mount_side(ptr: *mut Element) -> i32 {
    let p = unsafe { *Box::from_raw(ptr) };
    let backend = Rc::new(RefCell::new(WebBackend::new("#lazy-slot")));
    backend_web::install_global_self(&backend);
    let owner = runtime_core::render(backend, p);
    OWNERS.with(|o| o.borrow_mut().push(owner));
    1
}

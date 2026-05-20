//! Web entry — wasm-bindgen `start()` mounts the maps-test app and
//! installs the `maps` SDK against the `WebBackend`.

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use wasm_bindgen::prelude::*;

thread_local! {
    static OWNER: RefCell<Option<framework_core::Owner>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    backend_web::install_scheduler();
    backend_web::install_time_source();

    super::install();

    let mut backend = WebBackend::new("#app");
    // ONE line, regardless of how many backends `maps` ends up
    // supporting in the future. The umbrella's cfg-routed re-export
    // selects the web leaf at compile time.
    maps::register(&mut backend);

    let owner = framework_core::render(Rc::new(RefCell::new(backend)), super::app());
    OWNER.with(|slot| *slot.borrow_mut() = Some(owner));
}

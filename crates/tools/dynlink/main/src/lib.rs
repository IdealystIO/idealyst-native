use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::icon::IconData;
use runtime_core::signal;
use runtime_core::{Action, Backend, Platform, Element, StyleRules};
use dynlink_shared::DYNLINK_COUNTER;
#[no_mangle] pub extern "C" fn main_bump() -> i32 { let c=&DYNLINK_COUNTER.0; c.set(c.get()+1); c.get() }
#[no_mangle] pub extern "C" fn main_read() -> i32 { DYNLINK_COUNTER.0.get() }
// pulls the reactive system (ARENA etc.) into main so it DEFINES those statics
#[no_mangle] pub extern "C" fn main_signal() -> i32 { let s = signal!(100i32); s.get() }

// Reserve a region for a side module FROM main's heap, so DLMALLOC won't
// hand the same bytes to a later allocation. Loader uses it as __memory_base.
#[no_mangle] pub extern "C" fn host_reserve(size: usize) -> *mut u8 {
    unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size, 16).unwrap()) }
}

// Minimal Backend that counts create_view / create_text and sums text
// bytes. Enough to prove the walker mounts a side-built Element; it
// pulls the full walker (build dispatcher, scope/effect machinery) into
// main, which is where every backend call happens in the real design.
#[derive(Default)]
struct CountBackend {
    next: u64,
    views: u32,
    texts: u32,
    text_len: u32,
}
impl CountBackend {
    fn mint(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }
}
impl Backend for CountBackend {
    type Node = u64;
    // Report Web so `mount` skips installing the native `InstantTimeSource`
    // (`Instant::now()` panics on wasm32). The real web backend reports
    // Web for the same reason; it wires a `performance.now()` source at
    // bootstrap, which a static render never reads anyway.
    fn platform(&self) -> Platform {
        Platform::Web
    }
    fn create_view(&mut self, _a: &AccessibilityProps) -> u64 {
        self.views += 1;
        self.mint()
    }
    fn create_text(&mut self, content: &str, _a: &AccessibilityProps) -> u64 {
        self.texts += 1;
        self.text_len += content.len() as u32;
        self.mint()
    }
    fn create_button(
        &mut self,
        _label: &str,
        _on_click: &Action,
        _leading: Option<&IconData>,
        _trailing: Option<&IconData>,
        _a: &AccessibilityProps,
    ) -> u64 {
        self.mint()
    }
    fn insert(&mut self, _parent: &mut u64, _child: u64) {}
    fn update_text(&mut self, _node: &u64, _content: &str) {}
    fn clear_children(&mut self, _node: &u64) {}
    fn apply_style(&mut self, _node: &u64, _style: &Rc<StyleRules>) {}
    fn finish(&mut self, _root: u64) {}
}

// Take a `Element` the SIDE module built (on the shared heap) and mount
// it through main's real walker. Returns `texts * 1000 + total_text_bytes`
// so the JS harness can verify the side's UI actually reached the backend.
#[no_mangle]
pub extern "C" fn main_render_side(ptr: *mut Element) -> i32 {
    let primitive = unsafe { *Box::from_raw(ptr) };
    let backend = Rc::new(RefCell::new(CountBackend::default()));
    let owner = runtime_core::render(backend.clone(), primitive);
    // Keep the reactive scope alive (we only want the counts); leaking is
    // fine for a one-shot proof.
    core::mem::forget(owner);
    let b = backend.borrow();
    (b.texts as i32) * 1000 + b.text_len as i32
}

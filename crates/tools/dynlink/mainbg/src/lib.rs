// Bindgen variant of the dynamic-link MAIN — the realistic web config:
// the real app's main is wasm-bindgen-based (WebBackend drives the DOM via
// bindgen). This proves a wasm-bindgen-PROCESSED, PIC main still works as
// the dynamic-link host: its GOT.mem statics + functions survive bindgen as
// exports, a non-bindgen side links against it (0 unresolved), and main's
// walker mounts a side-built Primitive.
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::icon::IconData;
use runtime_core::signal;
use runtime_core::{Action, Backend, Platform, Primitive, StyleRules};
use dynlink_shared::DYNLINK_COUNTER;
use wasm_bindgen::prelude::*;

// The bindgen item that makes this a genuine wasm-bindgen module (without
// at least one, wasm-bindgen refuses: "failed to find intrinsics").
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[wasm_bindgen]
pub fn mb_log_touch() {
    log("mainbg: bindgen call live");
}

#[no_mangle]
pub extern "C" fn main_bump() -> i32 {
    let c = &DYNLINK_COUNTER.0;
    c.set(c.get() + 1);
    c.get()
}
#[no_mangle]
pub extern "C" fn main_read() -> i32 {
    DYNLINK_COUNTER.0.get()
}
#[no_mangle]
pub extern "C" fn main_signal() -> i32 {
    let s = signal!(100i32);
    s.get()
}

#[no_mangle]
pub extern "C" fn host_reserve(size: usize) -> *mut u8 {
    unsafe { std::alloc::alloc(std::alloc::Layout::from_size_align(size, 16).unwrap()) }
}

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

#[no_mangle]
pub extern "C" fn main_render_side(ptr: *mut Primitive) -> i32 {
    let primitive = unsafe { *Box::from_raw(ptr) };
    let backend = Rc::new(RefCell::new(CountBackend::default()));
    let owner = runtime_core::render(backend.clone(), primitive);
    core::mem::forget(owner);
    let b = backend.borrow();
    (b.texts as i32) * 1000 + b.text_len as i32
}

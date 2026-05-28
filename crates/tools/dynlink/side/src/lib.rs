use runtime_core::signal;
use runtime_core::{text, view, IntoElement, Element};
use dynlink_shared::DYNLINK_COUNTER;
// bumps the SAME counter as main (shared via GOT) by +10
#[no_mangle] pub extern "C" fn side_bump() -> i32 { let c=&DYNLINK_COUNTER.0; c.set(c.get()+10); c.get() }
// runs real reactive code in the side, using main's shared ARENA
#[no_mangle] pub extern "C" fn side_signal() -> i32 { let s = signal!(7i32); s.get() }

// The lazy-UI mechanic: the SIDE module constructs a `Element` (a View
// wrapping a Text) on the SHARED heap and hands main a raw pointer. main
// then mounts it through the real walker. No serialization — the side
// builds UI, main renders it, exactly as a `lazy!` body would.
//
// Text is built with `format!` to exercise the fmt path in a non-bindgen
// side (proven to work; the bindgen side spike is what hit the fmt bug).
#[no_mangle]
pub extern "C" fn side_make_view() -> *mut Element {
    let p: Element = view(vec![
        text(format!("hello from side #{}", 7)).into_element(),
    ])
    .into_element();
    Box::into_raw(Box::new(p))
}

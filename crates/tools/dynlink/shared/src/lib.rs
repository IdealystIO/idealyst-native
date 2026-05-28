#![no_std]
use core::cell::Cell;
pub struct Counter(pub Cell<i32>);
unsafe impl Sync for Counter {}
// One canonical instance; the loader points the side's GOT entry at main's copy.
#[no_mangle]
pub static DYNLINK_COUNTER: Counter = Counter(Cell::new(0));

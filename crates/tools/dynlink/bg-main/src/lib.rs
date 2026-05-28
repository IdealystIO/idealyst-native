// Plain std module: owns the heap + defines the std symbols (DLMALLOC, etc.)
// that the side imports via the GOT. No bindgen — the side has its own glue.
#[no_mangle]
pub extern "C" fn host_reserve(size: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(size);
    let p = v.as_mut_ptr();
    core::mem::forget(v);
    p
}
#[no_mangle]
pub extern "C" fn main_touch() -> i32 {
    // force a heap allocation so main's DLMALLOC is live before the side loads
    let v: Vec<i32> = (0..4).collect();
    v.iter().sum()
}

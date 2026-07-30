use runtime_core::Signal;

// An event handler runs BEFORE the flush, so every read inside it sees
// the value that was committed when the handler started.
fn handler(count: Signal<i32>) {
    let before = count.get(); // say 7
    count.set(before + 1); // stages 8
    assert_eq!(count.get(), before); // still 7 — reads never see a staged write
    count.set(count.get() + 1); // stages 8 again, not 9
} // the flush commits 8

// update() reads the STAGED value, so read-modify-write composes.
fn increment_twice(count: Signal<i32>) {
    count.update(|n| n + 1);
    count.update(|n| n + 1);
} // the flush commits +2

use runtime_core::{signal, Signal};

fn signal_basics() {
    let count: Signal<i32> = signal(0);

    let n = count.get(); // read the committed value
    count.set(n + 1); // STAGE a write
    count.update(|v| v + 1); // stage on top of the staged value

    let latest = count.peek(); // committed value, without subscribing
    let doubled = count.with(|v| v * 2); // borrowed read, no clone
}

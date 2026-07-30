use runtime_core::{effect, signal, Signal};

fn one_logical_update() {
    let first: Signal<i32> = signal(0);
    let second: Signal<i32> = signal(0);

    effect!({
        let sum = first.get() + second.get(); // subscribes to both
    });

    // One turn, two staged writes, ONE effect run at the flush. There is
    // no batch(..) wrapper to reach for: staging is the batching.
    first.set(1);
    second.set(2);
}

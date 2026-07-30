use runtime_core::{effect, signal, Signal};

fn subscribe_by_reading() {
    let count: Signal<i32> = signal(0);
    let verbose: Signal<bool> = signal(false);
    let label: Signal<String> = signal(String::new());

    effect!({
        // Dependencies are whatever THIS run reads. While `verbose` is
        // false the effect never reads `count`, so a write to `count`
        // does not wake it.
        if verbose.get() {
            label.set(format!("count is {}", count.get()));
        }
    });

    count.set(1); // stages; the flush wakes nothing
    verbose.set(true); // stages; the flush re-runs the effect, which now reads `count`
}

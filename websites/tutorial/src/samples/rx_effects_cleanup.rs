use runtime_core::{after_ms, effect, Signal};

fn polling(deps: Signal<i32>, tick: Signal<u32>) {
    // The `effect(..)` fn (rather than the `effect!` block macro) lets the
    // body RETURN a cleanup. It runs before the next re-run and again when
    // the owning scope drops, so the timer can never outlive the component.
    let _ = effect(move || {
        let _ = deps.get();
        let task = after_ms(500, move || tick.update(|n| n + 1));
        move || drop(task)
    });
}

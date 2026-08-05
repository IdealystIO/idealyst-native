use runtime_core::{effect, memo, signal, Memo, ReadSignal, Signal};

fn derived_state() {
    let count: Signal<i32> = signal(0);

    let doubled: Memo<i32> = memo(move || count.get() * 2);
    let is_big: Memo<bool> = memo(move || count.get() > 10);

    effect!({
        // Memos settle before reactions run, so this pair is always
        // consistent — never a fresh `count` beside a stale `doubled`.
        let pair = (count.get(), doubled.get());
    });
}

// A memo hands out the read half, so a signature can prove a component
// only ever observes the value.
fn observe_only(total: ReadSignal<i32>) -> i32 {
    total.get()
}

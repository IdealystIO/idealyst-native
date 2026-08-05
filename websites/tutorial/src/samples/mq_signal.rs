use runtime_core::{current_breakpoint, effect, Breakpoint, Signal};

fn choose_layout(stacked: Signal<bool>) {
    // Resolve the handle at build time (it needs the world), then read it
    // wherever you like. It re-fires when the BUCKET changes, so a
    // drag-resize inside one bucket costs nothing.
    let bucket = current_breakpoint();

    effect!({
        stacked.set(matches!(bucket.get(), Breakpoint::Xs | Breakpoint::Sm));
    });
}

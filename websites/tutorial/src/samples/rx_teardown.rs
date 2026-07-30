use runtime_core::{signal, Signal};

// Everything a component body creates — signals, effects, memos — is
// collected into that component's scope and freed when it unmounts.
// Dropping the scope IS the teardown; there is no dispose call.
fn component_local() -> Signal<i32> {
    signal(0)
}

// Through a handle that outlived its world: writes are silent no-ops, and
// reads panic with a stale-handle diagnostic so the leak surfaces instead
// of returning garbage.
fn after_teardown(leaked: Signal<i32>) {
    leaked.set(1); // no-op
    let stale = leaked.get(); // panics
}

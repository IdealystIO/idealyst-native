use runtime_core::Signal;

fn write_variants(flag: Signal<bool>) {
    flag.set(true); // stage; notify only if it commits DIFFERENT
    flag.set_always(true); // stage; notify even if it commits equal
    flag.touch(); // notify with no value write
    flag.set_untracked(true); // write the committed value, notify nobody
}

// The guard compares at COMMIT time, so a round trip inside one turn
// nets to zero and wakes nothing.
fn round_trip(name: Signal<String>) {
    name.set("b".to_string());
    name.set("a".to_string()); // if "a" was committed, subscribers stay asleep
}

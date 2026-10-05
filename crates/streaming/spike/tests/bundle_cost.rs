//! What the bundle side of a crossing costs, counted in wasm instructions
//! (wasmi's fuel) rather than time, so the bounds are exact on any machine.
//!
//! The bundle is instantiated here on its own, every import defined as a
//! trap: these are the bundle's own exports, which call no imports.

use stream_spike::REMOTE_ATTR_WASM;
use wasmi::{Config, Engine, ExternType, Linker, Module, Store};

fn instantiate() -> (Store<()>, wasmi::Instance) {
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, REMOTE_ATTR_WASM).expect("the bundle validates");
    let mut linker = Linker::new(&engine);
    for import in module.imports() {
        if let ExternType::Func(ty) = import.ty() {
            let name = format!("{}::{}", import.module(), import.name());
            linker
                .func_new(import.module(), import.name(), ty.clone(), move |_, _, _| {
                    Err(wasmi::Error::new(format!("{name} called")))
                })
                .unwrap();
        }
    }
    let mut store = Store::new(&engine, ());
    store.set_fuel(u64::MAX).unwrap();
    let instance = linker.instantiate_and_start(&mut store, &module).expect("instantiates");
    (store, instance)
}

/// Regression: the buffer the app writes a reply into
/// (`idealyst_ui_alloc`) was zero-filled with `Vec::resize`, which the
/// size-optimized bundle compiles to a loop storing one byte per iteration
/// — interpreted, on every call. That was nearly all of what receiving a
/// list cost: 5 ms per 100k `u32`s from a host function, against 0.4 ms
/// to send the same list. Now growing the buffer is one `memory.fill` and a
/// reused buffer isn't touched, so the instructions don't grow with the
/// size.
#[test]
fn regression_reserving_the_reply_buffer_does_not_touch_each_byte() {
    let (mut store, instance) = instantiate();
    let alloc = instance.get_typed_func::<u32, u32>(&store, "idealyst_ui_alloc").unwrap();
    let mut cost = |len: u32| {
        let before = store.get_fuel().unwrap();
        alloc.call(&mut store, len).unwrap();
        before - store.get_fuel().unwrap()
    };
    cost(16); // the allocator's first-call setup
    let small = cost(16);
    const BIG: u32 = 4 << 20;
    let grow = cost(BIG);
    let reuse = cost(BIG);
    // The byte loop ran ~26 instructions per byte (109 M for 4 MB). wasmi
    // charges the memory growth and the bulk `memory.fill` by size too
    // (~142 k for 4 MB), so growing still scales, at under 1/700 of that.
    // A reuse doesn't.
    assert!((grow - small) < u64::from(BIG) / 16, "growing to 4 MB ran {grow} instructions (16 bytes: {small})");
    assert!(reuse <= small, "reusing 4 MB ran {reuse} instructions (16 bytes: {small})");
}

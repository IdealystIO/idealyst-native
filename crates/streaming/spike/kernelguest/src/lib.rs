//! A bundle whose reactive state lives in the HOST's graph.
//!
//! Nothing here knows about bridging: it is ordinary runtime-world code.
//! Built as a bundle (`--cfg idealyst_stream_guest`), runtime-world's active
//! engine is the bridged one, so `signal()` allocates a slot in whatever
//! world the HOST has entered, `effect()` is a host effect whose body runs
//! here, and the host's `World::flush` drives this bundle's effects.
//!
//! Exports are the test surface `stream-spike/tests/kernel_bridge.rs` drives.

#![cfg(idealyst_stream_guest)]

use std::cell::{Cell, RefCell};

use runtime_world::remote_guest::{import_read_signal, import_signal, register_remote_context, Codec};
use runtime_world::{collect_owned, effect, inject, memo, on_cleanup, provide, signal, Memo, Owned, ReadSignal, Signal};
use stream_abi::Wire;

#[derive(Clone)]
struct Theme(i64);

thread_local! {
    static COUNT: Cell<Option<Signal<i64>>> = const { Cell::new(None) };
    static DOUBLED: Cell<Option<Memo<i64>>> = const { Cell::new(None) };
    static EFFECT_RUNS: Cell<i64> = const { Cell::new(0) };
    static EFFECT_SAW: Cell<i64> = const { Cell::new(-1) };
    static CLEANUPS: Cell<i64> = const { Cell::new(0) };
    static SCOPE: RefCell<Option<Owned>> = const { RefCell::new(None) };
}

fn count() -> Signal<i64> {
    COUNT.with(|c| c.get()).expect("kg_setup first")
}

/// Signal + memo + effect, created in the host's ambient world. The effect
/// runs once immediately — a host→bundle call made while this export is
/// still running (the re-entrant path).
#[no_mangle]
pub extern "C" fn kg_setup() {
    let count = signal(0i64);
    let doubled = memo(move || count.get() * 2);
    effect(move || {
        EFFECT_RUNS.with(|r| r.set(r.get() + 1));
        EFFECT_SAW.with(|s| s.set(doubled.get()));
    });
    COUNT.with(|c| c.set(Some(count)));
    DOUBLED.with(|d| d.set(Some(doubled)));
}

/// Staged write; nothing is visible until the HOST flushes.
#[no_mangle]
pub extern "C" fn kg_bump() {
    count().update(|c| c + 1);
}

#[no_mangle]
pub extern "C" fn kg_count() -> i64 {
    count().peek()
}

#[no_mangle]
pub extern "C" fn kg_doubled() -> i64 {
    DOUBLED.with(|d| d.get()).expect("kg_setup first").peek()
}

#[no_mangle]
pub extern "C" fn kg_effect_runs() -> i64 {
    EFFECT_RUNS.with(|r| r.get())
}

#[no_mangle]
pub extern "C" fn kg_effect_saw() -> i64 {
    EFFECT_SAW.with(|s| s.get())
}

/// The host world id `count` lives in (from its raw id's top byte).
#[no_mangle]
pub extern "C" fn kg_count_world() -> i64 {
    (count().raw_id() >> 56) as i64
}

/// A collected scope: a signal, an effect with a cleanup, and a context
/// provision, held here until `kg_scope_drop`.
#[no_mangle]
pub extern "C" fn kg_scope_open() -> i64 {
    let (seen, owned) = collect_owned(|| {
        let local = signal(10i64);
        effect(move || {
            let _ = local.get();
            on_cleanup(|| CLEANUPS.with(|c| c.set(c.get() + 1)));
        });
        provide(Theme(7));
        inject::<Theme>().map_or(-1, |t| t.0)
    });
    SCOPE.with(|s| *s.borrow_mut() = Some(owned));
    seen
}

#[no_mangle]
pub extern "C" fn kg_scope_len() -> i64 {
    SCOPE.with(|s| s.borrow().as_ref().map_or(-1, |o| o.len() as i64))
}

/// Drop the scope: its effect's cleanup runs, its context is retracted.
/// Returns what `inject::<Theme>()` sees afterwards (`-1` = nothing).
#[no_mangle]
pub extern "C" fn kg_scope_drop() -> i64 {
    let owned = SCOPE.with(|s| s.borrow_mut().take());
    drop(owned);
    inject::<Theme>().map_or(-1, |t| t.0)
}

#[no_mangle]
pub extern "C" fn kg_cleanups() -> i64 {
    CLEANUPS.with(|c| c.get())
}

// ---------------------------------------------------------------------------
// RemoteCounter's reactive half (phase 3b): host-owned props and context.
// The UI half (the element bridge) is the next phase; here the effect
// renders into a string the host reads back.
// ---------------------------------------------------------------------------

fn wire_encode<T: Wire>(v: &T, out: &mut Vec<u8>) {
    v.encode(out)
}

fn wire_decode<T: Wire>(b: &[u8]) -> Option<T> {
    T::from_bytes(b)
}

fn codec<T: Wire>() -> Codec<T> {
    Codec { encode: wire_encode::<T>, decode: wire_decode::<T> }
}

/// Host context, as this bundle sees it: who is signed in, reactively. The
/// host declares it under "CurrentUser" as the handle of its user signal.
#[derive(Clone)]
struct CurrentUser(ReadSignal<String>);

fn decode_handle(b: &[u8]) -> Option<(u32, u32, u32)> {
    let mut b = b;
    Some((u32::decode(&mut b)?, u32::decode(&mut b)?, u32::decode(&mut b)?))
}

thread_local! {
    static LINE: RefCell<String> = const { RefCell::new(String::new()) };
    static CLICKS: Cell<Option<Signal<i64>>> = const { Cell::new(None) };
    static VALUE: Cell<Option<Signal<i64>>> = const { Cell::new(None) };
}

/// Mount the counter: `external` is a read-only host prop, `value` a
/// two-way host prop, and `CurrentUser` comes from host context.
#[no_mangle]
pub extern "C" fn kg_counter_mount(ew: u32, es: u32, eg: u32, vw: u32, vs: u32, vg: u32) {
    register_remote_context::<CurrentUser>("CurrentUser", |b| {
        Some(CurrentUser(import_read_signal(decode_handle(b)?, codec::<String>())))
    });
    let external = import_read_signal((ew, es, eg), codec::<i64>());
    let value = import_signal((vw, vs, vg), codec::<i64>());
    let user = inject::<CurrentUser>().map(|u| u.0);
    let clicks = signal(0i64);
    effect(move || {
        let who = user.map_or_else(|| "nobody".to_string(), |u| u.get());
        let line = format!("external: {} clicks: {} value: {} user: {}", external.get(), clicks.get(), value.get(), who);
        LINE.with(|l| *l.borrow_mut() = line);
    });
    CLICKS.with(|c| c.set(Some(clicks)));
    VALUE.with(|v| v.set(Some(value)));
}

#[no_mangle]
pub extern "C" fn kg_counter_click() {
    CLICKS.with(|c| c.get()).expect("mounted").update(|n| n + 1);
}

/// Write the two-way prop back to the host.
#[no_mangle]
pub extern "C" fn kg_counter_bump_value() {
    VALUE.with(|v| v.get()).expect("mounted").update(|n| n + 1);
}

/// The rendered line, as `(ptr << 32) | len` into this module's memory.
#[no_mangle]
pub extern "C" fn kg_counter_line() -> i64 {
    LINE.with(|l| {
        let l = l.borrow();
        ((l.as_ptr() as usize as i64) << 32) | l.len() as i64
    })
}

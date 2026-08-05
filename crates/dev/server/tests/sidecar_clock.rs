//! Regression: the sidecar boot must install a monotonic clock.
//!
//! The bug: the old core installed a time source from `mount`. The
//! new-core sidecar boot (`sidecar::run_newcore`) never ran that
//! preamble, so `runtime_shared::time::now_micros()` returned `0` for
//! the life of the process. Every tween and every raf-driven
//! computation then resolved against t=0 and produced an identical
//! value on every tick.
//!
//! The symptom is deceptive, which is why it survived the migration:
//! nothing errors, the raf loop still spins, and the wire still floods
//! with `SetAnimated*` commands (a quarter-million of them in a few
//! seconds of the welcome example) — but the values never change, so
//! the screen is frozen and tweens sit pinned at their start value.
//! The welcome example's planets stayed at `opacity: 0` and its sun
//! never pulsed, while `--local` (which boots through
//! `backend_web::newcore::start_in`, installing a `performance.now()`
//! source) animated correctly.
//!
//! Why this is the tightest reachable test: `TIME_SOURCE` is a
//! process-wide `OnceLock`, so installed-vs-not cannot be toggled
//! mid-binary — the two halves below must live in ONE test, in this
//! order, in a test file of its own. Driving the real `run_loop` is
//! not an option either: it is a blocking stdio IPC server that owns
//! the process. So this pins the exact invariant the boot preamble is
//! responsible for, and the "before" half is a genuine reproduction of
//! the frozen-clock state rather than a restatement of the fix.

use std::thread::sleep;
use std::time::Duration;

use runtime_shared::time::now_micros;

#[test]
fn regression_sidecar_boot_installs_a_clock_so_tweens_advance() {
    // --- before: the state the sidecar shipped in -------------------
    // With no source installed, the clock is not merely coarse — it is
    // stuck at zero, so *no* elapsed-time computation can ever move.
    assert_eq!(
        now_micros(),
        0,
        "precondition: no time source installed yet in this test binary",
    );
    sleep(Duration::from_millis(5));
    assert_eq!(
        now_micros(),
        0,
        "a frozen clock is what made every tween emit its start value \
         forever while the raf loop kept spinning",
    );

    // --- after: the boot preamble runs ------------------------------
    // Goes through the boot seam `sidecar::run_loop` actually calls,
    // NOT `install_clock` directly. The bug was never that
    // `install_clock` misbehaved — it was that the boot never called
    // it, so a test poking the installer directly would have stayed
    // green through the entire regression.
    dev_server::scheduler::install_boot();

    let t0 = now_micros();
    sleep(Duration::from_millis(5));
    let t1 = now_micros();

    assert!(
        t1 > t0,
        "sidecar clock must advance so tweens progress: {t0} -> {t1}",
    );
    // Guard against a source that reports a constant non-zero value:
    // 5 ms of sleep must be visible as at least ~1 ms of elapsed time
    // (generous slack for scheduler jitter on a loaded CI box).
    assert!(
        t1 - t0 >= 1_000,
        "expected >= 1ms of elapsed time across a 5ms sleep, got {} us",
        t1 - t0,
    );
}

//! Portable host tests for the public connectivity surface.
//!
//! These exercise the crate from *outside* (as a dependent would), so they
//! run on whatever host runs the suite: the real `NWPathMonitor` path on
//! macOS, the inert `fallback` impl on Linux/Windows CI. Either way the
//! observable contract — a self-consistent snapshot and a watch guard that
//! registers and unregisters cleanly — must hold.

use connectivity::{current, watch, Connectivity, Transport};

/// `current()` always returns an online/transport pair that agrees: offline
/// iff `Transport::None`.
#[test]
fn current_snapshot_is_consistent() {
    let net = current();
    if net.online {
        assert_ne!(net.transport, Transport::None);
    } else {
        assert_eq!(net.transport, Transport::None);
    }
}

/// `watch` hands back an RAII guard; registering and dropping it must not
/// panic on any platform whose impl compiles for the host. The callback may
/// never fire (CI hosts don't flip network mid-test) — the lifecycle is what's
/// under test.
#[test]
fn watch_lifecycle_is_clean() {
    use std::cell::Cell;
    use std::rc::Rc;

    let hits = Rc::new(Cell::new(0u32));
    let hits_cb = hits.clone();
    let sub = watch(move |_net| hits_cb.set(hits_cb.get() + 1));
    drop(sub);
    // No assertion on the count — only that register + drop are panic-free.
    let _ = hits.get();
}

/// The named constants expose the two consistent extremes a caller compares
/// against.
#[test]
fn constants_round_trip() {
    assert_eq!(
        Connectivity::OFFLINE,
        Connectivity {
            online: false,
            transport: Transport::None
        }
    );
    assert!(Connectivity::ASSUME_ONLINE.online);
    assert_ne!(Connectivity::ASSUME_ONLINE.transport, Transport::None);
}

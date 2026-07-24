//! Linux (desktop / GTK4) reachability via **NetworkManager** over D-Bus.
//!
//! NetworkManager (`org.freedesktop.NetworkManager`) is the de-facto network
//! daemon on modern desktop Linux (GNOME, KDE, most distros). It sits on the
//! **system** bus and exposes the whole-machine connectivity state, so we can
//! answer [`current`] with a cheap property read and drive [`watch`] off its
//! property-change signals — no polling, no `libdbus` C dependency (we use the
//! pure-Rust async `zbus`, matching the `credentials` crate's Secret Service
//! backend).
//!
//! ## State mapping — why `CONNECTED_GLOBAL` is the online threshold
//!
//! NM's top-level `State` property (`NMState`, `nm-dbus-types.h`) is an
//! escalating enum: `DISCONNECTED(20)` → `CONNECTING(40)` →
//! `CONNECTED_LOCAL(50)` (link-local only) → `CONNECTED_SITE(60)` (a default
//! route exists but NM's connectivity check did NOT confirm internet — captive
//! portal / gateway with no upstream) → `CONNECTED_GLOBAL(70)` (full network
//! access). We report **online iff `State == CONNECTED_GLOBAL`**, mirroring the
//! Android backend's `NET_CAPABILITY_VALIDATED` requirement and Apple's
//! `nw_path` "satisfied": all three demand *verified* reachability, not merely
//! "a cable is plugged in". SITE/LOCAL therefore map to offline — an internet
//! request from those states would fail, which is exactly what a "should I
//! attempt the request?" caller wants to know. (When NM's connectivity
//! checking is disabled it promotes a routed connection straight to GLOBAL, so
//! the common fully-connected case still reads online.)
//!
//! ## Transport
//!
//! NM's `PrimaryConnectionType` property is the connection-setting name of the
//! active primary connection (`"802-11-wireless"`, `"802-3-ethernet"`, `"gsm"`,
//! …). We map it to the coarse [`Transport`] categories; anything we don't
//! recognise (VPN, bridge, tun, empty) is [`Transport::Other`] while still
//! online.
//!
//! ## `watch` — a background thread pumping the signal stream
//!
//! zbus is async; the SDK's [`watch`] callback is synchronous and `!Send`. We
//! spawn one dedicated thread that owns a `zbus::Connection` and blocks on the
//! NM property-change streams (`State` and `PrimaryConnectionType`), re-reading
//! the status and invoking the callback on each change. The callback is
//! delivered on *this* thread — consistent with the crate contract that the
//! callback "runs wherever the platform delivers the change" (a background
//! callback thread on Android, a dispatch queue on Apple). The subscription's
//! `Drop` closes a shutdown channel, which unblocks the stream select and lets
//! the thread exit cleanly (join, no detach, no leak).

use crate::{Connectivity, Transport, WatchCallback};

use futures_lite::prelude::*;

// `NMState` — org.freedesktop.NetworkManager `State` property (nm-dbus-types.h).
// Only the values we branch on are named; the rest collapse to "offline".
const NM_STATE_UNKNOWN: u32 = 0;
const NM_STATE_CONNECTED_GLOBAL: u32 = 70;

/// A minimal typed view of the NetworkManager manager object. `zbus::proxy`
/// generates both the async proxy (`NetworkManagerProxy`, used by `watch`) and
/// the blocking one (`NetworkManagerProxyBlocking`, used by the synchronous
/// `current`). We only bind the two properties we need.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
    /// `NMState` — whole-machine connectivity state (see module docs).
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;

    /// Connection-setting type of the active primary connection, e.g.
    /// `"802-11-wireless"`. Empty when there is no primary connection.
    #[zbus(property)]
    fn primary_connection_type(&self) -> zbus::Result<String>;
}

/// Map an NM primary-connection type string to a coarse [`Transport`].
///
/// The strings are NM's connection-setting names (`nm-setting-*`). Unknown /
/// uncategorised links (VPN, bridge, tun, or an empty value on an online-but-
/// odd setup) fall through to [`Transport::Other`] — still online, medium
/// undetermined, per the SDK contract.
fn transport_from_type(primary_type: &str) -> Transport {
    match primary_type {
        "802-11-wireless" | "wifi" => Transport::Wifi,
        "802-3-ethernet" | "ethernet" => Transport::Ethernet,
        // Mobile-broadband settings all read as cellular.
        "gsm" | "cdma" | "wwan" | "wimax" => Transport::Cellular,
        _ => Transport::Other,
    }
}

/// Pure NM-state → [`Connectivity`] mapping. Kept dependency-free (no bus, no
/// I/O) so the mapping logic is unit-testable in isolation — the live D-Bus
/// path only feeds `(state, primary_type)` into this.
///
/// Online iff `state == CONNECTED_GLOBAL` (see module docs); an online snapshot
/// carries the transport derived from `primary_type` (never `None`), an offline
/// one is the canonical `{ online: false, transport: None }`. This preserves
/// the crate-wide online/transport consistency invariant.
fn map_status(state: u32, primary_type: &str) -> Connectivity {
    if state == NM_STATE_CONNECTED_GLOBAL {
        Connectivity {
            online: true,
            transport: transport_from_type(primary_type),
        }
    } else {
        Connectivity::OFFLINE
    }
}

/// Synchronous snapshot from NetworkManager over the **system** bus.
///
/// Best-effort: any failure to reach NM (no system bus, NM not running, a
/// permission or property-read error) yields the SDK's `ASSUME_ONLINE`
/// fallback rather than a panic, matching the contract that `current()` never
/// throws. On a desktop with NM live this returns the real state.
pub(crate) fn current() -> Connectivity {
    query_current().unwrap_or(Connectivity::ASSUME_ONLINE)
}

fn query_current() -> zbus::Result<Connectivity> {
    // Blocking zbus over async-io — no separate runtime needed for the one-shot
    // read. Matches the `credentials` crate's pure-Rust zbus usage.
    let conn = zbus::blocking::Connection::system()?;
    let proxy = NetworkManagerProxyBlocking::new(&conn)?;
    let state = proxy.state()?;
    // A missing/erroring primary-type shouldn't sink the whole read — default
    // to empty (→ Transport::Other when otherwise online).
    let primary_type = proxy.primary_connection_type().unwrap_or_default();
    Ok(map_status(state, &primary_type))
}

/// Assert `Send` for the boxed callback so it can be moved onto the watcher
/// thread. The SDK's [`WatchCallback`] is `!Send` for the *web* backend's sake
/// (it captures non-`Send` JS values); the crate contract already documents
/// that on native platforms the callback is delivered on a platform-owned
/// background thread (Android's callback thread, Apple's dispatch queue). This
/// is the Linux equivalent: the callback only ever runs on the single watcher
/// thread we spawn, never concurrently, so moving it there is consistent with
/// that documented threading contract.
struct AssertSend<T>(T);
// SAFETY: see the doc comment — the wrapped callback is confined to, and only
// invoked from, the one watcher thread; there is no cross-thread aliasing.
unsafe impl<T> Send for AssertSend<T> {}

/// Why the stream select woke.
enum Wake {
    /// A watched property changed (or a stream ended) — re-query and deliver.
    Changed,
    /// Shutdown requested (subscription dropped) or a stream closed — exit.
    Stop,
}

/// Subscribe to NM connectivity changes and invoke `callback` on each.
///
/// Spawns a dedicated thread owning a `zbus::Connection`; the thread blocks on
/// NM's `State` / `PrimaryConnectionType` property-change streams. If the
/// connection can't be established (no system bus / NM absent) the thread exits
/// immediately and the subscription is inert — `watch` never panics.
pub(crate) fn watch(callback: WatchCallback) -> Subscription {
    // Bounded(1) shutdown channel: dropping the sender in `Subscription::Drop`
    // closes it, which resolves the receiver in the select below and lets the
    // thread break out of its loop.
    let (shutdown_tx, shutdown_rx) = async_channel::bounded::<()>(1);
    let cb = AssertSend(callback);

    let spawned = std::thread::Builder::new()
        .name("connectivity-nm-watch".into())
        .spawn(move || {
            let cb = cb; // move the whole Send wrapper onto this thread
            futures_lite::future::block_on(watch_loop(cb.0, shutdown_rx));
        });

    match spawned {
        Ok(handle) => Subscription {
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        },
        // Thread spawn failed → inert subscription (callback already dropped
        // with the failed closure). Never panic from `watch`.
        Err(_) => Subscription {
            shutdown: None,
            handle: None,
        },
    }
}

/// The watcher thread's async body: connect, then loop delivering a fresh
/// snapshot every time NM reports a state/transport change, until shutdown.
async fn watch_loop(callback: WatchCallback, shutdown: async_channel::Receiver<()>) {
    let conn = match zbus::Connection::system().await {
        Ok(c) => c,
        // No system bus / NM unreachable → nothing to watch; exit quietly.
        Err(_) => return,
    };
    let proxy = match NetworkManagerProxy::new(&conn).await {
        Ok(p) => p,
        Err(_) => return,
    };

    // Property-change streams. These fire on the standard
    // org.freedesktop.DBus.Properties.PropertiesChanged that NM emits; watching
    // both catches online/offline transitions (State) AND transport switches
    // that keep us online, e.g. wifi → ethernet (PrimaryConnectionType).
    let mut state_stream = proxy.receive_state_changed().await;
    let mut type_stream = proxy.receive_primary_connection_type_changed().await;

    loop {
        // Race the two property streams against the shutdown signal. A stream
        // yielding `None` means the connection closed — treat as Stop so we
        // don't spin. `FutureExt::or` drops the losing futures; the streams
        // themselves persist across iterations, so re-arming `.next()` is fine.
        let wake = {
            let on_state = async {
                match state_stream.next().await {
                    Some(_) => Wake::Changed,
                    None => Wake::Stop,
                }
            };
            let on_type = async {
                match type_stream.next().await {
                    Some(_) => Wake::Changed,
                    None => Wake::Stop,
                }
            };
            let on_shutdown = async {
                // Ok or Err (closed) both mean "stop".
                let _ = shutdown.recv().await;
                Wake::Stop
            };
            on_state.or(on_type).or(on_shutdown).await
        };

        match wake {
            Wake::Stop => break,
            Wake::Changed => {
                let state = proxy.state().await.unwrap_or(NM_STATE_UNKNOWN);
                let primary_type = proxy.primary_connection_type().await.unwrap_or_default();
                callback(map_status(state, &primary_type));
            }
        }
    }
}

/// Linux subscription: owns the shutdown channel and the watcher thread's join
/// handle. `Drop` closes the channel (unblocking the stream select) and joins
/// the thread, so the D-Bus connection is torn down and the callback freed
/// exactly when the caller drops the guard — no detached thread, no leak.
pub(crate) struct Subscription {
    shutdown: Option<async_channel::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // Closing the sender resolves `shutdown.recv()` on the watcher thread,
        // which selects to `Wake::Stop` and breaks the loop.
        if let Some(tx) = self.shutdown.take() {
            drop(tx);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Connectivity, Transport};

    /// NM `CONNECTED_GLOBAL` with a wifi primary connection → online over wifi.
    #[test]
    fn maps_connected_global_wifi_online() {
        assert_eq!(
            map_status(NM_STATE_CONNECTED_GLOBAL, "802-11-wireless"),
            Connectivity {
                online: true,
                transport: Transport::Wifi
            }
        );
    }

    /// `CONNECTED_GLOBAL` over ethernet → online over ethernet.
    #[test]
    fn maps_connected_global_ethernet_online() {
        assert_eq!(
            map_status(NM_STATE_CONNECTED_GLOBAL, "802-3-ethernet"),
            Connectivity {
                online: true,
                transport: Transport::Ethernet
            }
        );
    }

    /// A recognised mobile-broadband type → cellular.
    #[test]
    fn maps_cellular_types() {
        for t in ["gsm", "cdma", "wwan", "wimax"] {
            assert_eq!(transport_from_type(t), Transport::Cellular, "type {t}");
        }
    }

    /// Online but an uncategorised/empty primary type → online over `Other`
    /// (e.g. VPN, bridge). Still a consistent online snapshot.
    #[test]
    fn maps_connected_global_unknown_type_is_other() {
        for t in ["vpn", "bridge", "tun", ""] {
            let c = map_status(NM_STATE_CONNECTED_GLOBAL, t);
            assert!(c.online, "type {t:?} should be online");
            assert_eq!(c.transport, Transport::Other, "type {t:?}");
        }
    }

    /// Every non-GLOBAL NM state — including the "connected but unverified"
    /// SITE(60)/LOCAL(50) rungs and the disconnected/transitional ones — maps
    /// to the canonical offline snapshot, regardless of primary type. This is
    /// the behaviour that makes the Linux path *differ* from the old inert
    /// fallback (which always reported ASSUME_ONLINE).
    #[test]
    fn maps_non_global_states_offline() {
        for state in [
            NM_STATE_UNKNOWN,
            10, // ASLEEP
            20, // DISCONNECTED
            30, // DISCONNECTING
            40, // CONNECTING
            50, // CONNECTED_LOCAL
            60, // CONNECTED_SITE (default route, but internet unverified)
        ] {
            let c = map_status(state, "802-3-ethernet");
            assert_eq!(
                c,
                Connectivity::OFFLINE,
                "NM state {state} must map to OFFLINE"
            );
        }
    }

    /// Every snapshot `map_status` can produce satisfies the crate-wide
    /// online/transport consistency invariant.
    #[test]
    fn mapped_snapshots_are_self_consistent() {
        for state in [0u32, 20, 40, 50, 60, 70] {
            for t in ["802-11-wireless", "802-3-ethernet", "gsm", "vpn", ""] {
                let c = map_status(state, t);
                if c.online {
                    assert_ne!(c.transport, Transport::None);
                } else {
                    assert_eq!(c.transport, Transport::None);
                }
            }
        }
    }

    /// Live integration check against the real system bus + NetworkManager.
    /// Ignored by default: it needs a running system D-Bus with
    /// `org.freedesktop.NetworkManager` present (true on a desktop, not in a
    /// headless/minimal CI container), so it can't be an always-on unit test.
    /// Run it on a real desktop with `cargo test -p connectivity -- --ignored`.
    #[test]
    #[ignore = "needs a live system D-Bus + NetworkManager service"]
    fn live_current_reads_real_nm_state() {
        let net = super::current();
        // Whatever the box's real state, the snapshot must be self-consistent.
        if net.online {
            assert_ne!(net.transport, Transport::None);
        } else {
            assert_eq!(net.transport, Transport::None);
        }
    }
}

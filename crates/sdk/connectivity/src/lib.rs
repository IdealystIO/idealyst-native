//! Cross-platform **network reachability**.
//!
//! The smallest useful abstraction over the platform's connectivity state:
//! ask whether the device currently has network access and over what
//! transport, and subscribe to be told when that changes. No bandwidth
//! estimation, no metered/expensive flags, no reactive-`Signal` wrapper —
//! those are higher-level layers. This crate just reports reachability and a
//! coarse transport type.
//!
//! # API
//!
//! - [`current`] — a synchronous snapshot ([`Connectivity`]). Best-effort:
//!   platforms that can't answer cheaply fall back to
//!   `{ online: true, transport: Other }` (documented per-platform below).
//! - [`watch`] — register a callback fired on every change. The returned
//!   [`ConnectivitySubscription`] is an RAII guard: hold it for as long as
//!   you want updates, drop it to unregister. Don't `mem::forget` it — store
//!   it in your app/component state.
//!
//! ```ignore
//! use connectivity::{current, watch};
//!
//! // Snapshot now.
//! let net = current();
//! println!("online={} via {:?}", net.online, net.transport);
//!
//! // Subscribe to changes; keep `sub` alive to keep receiving them.
//! let sub = watch(|net| {
//!     println!("changed: online={} via {:?}", net.online, net.transport);
//! });
//! // ... later: drop(sub) to stop.
//! ```
//!
//! # Per-platform mechanism
//!
//! Exactly one cfg-gated backend module is compiled per target; the public
//! API delegates to its `imp` functions. The *observable behavior* is
//! identical across them — they diverge only in mechanism.
//!
//! - **web (wasm32)** — `navigator.onLine` for [`Connectivity::online`];
//!   the window `online` / `offline` events drive [`watch`];
//!   `navigator.connection` (NetworkInformation) supplies the transport hint
//!   where the browser exposes it (else [`Transport::Other`]).
//! - **iOS / macOS / tvOS** — `NWPathMonitor` from the Network framework:
//!   `currentPath.status == .satisfied` is online, and `usesInterfaceType:`
//!   picks the transport. The path-update block bridges to the [`watch`]
//!   callback; the monitor lives inside the subscription and is `cancel`led
//!   on drop. *Compile-checked only — not device-verified.*
//! - **Android** — `ConnectivityManager`: `getActiveNetwork` +
//!   `getNetworkCapabilities` answer [`current`]; the transport comes from
//!   `hasTransport(TRANSPORT_WIFI/CELLULAR/ETHERNET)` and online-ness from
//!   `NET_CAPABILITY_VALIDATED`. [`watch`] is structured around
//!   `registerDefaultNetworkCallback`, but the `NetworkCallback` requires a
//!   Java/Kotlin subclass that pure JNI can't synthesize — see
//!   [`watch`]'s notes for the host-shim seam. *Compile-checked only.*
//! - **other native (Windows / Linux / tests)** — no platform monitor;
//!   [`current`] returns the best-effort `{ online: true, transport: Other }`
//!   fallback and [`watch`] never fires (the guard is inert).
//!
//! # Permissions
//!
//! Only Android needs one: `ACCESS_NETWORK_STATE`. iOS / macOS / web need
//! none. The capability is declared as `network_state` in this crate's
//! `[package.metadata.idealyst]`; the CLI maps it to the Android manifest
//! permission and injects nothing elsewhere.

#![deny(missing_docs)]

// Backend selector. Exactly one compiles per target; each supplies the `imp`
// functions `current()` and `watch()` plus a `Subscription` whose `Drop`
// unregisters. Targets with no native monitor fall through to the inert
// `fallback` impl.
#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "ios", target_os = "macos", target_os = "tvos")
))]
#[path = "apple.rs"]
mod imp;

#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
#[path = "android.rs"]
mod imp;

#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
#[path = "fallback.rs"]
mod imp;

#[doc(hidden)]
#[cfg(feature = "catalog")]
mod recipes;

// ---------------------------------------------------------------------------
// Public, platform-agnostic surface.
// ---------------------------------------------------------------------------

/// The coarse kind of network transport currently carrying traffic.
///
/// Deliberately coarse — this is the reachability *category*, not a precise
/// link descriptor. Where a platform can't distinguish the medium it reports
/// [`Transport::Other`]; when there's no connectivity at all it's
/// [`Transport::None`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// A wifi (WLAN) link.
    Wifi,
    /// A cellular (mobile data) link.
    Cellular,
    /// A wired ethernet link.
    Ethernet,
    /// Connected over some other / undetermined medium (VPN, loopback, a
    /// transport the platform doesn't categorize, or a platform that can't
    /// tell). Pairs with `online: true`.
    Other,
    /// No transport — the device is offline. Pairs with `online: false`.
    None,
}

/// A snapshot of the device's network reachability.
///
/// `online` and `transport` are consistent: an offline snapshot is
/// `{ online: false, transport: None }`; an online one carries a non-`None`
/// transport (`Other` when the medium is unknown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Connectivity {
    /// Whether the device currently has network access.
    pub online: bool,
    /// The transport carrying that access (or [`Transport::None`] offline).
    pub transport: Transport,
}

impl Connectivity {
    /// An offline snapshot: `{ online: false, transport: None }`.
    pub const OFFLINE: Connectivity = Connectivity {
        online: false,
        transport: Transport::None,
    };

    /// The best-effort "we can't tell cheaply, assume reachable" snapshot:
    /// `{ online: true, transport: Other }`. Platforms with no synchronous
    /// query return this from [`current`].
    pub const ASSUME_ONLINE: Connectivity = Connectivity {
        online: true,
        transport: Transport::Other,
    };
}

/// A synchronous best-effort snapshot of the current network reachability.
///
/// Cheap to call. Where a platform exposes no synchronous query it returns
/// [`Connectivity::ASSUME_ONLINE`] (`{ online: true, transport: Other }`)
/// rather than blocking — assuming reachability is the safe default for a
/// "should I attempt the request?" check. Use [`watch`] for authoritative,
/// up-to-the-moment state delivered as it changes.
pub fn current() -> Connectivity {
    imp::current()
}

/// Subscribe to connectivity changes. `callback` is invoked with a fresh
/// [`Connectivity`] snapshot on every change the platform reports.
///
/// The returned [`ConnectivitySubscription`] is an RAII guard that
/// unregisters the callback (and tears down any native monitor it owns) when
/// dropped. **Hold onto it** for as long as you want updates — store it in
/// your component/app state. Dropping it stops delivery; do not `mem::forget`
/// it (that would leak the monitor and, on web, the JS closure).
///
/// The callback is *not* invoked immediately with the current state — call
/// [`current`] yourself to seed initial UI, then let `watch` deliver changes.
///
/// ## Threading
///
/// The callback runs wherever the platform delivers the change: the main
/// thread / run loop on web and Apple, a `ConnectivityManager` callback
/// thread on Android. Keep it fast and non-blocking; copy out what you need.
///
/// ## Android seam
///
/// On Android, change delivery uses `registerDefaultNetworkCallback`, which
/// needs a `ConnectivityManager.NetworkCallback` *subclass* to receive
/// `onAvailable` / `onLost` / `onCapabilitiesChanged`. Pure JNI can't define
/// a new Java class, so a small host-provided Java/Kotlin shim must forward
/// those callbacks back across JNI. Until that shim is wired by the host, the
/// Android [`watch`] registers the underlying request but the callback may
/// not fire on change — see `src/android.rs`. [`current`] is fully
/// implemented and authoritative on Android regardless.
pub fn watch(callback: impl Fn(Connectivity) + 'static) -> ConnectivitySubscription {
    ConnectivitySubscription {
        _inner: imp::watch(Box::new(callback)),
    }
}

/// The boxed callback shape backends receive from [`watch`].
///
/// Not `Send` — change delivery happens on the platform's own thread/run
/// loop (main thread on web/Apple), and the web backend captures non-`Send`
/// JS values, so a `Send` bound would be both unnecessary and unsatisfiable.
pub(crate) type WatchCallback = Box<dyn Fn(Connectivity) + 'static>;

/// An active connectivity subscription. Holds the platform monitor and
/// unregisters it on drop — keep it alive for as long as you want change
/// notifications (see [`watch`]). Dropping it stops delivery and releases the
/// native monitor.
pub struct ConnectivitySubscription {
    // The concrete type is backend-specific; its `Drop` does the teardown.
    _inner: imp::Subscription,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `current()` returns a self-consistent snapshot on whatever host runs
    /// the tests. On Linux/Windows CI that's the `fallback` impl
    /// (`ASSUME_ONLINE`); on macOS it's the real `NWPathMonitor` path. Either
    /// way the online/transport pair must be consistent.
    #[test]
    fn current_is_self_consistent() {
        let net = current();
        if net.online {
            assert_ne!(
                net.transport,
                Transport::None,
                "an online snapshot must not carry Transport::None"
            );
        } else {
            assert_eq!(
                net.transport,
                Transport::None,
                "an offline snapshot must carry Transport::None"
            );
        }
    }

    /// The two named constants must satisfy the same consistency invariant.
    #[test]
    fn constants_are_consistent() {
        assert!(!Connectivity::OFFLINE.online);
        assert_eq!(Connectivity::OFFLINE.transport, Transport::None);
        assert!(Connectivity::ASSUME_ONLINE.online);
        assert_ne!(Connectivity::ASSUME_ONLINE.transport, Transport::None);
    }

    /// `watch` returns a guard that drops cleanly without firing (the test
    /// host doesn't change network state mid-run). This exercises the
    /// register → drop/unregister lifecycle on every platform's impl that
    /// compiles for the host.
    #[test]
    fn watch_registers_and_drops_cleanly() {
        let sub = watch(|_net| {
            // No assertion: on a CI host the network doesn't flip during the
            // test, so this may never run. The point is that register + the
            // RAII unregister on drop don't panic.
        });
        drop(sub);
    }

    /// The fallback used by hosts with no native monitor must be the
    /// documented best-effort snapshot, so a "should I try the request?"
    /// check defaults to attempting it.
    #[cfg(all(
        not(target_arch = "wasm32"),
        not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
        not(target_os = "android")
    ))]
    #[test]
    fn fallback_assumes_online() {
        assert_eq!(current(), Connectivity::ASSUME_ONLINE);
    }
}

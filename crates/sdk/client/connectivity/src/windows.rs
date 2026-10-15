//! Windows reachability via WinRT `Windows.Networking.Connectivity`.
//!
//! ## Online threshold
//!
//! [`current`] reads the *internet* connection profile
//! (`NetworkInformation::GetInternetConnectionProfile`) and reports online iff
//! its `GetNetworkConnectivityLevel()` is `InternetAccess` — Windows' own
//! verified-internet check (NCSI), the same bar as Android's
//! `NET_CAPABILITY_VALIDATED`, Apple's `satisfied` and NetworkManager's
//! `CONNECTED_GLOBAL`. No profile, or `LocalAccess` / `ConstrainedInternetAccess`
//! (a captive portal), is offline: an internet request from there would fail.
//!
//! ## Transport
//!
//! From the same profile: `IsWlanConnectionProfile` → [`Transport::Wifi`],
//! `IsWwanConnectionProfile` → [`Transport::Cellular`], an adapter whose IANA
//! interface type is `ethernetCsmacd` (6) → [`Transport::Ethernet`], anything
//! else online (VPN, tunnel) → [`Transport::Other`].
//!
//! ## `watch`
//!
//! `NetworkInformation::NetworkStatusChanged` fires (on a thread-pool thread)
//! when the network state changes. Each event re-reads the snapshot and
//! delivers it if it differs from the last one delivered — the event also
//! fires for changes this crate does not report (cost, a second adapter). The
//! callback therefore runs on a background thread, as on Linux and Android
//! (the crate contract: it runs wherever the platform delivers the change).
//! The subscription's `Drop` removes the handler; Windows waits for an
//! in-flight invocation before that returns.

use std::sync::Mutex;

use windows::Networking::Connectivity::{
    NetworkConnectivityLevel, NetworkInformation, NetworkStatusChangedEventHandler,
};

use crate::{Connectivity, Transport, WatchCallback};

/// IANA `ifType` for Ethernet (`ethernetCsmacd`).
const IF_TYPE_ETHERNET: u32 = 6;

pub(crate) fn current() -> Connectivity {
    snapshot().unwrap_or(Connectivity::ASSUME_ONLINE)
}

/// The snapshot, or `Err` when WinRT itself failed (not when offline).
fn snapshot() -> windows::core::Result<Connectivity> {
    // No internet profile at all is a normal offline answer, not a failure.
    let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() else {
        return Ok(Connectivity::OFFLINE);
    };
    if profile.GetNetworkConnectivityLevel()? != NetworkConnectivityLevel::InternetAccess {
        return Ok(Connectivity::OFFLINE);
    }
    let transport = if profile.IsWlanConnectionProfile()? {
        Transport::Wifi
    } else if profile.IsWwanConnectionProfile()? {
        Transport::Cellular
    } else if profile.NetworkAdapter().and_then(|a| a.IanaInterfaceType()).ok() == Some(IF_TYPE_ETHERNET) {
        Transport::Ethernet
    } else {
        Transport::Other
    };
    Ok(Connectivity { online: true, transport })
}

/// The callback, moved onto WinRT's thread pool.
///
/// SAFETY: the crate contract hands the callback to whatever thread the
/// platform delivers on (Linux's watcher thread does the same with its own
/// wrapper); the mutex serializes the pool's concurrent invocations, so the
/// non-`Sync` callback is never entered from two threads at once.
struct Delivery {
    callback: WatchCallback,
    last: Option<Connectivity>,
}
unsafe impl Send for Delivery {}

pub(crate) fn watch(callback: WatchCallback) -> Subscription {
    let delivery = Mutex::new(Delivery { callback, last: Some(current()) });
    let handler = NetworkStatusChangedEventHandler::new(move |_| {
        let Ok(net) = snapshot() else { return Ok(()) };
        let mut d = delivery.lock().unwrap_or_else(|p| p.into_inner());
        if d.last != Some(net) {
            d.last = Some(net);
            (d.callback)(net);
        }
        Ok(())
    });
    Subscription { token: NetworkInformation::NetworkStatusChanged(&handler).ok() }
}

/// Removes the `NetworkStatusChanged` handler on drop. `None` when
/// registering failed: an inert subscription, never a panic.
pub(crate) struct Subscription {
    token: Option<i64>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            let _ = NetworkInformation::RemoveNetworkStatusChanged(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows used the inert fallback: `current()` was always the
    /// ASSUME_ONLINE guess and `watch` registered nothing. Now the snapshot
    /// comes from WinRT and `watch` holds a live `NetworkStatusChanged`
    /// handler. (The test machine's network cannot be flipped mid-test, so
    /// delivery itself is not exercised here.)
    #[test]
    fn regression_windows_reads_the_internet_profile_and_watches_it() {
        let net = snapshot().expect("WinRT NetworkInformation answers");
        eprintln!("windows connectivity: {net:?}");
        assert_eq!(current(), net);
        // The test machines are online through a wired or virtual adapter;
        // what matters is that the answer is the platform's, consistent.
        assert_eq!(net.online, net.transport != Transport::None, "{net:?}");
        let sub = watch(Box::new(|_| {}));
        assert!(sub.token.is_some(), "NetworkStatusChanged handler registered");
        drop(sub);
    }
}

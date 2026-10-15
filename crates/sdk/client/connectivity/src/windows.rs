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
//! `NetworkInformation::NetworkStatusChanged` fires on a thread-pool thread
//! when the network state changes; each event re-reads the snapshot. The
//! callback is then run ON THE UI THREAD: the handler sends the snapshot over
//! a channel to a task spawned on the host's async executor (the Win32 host
//! polls it from its message pump), which delivers it if it differs from the
//! last one delivered — the event also fires for changes this crate does not
//! report (cost, a second adapter). A callback that writes a signal (the
//! documented use) must run there: signals belong to the UI thread's world.
//!
//! Without an installed executor (a bare test process, no host) a task that
//! waits on the channel would block the caller forever, so the callback is
//! run on the thread-pool thread instead (the crate contract's fallback:
//! wherever the platform delivers), serialized behind a mutex.
//!
//! The subscription's `Drop` removes the handler; Windows waits for an
//! in-flight invocation before that returns. Dropping the handler drops the
//! channel's sender, which ends the UI-thread task.

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

/// The callback, moved onto WinRT's thread pool (no executor installed).
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
    if runtime_core::driver::async_executor_installed() {
        watch_on_ui_thread(callback)
    } else {
        watch_on_pool(callback)
    }
}

fn watch_on_ui_thread(callback: WatchCallback) -> Subscription {
    let tx = ui_thread_sink(callback, current());
    let handler = NetworkStatusChangedEventHandler::new(move |_| {
        if let Ok(net) = snapshot() {
            let _ = tx.try_send(net);
        }
        Ok(())
    });
    Subscription { token: NetworkInformation::NetworkStatusChanged(&handler).ok() }
}

/// A sender usable from any thread whose snapshots reach `callback` on the
/// executor's thread (the UI thread), changes only (starting from `last`).
/// The delivering task ends when every sender is dropped.
fn ui_thread_sink(callback: WatchCallback, mut last: Connectivity) -> async_channel::Sender<Connectivity> {
    let (tx, rx) = async_channel::unbounded::<Connectivity>();
    runtime_core::driver::spawn_async(async move {
        while let Ok(net) = rx.recv().await {
            if net != last {
                last = net;
                callback(net);
            }
        }
    });
    tx
}

fn watch_on_pool(callback: WatchCallback) -> Subscription {
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
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::task::{Context, Wake, Waker};

    type Task = Pin<Box<dyn Future<Output = ()>>>;

    thread_local! {
        static TASKS: RefCell<Vec<Task>> = RefCell::new(Vec::new());
    }

    /// A hand-polled executor standing in for the host's: tasks wait on the
    /// test thread until [`poll_all`] runs them.
    struct TestExecutor;
    impl runtime_core::driver::AsyncExecutor for TestExecutor {
        fn spawn(&self, future: Task) {
            TASKS.with(|t| t.borrow_mut().push(future));
            poll_all();
        }
    }

    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }

    fn poll_all() {
        let waker = Waker::from(Arc::new(Noop));
        let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
        tasks.retain_mut(|t| t.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        TASKS.with(|t| t.borrow_mut().extend(tasks));
    }

    /// `watch` ran the callback on WinRT's thread pool. The documented use
    /// writes a signal from it, and signals belong to the UI thread's world,
    /// so a real network change would have written one from the wrong
    /// thread. A snapshot sent from another thread now reaches the callback
    /// on the executor's (UI) thread, and only when it changed.
    #[test]
    fn regression_watch_delivers_on_the_executor_thread() {
        runtime_core::driver::install_async_executor(Box::new(TestExecutor));
        let seen: Rc<RefCell<Vec<(Connectivity, std::thread::ThreadId)>>> = Rc::default();
        let s = seen.clone();
        let tx = ui_thread_sink(
            Box::new(move |net| s.borrow_mut().push((net, std::thread::current().id()))),
            Connectivity::ASSUME_ONLINE,
        );
        let pool = std::thread::spawn(move || {
            tx.try_send(Connectivity::ASSUME_ONLINE).unwrap(); // unchanged: not delivered
            tx.try_send(Connectivity::OFFLINE).unwrap();
        });
        pool.join().unwrap();
        assert!(seen.borrow().is_empty(), "nothing runs off the executor's thread");
        poll_all();
        assert_eq!(*seen.borrow(), [(Connectivity::OFFLINE, std::thread::current().id())]);
        assert!(TASKS.with(|t| t.borrow().is_empty()), "the task ends once the sender is gone");
    }

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

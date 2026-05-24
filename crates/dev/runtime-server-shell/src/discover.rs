//! mDNS / DNS-SD discovery for the runtime-server dev-server.
//!
//! The dev-server advertises itself on `_idealyst-dev._tcp.` with
//! a TXT record carrying `app_id=<project-bundle-id>`. Native
//! clients (iOS, Android, desktop) call [`discover`] with the
//! `app_id` they expect; we browse, filter, resolve, and return a
//! `ws://host:port` URL ready to feed into `tungstenite::connect`.
//!
//! Cross-platform — same Rust mDNS code runs on every native target.
//! The only platform-specific piece is the iOS Info.plist's
//! `NSBonjourServices` array, which is what iOS uses to permit
//! multicast traffic for declared service types. As long as that's
//! present the OS doesn't care whether the multicast is initiated
//! via Apple's NWBrowser or via raw sockets (which is what
//! `mdns-sd` uses).
//!
//! Web hosts can't run multicast — they use [`web-sys`] and their
//! "URL" is the http origin, not a discovered LAN address. This
//! module is `native`-only.

use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent};

/// DNS-SD service type the dev-server advertises under. Matches the
/// constant in `dev-server/src/transport.rs`.
pub const SERVICE_TYPE: &str = "_idealyst-dev._tcp.local.";

/// IPv6 link-local prefix check. `Ipv6Addr::is_unicast_link_local`
/// is unstable on stable Rust, so re-implement the `fe80::/10`
/// test directly — bits 0..10 of the address are `1111 1110 10`.
#[inline]
fn is_ipv6_link_local(addr: &std::net::Ipv6Addr) -> bool {
    let segs = addr.segments();
    (segs[0] & 0xffc0) == 0xfe80
}

/// Browse the local network for a dev-server whose TXT record
/// has `app_id=<expected_app_id>`. Returns a `ws://host:port` URL
/// on the first match, or `None` if the timeout elapses before a
/// matching service appears.
///
/// Caller is expected to retry with backoff if the first browse
/// returns `None` — that's the natural way to wait for the
/// dev-server to come up after the app starts. See
/// [`discover_blocking`] for the typical "loop until found" wrapper
/// the transport thread uses.
///
/// Note on iOS: this works as long as `NSBonjourServices` in
/// Info.plist lists `_idealyst-dev._tcp` and the user has approved
/// the Local Network permission prompt. The first call after a
/// fresh install triggers that prompt — keep that in mind when
/// timing reconnect logs.
pub fn discover(expected_app_id: &str, timeout: Duration) -> Option<String> {
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[discover] failed to create mDNS daemon: {}", e);
            return None;
        }
    };
    let receiver = match daemon.browse(SERVICE_TYPE) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[discover] browse failed: {}", e);
            return None;
        }
    };

    let deadline = Instant::now() + timeout;
    let mut found: Option<String> = None;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = match receiver.recv_timeout(remaining) {
            Ok(e) => e,
            Err(_) => break, // timed out
        };
        if let ServiceEvent::ServiceResolved(info) = event {
            // Match on the TXT record's `app_id` field. If the user
            // is running two dev-servers (e.g. hot-reload-demo +
            // docs-app) on the same Wi-Fi, this is what keeps the
            // wires from crossing.
            let mut matches = false;
            for prop in info.get_properties().iter() {
                if prop.key().eq_ignore_ascii_case("app_id")
                    && prop.val_str() == expected_app_id
                {
                    matches = true;
                    break;
                }
            }
            if !matches {
                continue;
            }
            // Address-selection order:
            //
            //   1. IPv4 (routable or loopback) — tungstenite handles
            //      these cleanly with no URL-bracket dance, and dev
            //      networks are predominantly IPv4.
            //   2. Routable IPv6 (`!a.is_loopback() && !is_link_local`).
            //   3. Loopback IPv6 (`::1`).
            //   4. Link-local IPv6 (`fe80::…`) — last resort. These
            //      *can* work with a zone id appended (`%enX`) but
            //      tungstenite + rust-url don't agree on the encoding,
            //      so the connect usually fails. The fallback below
            //      lets us at least try if nothing else is on offer.
            //
            // Pre-fix the previous code picked "first IPv4 or first
            // address" and then bracketed based on whether the
            // *first* address (not the chosen one) was IPv6 —
            // double-buggy. macOS in particular tends to surface
            // link-local fe80:: as the first advertised address,
            // which would loop forever trying to connect.
            let addrs = info.get_addresses();
            let pick_priority = |a: &std::net::IpAddr| -> u8 {
                match a {
                    std::net::IpAddr::V4(_) => 0,
                    std::net::IpAddr::V6(v) => {
                        if v.is_loopback() {
                            2
                        } else if is_ipv6_link_local(v) {
                            3
                        } else {
                            1
                        }
                    }
                }
            };
            let mut sorted: Vec<&std::net::IpAddr> = addrs.iter().collect();
            sorted.sort_by_key(|a| pick_priority(a));
            let port = info.get_port();
            // If the only advertised addresses are link-local IPv6
            // (`fe80::…`), tungstenite will reliably fail to connect
            // — the zone-id encoding inconsistency between rust-url
            // and tungstenite makes the URL un-parseable as a usable
            // target on macOS. Fall back to loopback. The dev-server
            // binds to `0.0.0.0` so 127.0.0.1:<port> works whenever
            // the client is on the same machine, which is the case
            // for desktop runtime-server clients and the iOS Simulator and
            // wgpu-sim. Real LAN clients (phones) get IPv4 in the
            // mDNS response so the fallback never triggers there.
            let only_link_local = !sorted.is_empty()
                && sorted.iter().all(|a| match a {
                    std::net::IpAddr::V4(_) => false,
                    std::net::IpAddr::V6(v) => is_ipv6_link_local(v),
                });
            let chosen = if only_link_local {
                eprintln!(
                    "[discover] mDNS returned only IPv6 link-local addresses; \
                     falling back to 127.0.0.1:{port} (assumes dev-server is \
                     on the same machine)",
                    port = port,
                );
                Some(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
            } else {
                sorted.first().copied().copied()
            };
            if let Some(chosen) = chosen {
                let url = match chosen {
                    std::net::IpAddr::V4(_) => format!("ws://{}:{}", chosen, port),
                    std::net::IpAddr::V6(_) => format!("ws://[{}]:{}", chosen, port),
                };
                found = Some(url);
                break;
            }
        }
    }

    // Best-effort cleanup. The daemon's thread shuts down on drop.
    let _ = daemon.shutdown();
    found
}

/// Loop forever (with a small per-attempt sleep) until a matching
/// dev-server is found. Use this from the transport thread when
/// you'd otherwise hard-code a URL — it makes the client resilient
/// to the dev-server starting after the app, restarting on a
/// different port, or moving to a new IP on the same Wi-Fi.
///
/// Each call browses for up to `per_attempt`. If nothing matches,
/// logs a hint and tries again. Returns when a match is found.
pub fn discover_blocking(expected_app_id: &str, per_attempt: Duration) -> String {
    loop {
        if let Some(url) = discover(expected_app_id, per_attempt) {
            return url;
        }
        eprintln!(
            "[discover] no dev-server with app_id={:?} yet — still browsing",
            expected_app_id
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

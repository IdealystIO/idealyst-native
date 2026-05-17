//! mDNS / DNS-SD discovery for the AAS dev-server.
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
            // Prefer an IPv4 address — tungstenite handles those
            // cleanly, IPv6 needs URL brackets, and dev networks
            // are almost always IPv4. Fall back to the first
            // address if there's no v4.
            let addrs = info.get_addresses();
            let host = addrs
                .iter()
                .find(|a| a.is_ipv4())
                .or_else(|| addrs.iter().next())
                .map(|a| a.to_string());
            if let Some(h) = host {
                let port = info.get_port();
                let url = if info
                    .get_addresses()
                    .iter()
                    .next()
                    .map(|a| a.is_ipv6())
                    .unwrap_or(false)
                    && h.contains(':')
                {
                    // IPv6 literal in a URL needs brackets.
                    format!("ws://[{}]:{}", h, port)
                } else {
                    format!("ws://{}:{}", h, port)
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

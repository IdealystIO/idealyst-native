//! Where the Inspector server is, and which app to open on, for this
//! build of the front end.
//!
//! In a browser the page was served BY the Inspector server, so the
//! socket is on the page's own host, and a launcher names the app to open
//! with `?app=<id>` (`idealyst dev --inspect` does). The desktop build
//! reads the same two facts from its environment, defaulting to the
//! server's default port.

use inspector_protocol::{DEFAULT_PORT, WS_PATH};

/// Overrides the desktop build's server URL (`ws://127.0.0.1:9719/ws`).
pub const URL_ENV: &str = "IDEALYST_INSPECT_URL";
/// Names an app id to attach to on launch (the desktop build's `?app=`).
pub const APP_ENV: &str = "IDEALYST_INSPECT_APP";

/// The Inspector socket URL.
#[cfg(target_arch = "wasm32")]
pub fn server_url() -> String {
    let host = web_sys::window().and_then(|w| w.location().host().ok()).unwrap_or_default();
    if host.is_empty() {
        format!("ws://127.0.0.1:{DEFAULT_PORT}{WS_PATH}")
    } else {
        format!("ws://{host}{WS_PATH}")
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn server_url() -> String {
    std::env::var(URL_ENV).unwrap_or_else(|_| format!("ws://127.0.0.1:{DEFAULT_PORT}{WS_PATH}"))
}

/// The app id to attach to on launch, if the launcher named one.
#[cfg(target_arch = "wasm32")]
pub fn initial_app() -> Option<String> {
    let search = web_sys::window().and_then(|w| w.location().search().ok())?;
    app_from_query(&search)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn initial_app() -> Option<String> {
    std::env::var(APP_ENV).ok().filter(|s| !s.is_empty())
}

/// `?app=Todo-48213&x=1` → `Todo-48213`, percent-decoded (an app's name,
/// and so its id, can hold any character).
pub fn app_from_query(search: &str) -> Option<String> {
    let raw = search.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix("app="))?;
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            (b'+', ..) => {
                out.push(b' ');
                i += 1;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_app_from_the_query() {
        assert_eq!(app_from_query("?app=Todo-48213").as_deref(), Some("Todo-48213"));
        assert_eq!(app_from_query("?x=1&app=Notes-2").as_deref(), Some("Notes-2"));
        assert_eq!(app_from_query("?app="), None);
        assert_eq!(app_from_query(""), None);
        assert_eq!(app_from_query("?app=My%20App-7").as_deref(), Some("My App-7"));
        assert_eq!(app_from_query("?app=Caf%C3%A9-1").as_deref(), Some("Café-1"));
    }
}

//! The session's control endpoint and its entry in the machine's session
//! registry (`dev_events::sessions`), so a tool — `idealyst mcp`, for an
//! agent — finds this session and drives it as it would one it started:
//! reads its state, requests a rebuild (the panel's `r`), and finds its
//! log. Stopping goes by the pid in the entry, so it works on a session
//! too wedged to answer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dev_events::sessions::{self, SessionEntry};

/// What the endpoint does for the session: read its state, and request a
/// rebuild (returning the request's number and the targets that took
/// it). Supplied by the caller so the request handling is testable
/// without a session.
pub struct Actions {
    pub status: Box<dyn Fn() -> Vec<serde_json::Value> + Send + Sync>,
    pub rebuild: Box<dyn Fn() -> crate::dev_log::Requested + Send + Sync>,
}

/// Answer one request: `(status code, JSON body)`. `authorization` is the
/// request's header, `has_origin` whether it carried an `Origin` header
/// (a browser page's request always does; a local tool's never).
pub fn handle(
    method: &str,
    path: &str,
    authorization: Option<&str>,
    has_origin: bool,
    entry: &SessionEntry,
    actions: &Actions,
) -> (u16, String) {
    if has_origin {
        return (403, r#"{"error":"a request from a web page is refused"}"#.into());
    }
    let expected = format!("Bearer {}", entry.token);
    if authorization != Some(expected.as_str()) {
        return (401, r#"{"error":"missing or wrong token; read it from this session's registry entry"}"#.into());
    }
    match (method, path) {
        ("GET", "/status") => {
            let body = serde_json::json!({ "session": entry.public(), "events": (actions.status)() });
            (200, body.to_string())
        }
        ("POST", "/rebuild") => {
            // `request` is what a wait on this rebuild's outcome names
            // (`RebuildTaken` events carry it).
            let r = (actions.rebuild)();
            let body = serde_json::json!({
                "accepted": r.targets.len(),
                "request": r.request,
                "targets": r.targets,
                "no_watch": entry.no_watch,
            });
            (if r.targets.is_empty() { 409 } else { 202 }, body.to_string())
        }
        _ => (404, r#"{"error":"GET /status or POST /rebuild"}"#.into()),
    }
}

/// Removes the session's registry entry when the session ends normally
/// (the Ctrl-C path, which exits the process, removes it from the
/// session's exit-file list instead).
pub struct Registration {
    path: PathBuf,
}

impl Registration {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Start the control endpoint on an ephemeral loopback port and register
/// the session (`entry` filled in but for its address and token, which
/// this supplies). `None` when there is no registry directory or the
/// endpoint cannot start: the session runs anyway, just undiscoverable.
pub fn start(entry: SessionEntry, actions: Actions) -> Option<Registration> {
    start_in(&sessions::registry_dir()?, entry, actions)
}

/// [`start`], registering into `registry`.
pub fn start_in(registry: &Path, mut entry: SessionEntry, actions: Actions) -> Option<Registration> {
    let server = tiny_http::Server::http("127.0.0.1:0").ok()?;
    let port = server.server_addr().to_ip()?.port();
    entry.control_url = format!("http://127.0.0.1:{port}");
    entry.token = token();
    let path = sessions::write(registry, &entry).ok()?;
    let entry = Arc::new(entry);
    std::thread::Builder::new()
        .name("dev-control".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                let header = |name: &str| {
                    request
                        .headers()
                        .iter()
                        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
                        .map(|h| h.value.as_str().to_string())
                };
                let authorization = header("authorization");
                let has_origin = header("origin").is_some();
                let method = request.method().as_str().to_string();
                let path = request.url().split('?').next().unwrap_or("").to_string();
                let (code, body) = handle(&method, &path, authorization.as_deref(), has_origin, &entry, &actions);
                let json = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("a valid header");
                let _ = request.respond(tiny_http::Response::from_string(body).with_status_code(code).with_header(json));
            }
        })
        .ok()?;
    Some(Registration { path })
}

/// 32 hex characters from the OS's randomness. A process that can read
/// `/dev/urandom` but not this user's entry file cannot guess it.
fn token() -> String {
    let mut bytes = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes));
    if read.is_err() {
        // No /dev/urandom (Windows): the std hasher's per-process random
        // keys, over the time and the pid.
        use std::hash::{BuildHasher, Hasher};
        for chunk in bytes.chunks_mut(8) {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
            h.write_u32(std::process::id());
            chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn actions(rebuilds: Arc<AtomicUsize>) -> Actions {
        Actions {
            status: Box::new(|| vec![serde_json::json!({"type": "session_started"})]),
            rebuild: Box::new(move || {
                let n = rebuilds.fetch_add(1, Ordering::SeqCst) as u64 + 1;
                crate::dev_log::Requested { request: n, targets: vec!["web".into()] }
            }),
        }
    }

    fn entry() -> SessionEntry {
        let mut e = SessionEntry::new(7, "/w/app", "http://127.0.0.1:0", "tok");
        e.app = "app".into();
        e.no_watch = true;
        e
    }

    #[test]
    fn the_token_is_required_and_a_web_page_is_refused() {
        let n = Arc::new(AtomicUsize::new(0));
        let a = actions(n.clone());
        let e = entry();
        assert_eq!(handle("POST", "/rebuild", None, false, &e, &a).0, 401);
        assert_eq!(handle("POST", "/rebuild", Some("Bearer nope"), false, &e, &a).0, 401);
        assert_eq!(handle("POST", "/rebuild", Some("Bearer tok"), true, &e, &a).0, 403, "a page carrying the token");
        assert_eq!(n.load(Ordering::SeqCst), 0, "a refused request rebuilt");
        let (code, body) = handle("POST", "/rebuild", Some("Bearer tok"), false, &e, &a);
        assert_eq!(code, 202);
        assert_eq!(n.load(Ordering::SeqCst), 1);
        assert!(body.contains(r#""accepted":1"#), "{body}");
        assert!(body.contains(r#""request":1"#), "the request's number is what a wait names: {body}");
        let (code, body) = handle("GET", "/status", Some("Bearer tok"), false, &e, &a);
        assert_eq!(code, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["session"]["app"], "app");
        assert!(v["session"].get("token").is_none(), "the status leaks the token");
        assert_eq!(v["events"][0]["type"], "session_started");
        assert_eq!(handle("DELETE", "/status", Some("Bearer tok"), false, &e, &a).0, 404);
    }

    /// The whole path a tool takes: find the entry, request through it.
    #[test]
    fn a_registered_session_is_driven_through_its_entry() {
        let registry = std::env::temp_dir().join(format!("idealyst-control-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&registry);
        let n = Arc::new(AtomicUsize::new(0));
        let reg = start_in(&registry, entry(), actions(n.clone())).expect("the endpoint starts");
        let found = sessions::read_all(&registry);
        assert_eq!(found.len(), 1);
        let (path, e) = &found[0];
        assert_eq!(path, reg.path());
        assert_eq!(e.token.len(), 32);
        let (code, _) = sessions::request(e, "POST", "/rebuild", Duration::from_secs(5)).unwrap();
        assert_eq!(code, 202);
        assert_eq!(n.load(Ordering::SeqCst), 1);
        let (code, body) = sessions::request(e, "GET", "/status", Duration::from_secs(5)).unwrap();
        assert_eq!(code, 200, "{body}");
        drop(reg);
        assert!(sessions::read_all(&registry).is_empty(), "the entry outlived the session");
        let _ = std::fs::remove_dir_all(&registry);
    }
}

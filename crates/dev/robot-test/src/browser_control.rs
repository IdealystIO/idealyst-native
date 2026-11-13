//! The test runner's control of the browser a web suite runs in.
//!
//! A page cannot resize its own window, so on web the robot's
//! `set_viewport` is answered by `idealyst test --web`, which holds the
//! browser's DevTools session: it serves this socket for the length of a
//! run and names it to the suite in [`ENV`]. One request per connection,
//! one JSON line each way:
//!
//! ```text
//! → {"set_viewport":{"width":1100,"height":900}}
//! ← {"ok":true}          or   {"err":"…"}
//! ```
//!
//! The resize goes through the runner's own DevTools session because
//! Chrome ties a metrics override to the session that set it: one set from
//! a short-lived connection is dropped when that connection closes.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

/// Names the control socket (`host:port`) in a suite's environment.
pub const ENV: &str = "IDEALYST_TEST_BROWSER_CONTROL";

/// The control socket this suite was given, if any.
pub fn from_env() -> Option<SocketAddr> {
    std::env::var(ENV).ok()?.parse().ok()
}

/// A request's line.
pub fn set_viewport_request(width: u32, height: u32) -> String {
    json!({ "set_viewport": { "width": width, "height": height } }).to_string()
}

/// The size a request line asks for; `None` for anything else.
pub fn parse_set_viewport(line: &str) -> Option<(u32, u32)> {
    let v: Value = serde_json::from_str(line).ok()?;
    let size = v.get("set_viewport")?;
    let dim = |k: &str| size.get(k)?.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0);
    Some((dim("width")?, dim("height")?))
}

/// A reply's line.
pub fn reply(result: Result<(), String>) -> String {
    match result {
        Ok(()) => json!({ "ok": true }).to_string(),
        Err(e) => json!({ "err": e }).to_string(),
    }
}

/// Ask the runner at `addr` to resize the page.
pub fn set_viewport(addr: SocketAddr, width: u32, height: u32) -> Result<(), String> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("the test runner's browser control at {addr}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
    writeln!(stream, "{}", set_viewport_request(width, height)).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| format!("the test runner did not answer: {e}"))?;
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| format!("the test runner's answer {line:?}: {e}"))?;
    match v.get("err").and_then(|e| e.as_str()) {
        Some(e) => Err(e.to_string()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips() {
        assert_eq!(parse_set_viewport(&set_viewport_request(1100, 900)), Some((1100, 900)));
        assert_eq!(parse_set_viewport(r#"{"set_viewport":{"width":0,"height":900}}"#), None);
        assert_eq!(parse_set_viewport("{}"), None);
    }

    #[test]
    fn the_client_reads_the_servers_reply() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for answer in [Ok(()), Err("no page".to_string())] {
                let (s, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
                assert_eq!(parse_set_viewport(line.trim()), Some((640, 480)));
                writeln!(&s, "{}", reply(answer)).unwrap();
            }
        });
        assert_eq!(set_viewport(addr, 640, 480), Ok(()));
        assert_eq!(set_viewport(addr, 640, 480), Err("no page".into()));
        server.join().unwrap();
    }
}

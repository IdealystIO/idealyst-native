//! Events across a process boundary.
//!
//! Part of the dev loop runs in child processes — the runtime-server
//! sidecar host decides and applies saves in its own process. Its events
//! belong in the parent's session like any other, so the child writes
//! each one to its stderr as a single marked line ([`encode`]) and the
//! parent, which already reads that pipe, turns marked lines back into
//! events ([`decode`]) and everything else into [`crate::DevEvent::Output`].
//!
//! The child opts in when the parent sets [`ENV`] in its environment
//! ([`reporter_from_env`]); run by hand, the same binary prints plain
//! lines as it always did.

use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::{DevEvent, Envelope, Reporter, Sink};

/// Set by a parent that reads the child's stderr and decodes events.
pub const ENV: &str = "IDEALYST_DEV_EVENTS";
/// The value of [`ENV`] asking for marked lines on stderr.
pub const ENV_STDERR: &str = "stderr";

/// Starts every marked line. A record separator, so it can never be the
/// start of a line a human or a compiler printed.
pub const MARK: &str = "\u{1e}idealyst-event ";

/// The marked line for `envelope`. The parent re-numbers the event into
/// its own sequence, so only the event itself is sent.
pub fn encode(event: &DevEvent) -> Option<String> {
    serde_json::to_string(event).ok().map(|json| format!("{MARK}{json}"))
}

/// The event a marked line carries, or `None` for any other line.
pub fn decode(line: &str) -> Option<DevEvent> {
    serde_json::from_str(line.strip_prefix(MARK)?).ok()
}

/// A child-side sink writing marked lines.
pub struct Marked<W: Write + Send> {
    out: Mutex<W>,
}

impl<W: Write + Send> Marked<W> {
    pub fn new(out: W) -> Self {
        Self { out: Mutex::new(out) }
    }
}

impl<W: Write + Send> Sink for Marked<W> {
    fn emit(&self, envelope: &Envelope) {
        let Some(line) = encode(&envelope.event) else { return };
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    }
}

/// The reporter a child process should use: marked lines on stderr when
/// its parent asked for them, plain lines otherwise.
pub fn reporter_from_env() -> Reporter {
    reporter_for(std::env::var(ENV).ok().as_deref())
}

fn reporter_for(value: Option<&str>) -> Reporter {
    if value == Some(ENV_STDERR) {
        let r = Reporter::new();
        r.add_sink(Arc::new(Marked::new(std::io::stderr())));
        r
    } else {
        Reporter::plain_stderr()
    }
}

/// Parent side: route one line of a child's output. A marked line is
/// re-emitted as the event it carries; anything else is `Output` from
/// `source`.
pub fn forward(reporter: &Reporter, source: &str, line: &str) {
    match decode(line) {
        Some(event) => reporter.emit(event),
        None => reporter.output(source, line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Queue;

    #[test]
    fn a_marked_line_round_trips_and_a_plain_line_is_output() {
        let event = DevEvent::OverlayPushed { target: "web".into(), sites: 2, ms: 14 };
        let line = encode(&event).unwrap();
        assert!(!line.contains('\n'), "a marked event must be one line");
        assert_eq!(decode(&line), Some(event.clone()));
        assert_eq!(decode("[runtime-server-host] hot-patch applied"), None);

        let r = Reporter::new();
        let q = Queue::new();
        r.add_sink(Arc::new(q.clone()));
        forward(&r, "sidecar", &line);
        forward(&r, "sidecar", "plain text");
        let got: Vec<DevEvent> = q.drain().into_iter().map(|e| e.event).collect();
        assert_eq!(
            got,
            vec![
                event,
                DevEvent::Output { source: "sidecar".into(), line: "plain text".into(), target: None }
            ]
        );
    }

    #[test]
    fn the_marked_sink_writes_one_decodable_line_per_event() {
        let sink = Arc::new(Marked::new(Vec::<u8>::new()));
        let r = Reporter::new();
        r.add_sink(sink.clone());
        r.log("runtime-server-host", "hot-patch applied");
        r.emit(DevEvent::Decided { target: "sidecar".into(), decision: crate::Decision::Unchanged });
        let bytes = sink.out.lock().unwrap().clone();
        let lines: Vec<&str> = std::str::from_utf8(&bytes).unwrap().lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| decode(l).is_some()), "{lines:?}");
    }
}

//! A check: a target's normal build of what is on disk, with nothing applied.
//!
//! The build's own events would describe a build to every consumer of the
//! session's stream: the status rows, the metrics records and the page's
//! badge would all show a rebuild that changes nothing they care about, and a
//! `diagnostic` from a check would put a compile error on a page that is
//! running fine. So a check's cargo reports to a private [`Reporter`] whose
//! only sink collects what the [`CheckReport`] needs, and the session hears
//! [`DevEvent::CheckStarted`] and [`DevEvent::CheckFinished`] alone.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::{CheckReport, DevEvent, Diagnostic, Envelope, Reporter, Sink};

#[derive(Default)]
struct Collected {
    compiled: Vec<String>,
    errors: Vec<Diagnostic>,
}

#[derive(Default)]
struct Collector(Mutex<Collected>);

impl Sink for Collector {
    fn emit(&self, envelope: &Envelope) {
        let Ok(mut c) = self.0.lock() else { return };
        match &envelope.event {
            DevEvent::UnitBuilt { package, kind, .. } => {
                let unit = format!("{package} ({kind})");
                if !c.compiled.contains(&unit) {
                    c.compiled.push(unit);
                }
            }
            DevEvent::Diagnostic { diagnostic, .. } if diagnostic.is_error() => {
                c.errors.push(Diagnostic { ansi: None, ..diagnostic.clone() });
            }
            _ => {}
        }
    }
}

/// Run `build` as check `request` of `target`, reporting it on `session`.
///
/// `build` is handed the reporter its cargo must report to, and answers with
/// why it failed. It runs on the caller's thread: a watcher's, between saves.
pub fn run(
    session: &Reporter,
    target: &str,
    request: u64,
    tests: Option<String>,
    build: impl FnOnce(&Reporter) -> Result<(), String>,
) -> CheckReport {
    session.emit(DevEvent::CheckStarted { target: target.into(), request, tests: tests.clone() });
    let started = Instant::now();
    let private = Reporter::new();
    let collector = Arc::new(Collector::default());
    private.add_sink(collector.clone());
    let result = build(&private);
    let collected = std::mem::take(&mut *collector.0.lock().unwrap_or_else(|p| p.into_inner()));
    let report = CheckReport {
        request,
        ok: result.is_ok(),
        compiled: collected.compiled,
        // The compiler said why: cargo's "could not compile" adds nothing.
        failure: result.err().filter(|_| collected.errors.is_empty()),
        errors: collected.errors,
        tests,
        ms: started.elapsed().as_millis() as u64,
    };
    session.emit(DevEvent::CheckFinished { target: target.into(), report: report.clone() });
    report
}

/// A finished check, as the terminal shows it: one line, then the errors.
pub fn summary(target: &str, report: &CheckReport) -> String {
    let what = if report.ok { "passed" } else { "FAILED" };
    let ms = report.ms;
    let mut out = format!("[check {target}] {what} in {ms} ms, {} unit(s) compiled", report.compiled.len());
    if !report.compiled.is_empty() {
        out.push_str(&format!(": {}", report.compiled.join(", ")));
    }
    if let Some(why) = &report.failure {
        out.push_str(&format!("\n{why}"));
    }
    for e in &report.errors {
        out.push('\n');
        out.push_str(e.rendered.trim_end());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(level: &str, line: u32) -> Diagnostic {
        Diagnostic {
            level: level.into(),
            message: "mismatched types".into(),
            code: Some("E0308".into()),
            file: Some("src/lib.rs".into()),
            line: Some(line),
            column: Some(9),
            package: Some("shared".into()),
            rendered: "error[E0308]: mismatched types\n".into(),
            ansi: Some("\u{1b}[31merror\u{1b}[0m".into()),
        }
    }

    /// The session's stream carries the check's start and end, and none of
    /// the build events a status view or the page would read as a build.
    #[test]
    fn a_checks_build_events_stay_out_of_the_sessions_stream() {
        let session = Reporter::new();
        let queue = Arc::new(crate::Queue::default());
        session.add_sink(queue.clone());
        let report = run(&session, "web", 3, None, |private| {
            private.emit(DevEvent::UnitBuilt {
                target: "web".into(),
                package: "shared".into(),
                version: "0.0.1".into(),
                kind: "lib".into(),
                ms: Some(4),
            });
            private.emit(DevEvent::UnitBuilt {
                target: "web".into(),
                package: "shared".into(),
                version: "0.0.1".into(),
                kind: "lib".into(),
                ms: Some(4),
            });
            private.emit(DevEvent::CargoProgress { target: "web".into(), compiled: 1, total: None, current: None });
            private.emit(DevEvent::Diagnostic { target: "web".into(), diagnostic: diagnostic("warning", 1) });
            Ok(())
        });
        assert!(report.ok);
        assert_eq!(report.compiled, ["shared (lib)"], "a unit is listed once");
        assert!(report.errors.is_empty(), "a warning is not an error");
        let kinds: Vec<_> = queue.drain().into_iter().map(|e| serde_json::to_value(&e).unwrap()["type"].clone()).collect();
        assert_eq!(kinds, ["check_started", "check_finished"]);
    }

    /// A compile error answers with where it is, without colour codes, and
    /// cargo's own "could not compile" is not reported on top of it.
    #[test]
    fn a_compile_error_is_located_and_replaces_cargos_failure() {
        let session = Reporter::new();
        let report = run(&session, "server", 1, None, |private| {
            private.emit(DevEvent::Diagnostic { target: "server".into(), diagnostic: diagnostic("error", 12) });
            Err("server build failed".into())
        });
        assert!(!report.ok);
        assert_eq!(report.failure, None);
        assert_eq!(report.errors[0].location().as_deref(), Some("src/lib.rs:12:9"));
        assert_eq!(report.errors[0].ansi, None);
    }

    /// With no diagnostic, the failure itself is the only explanation.
    #[test]
    fn a_failure_with_no_diagnostic_keeps_its_reason() {
        let session = Reporter::new();
        let report = run(&session, "server", 1, None, |_| Err("no package `nope`".into()));
        assert!(!report.ok);
        assert_eq!(report.failure.as_deref(), Some("no package `nope`"));
        assert!(summary("server", &report).contains("no package `nope`"));
    }
}

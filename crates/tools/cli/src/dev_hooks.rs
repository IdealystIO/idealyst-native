//! `[hooks]` in `dev.toml`: shell commands run on dev-session events.
//!
//! The simplest external consumer of the event stream — no HTTP, no
//! file tailing. [`HookSink`] is an ordinary `dev_events::Sink`
//! subscribed to the session reporter; see `DevConfig::hooks` for the
//! configuration and `docs/hot-reload.md#hooking-into-a-dev-session`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use dev_events::{BuildOutcome, DevEvent, Envelope, SidecarUpdate, Sink};

/// Where a failed hook reports. Its warnings never trigger hooks, so a
/// broken `warning` hook cannot feed itself.
const SOURCE: &str = "dev hooks";

/// The kinds `event` answers to: its `type`, plus the derived
/// `build_failed` and `patched`.
pub fn kinds(event: &DevEvent) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::to_value(event) {
        if let Some(t) = v["type"].as_str() {
            out.push(t.to_string());
        }
    }
    match event {
        DevEvent::BuildFinished { outcome: BuildOutcome::Failed { .. }, .. } => {
            out.push("build_failed".into())
        }
        DevEvent::PatchBuilt { .. }
        | DevEvent::OverlayPushed { .. }
        | DevEvent::SidecarApplied { how: SidecarUpdate::HotPatch, .. } => out.push("patched".into()),
        _ => {}
    }
    out
}

/// Runs the configured command for each event whose kind has one.
pub struct HookSink {
    hooks: BTreeMap<String, String>,
    dir: PathBuf,
}

impl HookSink {
    /// `None` when no hooks are configured: no sink, no cost.
    pub fn new(hooks: BTreeMap<String, String>, dir: PathBuf) -> Option<Self> {
        (!hooks.is_empty()).then_some(Self { hooks, dir })
    }
}

impl Sink for HookSink {
    fn emit(&self, envelope: &Envelope) {
        if let DevEvent::Warning { source, .. } = &envelope.event {
            if source == SOURCE {
                return;
            }
        }
        for kind in kinds(&envelope.event) {
            let Some(command) = self.hooks.get(&kind) else { continue };
            let Ok(json) = serde_json::to_string(envelope) else { continue };
            let (command, dir) = (command.clone(), self.dir.clone());
            // Off the emitting thread: a hook must never stall the dev
            // loop, and this runs with the reporter's fan-out lock held —
            // the failure report below has to come from elsewhere.
            std::thread::spawn(move || {
                if let Err(why) = run(&command, &dir, &kind, &json) {
                    dev_events::global()
                        .warn(SOURCE, format!("`{kind}` hook `{command}` failed: {why}"));
                }
            });
        }
    }
}

fn run(command: &str, dir: &std::path::Path, kind: &str, json: &str) -> Result<(), String> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    let mut child = cmd
        .current_dir(dir)
        .env("IDEALYST_EVENT_KIND", kind)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        // A hook that does not read stdin closes it early; that is its
        // business, not a failure.
        let _ = writeln!(stdin, "{json}");
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(format!("{} {}", out.status, stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn wait_for(path: &std::path::Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(s) = std::fs::read_to_string(path) {
                if s.ends_with('\n') {
                    return s;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the hook never wrote {}", path.display());
    }

    #[test]
    fn a_hook_receives_the_event_json_on_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let mut hooks = BTreeMap::new();
        hooks.insert("build_failed".to_string(), "cat > failed.json".to_string());
        hooks.insert("patched".to_string(), "printf '%s\\n' \"$IDEALYST_EVENT_KIND\" > kind.txt".to_string());
        let sink = HookSink::new(hooks, dir.path().to_path_buf()).unwrap();
        let r = dev_events::Reporter::new();
        r.add_sink(Arc::new(sink));
        r.emit(DevEvent::BuildFinished {
            target: "web".into(),
            outcome: BuildOutcome::Failed { error: "E0308".into() },
            ms: 7,
        });
        r.emit(DevEvent::OverlayPushed { target: "web".into(), sites: 1, ms: 2 });

        let json = wait_for(&dir.path().join("failed.json"));
        let got: Envelope = serde_json::from_str(json.trim()).unwrap();
        assert!(matches!(
            got.event,
            DevEvent::BuildFinished { outcome: BuildOutcome::Failed { .. }, .. }
        ));
        assert_eq!(got.v, dev_events::SCHEMA_VERSION);
        assert_eq!(wait_for(&dir.path().join("kind.txt")), "patched\n");
    }

    #[test]
    fn the_kinds_include_the_type_and_the_derived_names() {
        let failed = DevEvent::BuildFinished {
            target: "web".into(),
            outcome: BuildOutcome::Failed { error: "e".into() },
            ms: 1,
        };
        assert_eq!(kinds(&failed), vec!["build_finished", "build_failed"]);
        let patched = DevEvent::SidecarApplied {
            target: "runtime-server".into(),
            how: SidecarUpdate::HotPatch,
            ms: 1,
            reason: None,
        };
        assert_eq!(kinds(&patched), vec!["sidecar_applied", "patched"]);
    }

    #[test]
    fn no_hooks_no_sink() {
        assert!(HookSink::new(BTreeMap::new(), PathBuf::from(".")).is_none());
    }
}

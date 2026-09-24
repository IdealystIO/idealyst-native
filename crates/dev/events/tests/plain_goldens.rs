//! The plain sink's lines, frozen.
//!
//! Each event that replaced an `eprintln!` must render as exactly that
//! line — terminals, CI logs, the MCP dev-runner's log files and the
//! CLI's end-to-end tests read them. The expected strings below are the
//! format strings those `eprintln!`s used, filled in by hand; a change
//! here is a change every one of those consumers sees.

use std::sync::Arc;

use dev_events::{
    plain, BuildCause, BuildOutcome, CrateTiming, Decision, DevEvent, Diagnostic, HotTier, Mode,
    PageAck, PlainLines, Reporter, ServerKind, Timing, Verbosity,
};

fn web() -> String {
    "web".to_string()
}

fn t(name: &str, ms: u64) -> Timing {
    Timing { name: name.into(), ms }
}

fn line(event: DevEvent) -> Option<String> {
    plain::render(&event)
}

#[test]
fn the_initial_build_lines() {
    assert_eq!(
        line(DevEvent::SessionStarted {
            app: "Hotreload Lab".into(),
            targets: vec!["web".into()],
            mode: Mode::Local,
            hot_tier: HotTier::Armed,
            log_file: None,
        })
        .as_deref(),
        Some("[dev] local mode, targets: web")
    );
    assert_eq!(
        line(DevEvent::BuildStarted { target: web(), cause: BuildCause::Initial }).as_deref(),
        Some("[dev-reload] initial build…")
    );
    assert_eq!(
        line(DevEvent::BuildTimed {
            target: web(),
            stages: vec![
                t("cargo", 41_230),
                t("hotpatch-base-prep", 2_010),
                t("wasm-bindgen", 5_500),
                t("stage+fingerprint", 40),
            ],
            total_ms: 48_780,
        })
        .as_deref(),
        Some(
            "[build-web] timing: total 48.78s — cargo 41.23s | wasm-bindgen 5.50s | \
             hotpatch-base-prep 2.01s | stage+fingerprint 0.04s"
        )
    );
    assert_eq!(
        line(DevEvent::Watching {
            target: web(),
            roots: vec!["/lab/src".into(), "/lab/Cargo.toml".into()],
            rewatch: false,
        })
        .as_deref(),
        Some("[dev-reload] watching /lab/src, /lab/Cargo.toml for changes")
    );
    assert_eq!(
        line(DevEvent::ServerReady {
            target: web(),
            kind: ServerKind::Livereload,
            url: "http://0.0.0.0:8082".into(),
        })
        .as_deref(),
        Some("[dev web] livereload HTTP at http://0.0.0.0:8082")
    );
    // The first build's success had no line of its own.
    assert_eq!(
        line(DevEvent::BuildFinished {
            target: web(),
            outcome: BuildOutcome::Ready { gen: 1 },
            ms: 48_900,
        }),
        None
    );
}

#[test]
fn the_overlay_patch_lines() {
    assert_eq!(
        line(DevEvent::ChangeDetected {
            target: web(),
            paths: vec!["/lab/src/app.rs".into()],
            crates: vec!["hotreload-lab".into()],
            folded: 0,
        }),
        None,
        "change detection had no line of its own"
    );
    assert_eq!(
        line(DevEvent::Decided { target: web(), decision: Decision::Overlay { sites: 1 } }),
        None
    );
    assert_eq!(
        line(DevEvent::OverlayPushed { target: web(), sites: 1, ms: 12 }).as_deref(),
        Some("[dev] patched 1 site(s) in 12 ms, no rebuild")
    );
    assert_eq!(
        line(DevEvent::PageAck {
            target: web(),
            ack: PageAck::Overlay { applied: Some(1), refused: Some(0) },
        }),
        None,
        "the page's ack is new, and must not add a line to a plain terminal"
    );
    assert_eq!(
        line(DevEvent::Decided { target: web(), decision: Decision::Unchanged }).as_deref(),
        Some("[dev] no UI or code change in this save, no rebuild")
    );
}

#[test]
fn the_hot_patch_lines() {
    let steps = vec![t("cargo", 402), t("link", 31), t("table", 9), t("strip", 4)];
    // One crate: no per-crate list.
    assert_eq!(
        line(DevEvent::PatchBuilt {
            target: web(),
            files: vec!["src/app.rs".into()],
            crates: vec![CrateTiming { name: "hotreload_lab".into(), ms: Some(402) }],
            redirected: 3,
            steps: steps.clone(),
            skipped: vec![],
            bytes: 18_433,
            ms: 446,
        })
        .as_deref(),
        Some(
            "[hotpatch] src/app.rs · 3 function(s) redirected · \
             cargo 402ms · link 31ms · table 9ms · strip 4ms (total 446ms)"
        )
    );
    // Several crates replaying concurrently name the slowest, and
    // crates the base never compiled are listed.
    assert_eq!(
        line(DevEvent::PatchBuilt {
            target: web(),
            files: vec!["lab-shared/src/lib.rs".into(), "src/app.rs".into()],
            crates: vec![
                CrateTiming { name: "lab_shared".into(), ms: Some(380) },
                CrateTiming { name: "hotreload_lab".into(), ms: None },
            ],
            redirected: 5,
            steps,
            skipped: vec!["lab_server".into()],
            bytes: 30_000,
            ms: 446,
        })
        .as_deref(),
        Some(
            "[hotpatch] lab-shared/src/lib.rs, src/app.rs · 5 function(s) redirected · \
             cargo 402ms · link 31ms · table 9ms · strip 4ms (total 446ms) \
             (not in the wasm build: lab_server) [lab_shared 380ms, hotreload_lab reused]"
        )
    );
    assert_eq!(
        line(DevEvent::PatchFailed {
            target: web(),
            files: vec!["src/app.rs".into()],
            reason: "no capture".into(),
        })
        .as_deref(),
        Some("[hotpatch] src/app.rs changed inside function bodies, but no patch: no capture; rebuilding")
    );
}

#[test]
fn the_rebuild_lines() {
    assert_eq!(
        line(DevEvent::Decided {
            target: web(),
            decision: Decision::Rebuild {
                reason: Some("src/app.rs changed outside its function bodies".into()),
            },
        })
        .as_deref(),
        Some("[dev] rebuilding: src/app.rs changed outside its function bodies")
    );
    assert_eq!(
        line(DevEvent::Decided { target: web(), decision: Decision::Rebuild { reason: None } }),
        None
    );
    assert_eq!(
        line(DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 0 } })
            .as_deref(),
        Some("[dev-reload] change detected, rebuilding…")
    );
    assert_eq!(
        line(DevEvent::BuildStarted { target: web(), cause: BuildCause::Save { folded: 2 } })
            .as_deref(),
        Some("[dev-reload] change detected (+2 more), rebuilding…")
    );
    assert_eq!(
        line(DevEvent::BuildFinished {
            target: web(),
            outcome: BuildOutcome::Reloaded { gen: 4 },
            ms: 9_000,
        })
        .as_deref(),
        Some("[dev-reload] rebuilt — gen=4")
    );
    assert_eq!(
        line(DevEvent::BuildFinished { target: web(), outcome: BuildOutcome::Unchanged, ms: 300 })
            .as_deref(),
        Some("[dev-reload] wasm unchanged — packaging skipped, nothing to reload")
    );
    assert_eq!(
        line(DevEvent::BuildFinished {
            target: web(),
            outcome: BuildOutcome::PremintRefreshed { gen: 5 },
            ms: 300,
        })
        .as_deref(),
        Some("[dev-reload] wasm unchanged, premint refreshed — gen=5")
    );
    assert_eq!(
        line(DevEvent::Watching {
            target: web(),
            roots: vec!["/lab/src".into()],
            rewatch: true,
        })
        .as_deref(),
        Some("[dev-reload] dependencies changed — now watching /lab/src")
    );
    // A one-shot build (runtime-server mode's thin client) never had a
    // line; its caller announces it.
    assert_eq!(line(DevEvent::BuildStarted { target: web(), cause: BuildCause::OneShot }), None);
    // A labelled watcher (the full-stack server's) keeps its own lines.
    let server = "server".to_string();
    assert_eq!(
        line(DevEvent::BuildStarted { target: server.clone(), cause: BuildCause::Save { folded: 1 } })
            .as_deref(),
        Some("[dev-reload server] change detected (+1 more)")
    );
    assert_eq!(
        line(DevEvent::BuildFinished {
            target: server.clone(),
            outcome: BuildOutcome::Reloaded { gen: 2 },
            ms: 1,
        })
        .as_deref(),
        Some("[dev-reload server] regen complete — gen=2")
    );
    assert_eq!(
        line(DevEvent::BuildFinished { target: server.clone(), outcome: BuildOutcome::Unchanged, ms: 1 })
            .as_deref(),
        Some("[dev-reload server] rebuilt, artifact unchanged — nothing to do")
    );
    assert_eq!(
        line(DevEvent::Watching { target: server, roots: vec!["/s".into()], rewatch: false })
            .as_deref(),
        Some("[dev-reload server] watching /s")
    );
}

#[test]
fn the_error_lines() {
    assert_eq!(
        line(DevEvent::BuildFinished {
            target: web(),
            outcome: BuildOutcome::Failed { error: "cargo exited with exit status: 101".into() },
            ms: 3_000,
        })
        .as_deref(),
        Some("[dev-reload] rebuild failed: cargo exited with exit status: 101")
    );
    let rendered = "error[E0308]: mismatched types\n --> src/app.rs:3:18\n";
    let diagnostic = Diagnostic {
        level: "error".into(),
        message: "mismatched types".into(),
        code: Some("E0308".into()),
        file: Some("src/app.rs".into()),
        line: Some(3),
        column: Some(18),
        package: None,
        rendered: rendered.into(),
        ansi: Some(format!("\x1b[1m\x1b[91m{rendered}\x1b[0m")),
    };
    // What cargo printed itself: the coloured rendering, when colour was on.
    assert_eq!(
        line(DevEvent::Diagnostic { target: web(), diagnostic: diagnostic.clone() }).as_deref(),
        Some("\x1b[1m\x1b[91merror[E0308]: mismatched types\n --> src/app.rs:3:18\n\x1b[0m")
    );
    assert_eq!(
        line(DevEvent::Diagnostic {
            target: web(),
            diagnostic: Diagnostic { ansi: None, ..diagnostic },
        })
        .as_deref(),
        Some("error[E0308]: mismatched types\n --> src/app.rs:3:18")
    );
    assert_eq!(
        line(DevEvent::Error {
            source: "dev-reload".into(),
            message: "could not start file watcher: too many open files".into(),
        })
        .as_deref(),
        Some("[dev-reload] could not start file watcher: too many open files")
    );
    assert_eq!(
        line(DevEvent::Output {
            source: "cargo".into(),
            line: "error: could not compile `app` due to 1 previous error".into(),
        })
        .as_deref(),
        Some("error: could not compile `app` due to 1 previous error"),
        "subprocess output is passed through verbatim"
    );
}

#[test]
fn the_plain_sink_writes_exactly_the_legacy_lines_and_nothing_else() {
    let sink = Arc::new(PlainLines::new(Vec::<u8>::new(), Verbosity::Legacy));
    let r = Reporter::new();
    r.add_sink(sink.clone());
    r.emit(DevEvent::ChangeDetected {
        target: web(),
        paths: vec!["/lab/src/app.rs".into()],
        crates: vec![],
        folded: 0,
    });
    r.emit(DevEvent::StageStarted { target: web(), stage: "cargo".into() });
    r.emit(DevEvent::CargoProgress { target: web(), compiled: 3, total: Some(9), current: None });
    r.log("dev web", "robot relay URL injected (ws://127.0.0.1:5555)");
    r.emit(DevEvent::PageAck { target: web(), ack: PageAck::Connected { gen: 1 } });
    drop(r);
    let sink = Arc::try_unwrap(sink).ok().expect("the reporter was the only other holder");
    assert_eq!(
        String::from_utf8(sink.into_inner()).unwrap(),
        "[dev web] robot relay URL injected (ws://127.0.0.1:5555)\n"
    );
}

#[test]
fn the_session_log_keeps_the_facts_a_terminal_never_showed() {
    let sink = Arc::new(PlainLines::new(Vec::<u8>::new(), Verbosity::Verbose));
    let r = Reporter::new();
    r.add_sink(sink.clone());
    r.emit(DevEvent::PageAck {
        target: web(),
        ack: PageAck::HotPatch { redirected: Some(3), carried: Some(12) },
    });
    r.emit(DevEvent::StageFinished { target: web(), stage: "wasm-bindgen".into(), ms: 5_500 });
    drop(r);
    let sink = Arc::try_unwrap(sink).ok().unwrap();
    let text = String::from_utf8(sink.into_inner()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(
        lines[0].ends_with(
            "[web page] hot patch applied: 3 function(s) redirected, 12 signal value(s) carried"
        ),
        "{text}"
    );
    assert!(lines[1].ends_with("[web] wasm-bindgen 5500 ms"), "{text}");
    assert!(lines[0].trim_start().starts_with("0."), "a session-time prefix: {text}");
}

#[test]
fn the_runtime_server_host_lines() {
    use dev_events::SidecarUpdate;
    let rs = || "runtime-server".to_string();
    assert_eq!(
        line(DevEvent::SidecarApplied { target: rs(), how: SidecarUpdate::HotPatch, ms: 412, reason: None })
            .as_deref(),
        Some("[runtime-server-host] hot-patch applied in 412ms")
    );
    assert_eq!(
        line(DevEvent::SidecarApplied {
            target: rs(),
            how: SidecarUpdate::Respawn,
            ms: 5210,
            reason: Some("force_respawn".into()),
        })
        .as_deref(),
        Some("[runtime-server-host] respawn applied in 5210ms (force_respawn)")
    );
    // The sidecar applying an overlay patch keeps its own legacy line
    // (a `Log`); the typed ack beside it adds nothing to a terminal.
    assert_eq!(
        line(DevEvent::PageAck {
            target: rs(),
            ack: PageAck::Overlay { applied: Some(2), refused: Some(0) },
        }),
        None
    );
}

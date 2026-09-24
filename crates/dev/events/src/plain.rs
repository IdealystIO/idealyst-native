//! Events as the text lines `idealyst dev` printed before events existed.
//!
//! [`render`] is a compatibility contract, not a presentation choice:
//! scripts, CI logs, the MCP dev-runner's log files and the CLI's own
//! end-to-end tests match these lines, so an event that replaced an
//! `eprintln!` renders as exactly that `eprintln!`'s text. An event that
//! never had a line (a page ack, a stage boundary, cargo progress)
//! renders as nothing here, so a plain terminal session looks the same as
//! it did — those facts appear in [`render_verbose`] (the session log
//! file) and in the other sinks.
//!
//! The goldens in `tests/plain_goldens.rs` hold the lines to their old
//! text.

use crate::{
    BuildCause, BuildOutcome, CrateTiming, Decision, DevEvent, Envelope, PageAck, ServerKind,
    SidecarUpdate, StreamRoute, Timing, SERVER_OUTPUT_SOURCE, SERVER_TARGET,
};

/// The target whose lines carry the watcher's bare `[dev-reload]` prefix.
/// Every other watcher labels itself (`[dev-reload server]`).
const WEB: &str = "web";

/// The legacy line for `event`, or `None` when it never had one.
pub fn render(event: &DevEvent) -> Option<String> {
    Some(match event {
        DevEvent::SessionStarted { targets, mode, .. } => {
            format!("[dev] {} mode, targets: {}", mode.as_str(), targets.join(", "))
        }
        DevEvent::ServerReady { target, kind, url } => match kind {
            ServerKind::Livereload => format!("[dev {target}] livereload HTTP at {url}"),
            ServerKind::RuntimeServerBridged => {
                format!("[dev {target}] runtime-server-bridged HTTP at {url}")
            }
            ServerKind::ReloadStream => format!("[dev-http] reload/overlay stream on {url}"),
            // Announced by its own line, which carries the pid.
            ServerKind::FullStack => return None,
            // New with the event stream: a plain terminal stays as it was.
            // The URL is in the session log and `.idealyst/events.url`.
            ServerKind::Events => return None,
        },
        // New: which way a full-stack page reaches the stream decides
        // whether it gets livereload at all in a container.
        DevEvent::StreamRoute { target, route, url } => match route {
            StreamRoute::SameOrigin => {
                format!("[dev {target}] page dev stream: same-origin through the app server ({url})")
            }
            StreamRoute::Port => format!(
                "[dev {target}] page dev stream: {url} (the app server does not proxy /__idealyst/*)"
            ),
        },
        DevEvent::Watching { target, roots, rewatch } => {
            let list = roots.join(", ");
            match (target.as_str(), rewatch) {
                (WEB, false) => format!("[dev-reload] watching {list} for changes"),
                (WEB, true) => format!("[dev-reload] dependencies changed — now watching {list}"),
                (label, _) => format!("[dev-reload {label}] watching {list}"),
            }
        }
        DevEvent::ChangeDetected { .. } => return None,
        DevEvent::Decided { decision, .. } => match decision {
            Decision::Unchanged => "[dev] no UI or code change in this save, no rebuild".into(),
            Decision::Rebuild { reason: Some(why) } => format!("[dev] rebuilding: {why}"),
            Decision::Rebuild { reason: None }
            | Decision::Overlay { .. }
            | Decision::HotPatch { .. } => return None,
        },
        DevEvent::OverlayPushed { sites, ms, .. } => {
            format!("[dev] patched {sites} site(s) in {ms} ms, no rebuild")
        }
        DevEvent::PatchBuilt { files, redirected, steps, crates, skipped, .. } => format!(
            "[hotpatch] {} · {redirected} function(s) redirected · {}",
            files.join(", "),
            patch_timing_line(steps, crates, skipped),
        ),
        DevEvent::PatchFailed { files, reason, .. } => format!(
            "[hotpatch] {} changed inside function bodies, but no patch: {reason}; rebuilding",
            files.join(", "),
        ),
        DevEvent::BuildStarted { target, cause } => match (target.as_str(), cause) {
            (WEB, BuildCause::Initial) => "[dev-reload] initial build…".into(),
            (WEB, BuildCause::Save { folded: 0 }) => {
                "[dev-reload] change detected, rebuilding…".into()
            }
            (WEB, BuildCause::Save { folded }) => {
                format!("[dev-reload] change detected (+{folded} more), rebuilding…")
            }
            (WEB, BuildCause::Forced) => "[dev-reload] rebuild requested, rebuilding…".into(),
            (label, BuildCause::Save { folded: 0 }) => {
                format!("[dev-reload {label}] change detected")
            }
            (label, BuildCause::Save { folded }) => {
                format!("[dev-reload {label}] change detected (+{folded} more)")
            }
            (label, BuildCause::Forced) => format!("[dev-reload {label}] rebuild requested"),
            // New with the server's typed build. Its first build used to be
            // `cargo run`'s own output and nothing else, so a plain
            // terminal could not tell a server was being built at all.
            (SERVER_TARGET, BuildCause::Initial) => "[dev-reload server] initial build…".into(),
            // Never had a line: the caller announces what it builds.
            (_, BuildCause::Initial | BuildCause::OneShot) => return None,
        },
        DevEvent::StageStarted { .. }
        | DevEvent::StageFinished { .. }
        | DevEvent::CargoProgress { .. } => return None,
        DevEvent::Diagnostic { diagnostic, .. } => diagnostic
            .ansi
            .as_deref()
            .unwrap_or(&diagnostic.rendered)
            .trim_end_matches('\n')
            .to_string(),
        DevEvent::BuildTimed { stages, total_ms, .. } => {
            if stages.is_empty() {
                return None;
            }
            format!("[build-web] timing: total {} — {}", secs(*total_ms), stage_list(stages))
        }
        DevEvent::BuildFinished { target, outcome, ms } => match (target.as_str(), outcome) {
            // The web bundle's has its `[build-web] timing` line; the
            // server's build has nothing else that says it finished.
            (SERVER_TARGET, BuildOutcome::Ready { .. }) => {
                format!("[dev-reload server] initial build done in {}", secs(*ms))
            }
            (_, BuildOutcome::Ready { .. }) => return None,
            (WEB, BuildOutcome::Reloaded { gen }) => format!("[dev-reload] rebuilt — gen={gen}"),
            (WEB, BuildOutcome::Unchanged) => {
                "[dev-reload] wasm unchanged — packaging skipped, nothing to reload".into()
            }
            (WEB, BuildOutcome::PremintRefreshed { gen }) => {
                format!("[dev-reload] wasm unchanged, premint refreshed — gen={gen}")
            }
            (WEB, BuildOutcome::Failed { error }) => format!("[dev-reload] rebuild failed: {error}"),
            (label, BuildOutcome::Reloaded { gen } | BuildOutcome::PremintRefreshed { gen }) => {
                format!("[dev-reload {label}] regen complete — gen={gen}")
            }
            (label, BuildOutcome::Unchanged) => {
                format!("[dev-reload {label}] rebuilt, artifact unchanged — nothing to do")
            }
            (label, BuildOutcome::Failed { error }) => {
                format!("[dev-reload {label}] regen failed: {error}")
            }
        },
        DevEvent::PageAck { .. } => return None,
        DevEvent::SidecarApplied { how, ms, reason, .. } => match how {
            SidecarUpdate::HotPatch => format!("[runtime-server-host] hot-patch applied in {ms}ms"),
            SidecarUpdate::Respawn => format!(
                "[runtime-server-host] respawn applied in {ms}ms ({})",
                reason.as_deref().unwrap_or("rebuild")
            ),
        },
        DevEvent::Warning { source, message }
        | DevEvent::Error { source, message }
        | DevEvent::Log { source, line: message } => format!("[{source}] {message}"),
        // The full-stack server's own output is not the dev loop's: tagged,
        // so a log reader can tell a request line from a build line. It
        // used to reach the terminal straight from the inherited pipe.
        DevEvent::Output { source, line, .. } if source == SERVER_OUTPUT_SOURCE => {
            if line.starts_with("[server]") {
                line.clone()
            } else {
                format!("[server] {line}")
            }
        }
        DevEvent::Output { line, .. } => line.clone(),
    })
}

/// Every event worth a line in the session log, each prefixed with its
/// session time: the legacy line where there is one, a description of
/// the fact otherwise. Cargo progress is left out — cargo's own
/// `Compiling …` lines, which arrive as [`DevEvent::Output`], already say
/// the same thing once per crate.
pub fn render_verbose(envelope: &Envelope) -> Option<String> {
    let body = match render(&envelope.event) {
        Some(line) => line,
        None => describe(&envelope.event)?,
    };
    Some(format!("{:>9.3}s {body}", envelope.at_ms as f64 / 1000.0))
}

/// The fact behind an event with no legacy line.
fn describe(event: &DevEvent) -> Option<String> {
    Some(match event {
        DevEvent::ServerReady { target, url, .. } => format!("[{target}] serving at {url}"),
        DevEvent::ChangeDetected { target, paths, folded, .. } => {
            let more = if *folded > 0 { format!(" (+{folded} more batches)") } else { String::new() };
            format!("[{target}] changed: {}{more}", paths.join(", "))
        }
        DevEvent::Decided { target, decision } => match decision {
            Decision::Overlay { sites } => format!("[{target}] decided: overlay ({sites} site(s))"),
            Decision::HotPatch { crates, .. } => {
                format!("[{target}] decided: hot patch ({})", crates.join(", "))
            }
            Decision::Rebuild { .. } => format!("[{target}] decided: rebuild"),
            Decision::Unchanged => return None,
        },
        DevEvent::StageStarted { .. } => return None,
        DevEvent::StageFinished { target, stage, ms } => format!("[{target}] {stage} {ms} ms"),
        DevEvent::BuildStarted { target, cause: BuildCause::Initial | BuildCause::OneShot } => {
            format!("[{target}] build")
        }
        DevEvent::BuildFinished { target, outcome: BuildOutcome::Ready { gen }, ms } => {
            format!("[{target}] ready (gen {gen}) in {ms} ms")
        }
        DevEvent::PageAck { target, ack } => format!("[{target} page] {}", describe_ack(ack)),
        _ => return None,
    })
}

/// A page ack in words.
pub fn describe_ack(ack: &PageAck) -> String {
    match ack {
        PageAck::Connected { gen } => format!("connected (gen {gen})"),
        PageAck::Reloading { gen } => format!("reloading onto gen {gen}"),
        PageAck::Overlay { applied, refused } => match (applied, refused) {
            (Some(a), Some(r)) => format!("overlay applied: {a} updated, {r} waiting for a render"),
            _ => "overlay applied".into(),
        },
        PageAck::HotPatch { redirected, carried } => {
            let mut s = "hot patch applied".to_string();
            if let Some(n) = redirected {
                s.push_str(&format!(": {n} function(s) redirected"));
            }
            if let Some(n) = carried {
                s.push_str(&format!(", {n} signal value(s) carried"));
            }
            s
        }
        PageAck::Failed { what, error } => format!("{what} failed: {error}"),
    }
}

/// `1.23s` — two decimals, as the bundler always printed its timings.
fn secs(ms: u64) -> String {
    format!("{:.2}s", ms as f64 / 1000.0)
}

/// `cargo 12.30s | wasm-bindgen 1.10s`, longest first.
fn stage_list(stages: &[Timing]) -> String {
    let mut sorted: Vec<&Timing> = stages.iter().collect();
    // Stable, so equal durations keep the order they ran in.
    sorted.sort_by(|a, b| b.ms.cmp(&a.ms));
    sorted
        .iter()
        .map(|t| format!("{} {}", t.name, secs(t.ms)))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The hot patch's timing summary: each step in order, the total, any
/// crates the base never compiled, and — when several crates replayed
/// concurrently — which one was the slowest.
///
/// This is THE formatter: `build_web::hotpatch_build::BuiltPatch::
/// timing_line` delegates here, so the line the bundler used to print and
/// the event's rendering cannot drift apart.
pub fn patch_timing_line(steps: &[Timing], crates: &[CrateTiming], skipped: &[String]) -> String {
    let parts: Vec<String> = steps.iter().map(|t| format!("{} {}ms", t.name, t.ms)).collect();
    let total: u64 = steps.iter().map(|t| t.ms).sum();
    let mut line = format!("{} (total {total}ms)", parts.join(" · "));
    if !skipped.is_empty() {
        line = format!("{line} (not in the wasm build: {})", skipped.join(", "));
    }
    if crates.len() < 2 {
        return line;
    }
    let per_crate: Vec<String> = crates
        .iter()
        .map(|c| match c.ms {
            Some(ms) => format!("{} {ms}ms", c.name),
            None => format!("{} reused", c.name),
        })
        .collect();
    format!("{line} [{}]", per_crate.join(", "))
}

/// Remove ANSI escape sequences (CSI `ESC [ … final` and OSC `ESC ] … BEL`).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // Parameter and intermediate bytes, then one final byte
                // in `@`..=`~`.
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_colour_and_keeps_text() {
        let s = "\x1b[1m\x1b[91merror[E0308]\x1b[0m\x1b[1m: mismatched types\x1b[0m";
        assert_eq!(strip_ansi(s), "error[E0308]: mismatched types");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\x1b]8;;http://x\x07link\x1b]8;;\x07"), "link");
    }
}

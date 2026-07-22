//! Headless web client for `idealyst dev`.
//!
//! A web app's Robot support is dial-out: the wasm bundle connects to the
//! dev session's `robot-relay` **when a browser loads the page**. In a
//! display-less environment (container, CI, SSH box) nothing ever loads it,
//! so the app never registers and robot introspection (MCP `wait_for_app`,
//! `find_element`, …) times out — the app is served but effectively invisible.
//! This module closes that gap: dev launches a headless Chromium-family
//! browser at the served URL, giving the robot loop a live client with no
//! display anywhere.
//!
//! Browser discovery mirrors the arena harness (`ARENA_CHROME` there):
//! `IDEALYST_BROWSER` env wins outright — and fails loudly when the path is
//! wrong rather than silently running a different browser — else fixed
//! install locations, else a Playwright-managed Chromium.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};

/// Fixed install locations probed (in order) when `IDEALYST_BROWSER` is
/// unset. Any Chromium-family binary works — only `--headless=new` is needed.
const BROWSER_CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/brave",
];

/// Decide whether dev should auto-launch the headless client.
///
/// Pure so the policy is unit-testable: an explicit flag always wins; the
/// auto case requires a robot relay to dial (otherwise the client would just
/// burn RAM) and no display to open a real browser on.
pub fn should_launch(force_on: bool, force_off: bool, relay_active: bool, has_display: bool) -> bool {
    if force_off {
        return false;
    }
    if force_on {
        return true;
    }
    relay_active && !has_display
}

/// Is a graphical display available? On macOS/Windows the answer is always
/// yes (a logged-in session has one); on unix it's the X11/Wayland env vars.
pub fn has_display() -> bool {
    if cfg!(target_os = "macos") || cfg!(windows) {
        return true;
    }
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Launch a headless browser at `url`, using `profile_dir` for the throwaway
/// user-data dir. The returned [`Child`] must be pushed into dev's `children`
/// list so Ctrl-C / session teardown kills it.
pub fn launch(url: &str, profile_dir: &Path) -> Result<Child> {
    let browser = resolve_browser(std::env::var("IDEALYST_BROWSER").ok(), BROWSER_CANDIDATES)
        .context(
            "no headless browser found — install chromium (or google-chrome/brave), \
             or point IDEALYST_BROWSER at one",
        )?;
    let mut command = Command::new(&browser);
    // Chromium reserves huge virtual address regions (V8 pointer cage); under
    // the CLI's inherited RLIMIT_AS it dies instantly and silently. Lift the
    // soft cap for this child (hard limit permits it — see memory_limit).
    crate::memory_limit::unlimit_child(&mut command);
    let child = command
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            // Docker's default /dev/shm is 64MB; without this flag chromium's
            // renderer dies there SILENTLY (no console output, empty DOM) the
            // moment a multi-MB wasm app initializes — the page loads, the
            // app never mounts, the robot relay never sees a dial-in. Routes
            // shared memory to /tmp instead. (Root-caused live 2026-07-20.)
            "--disable-dev-shm-usage",
            "--no-first-run",
            "--no-default-browser-check",
            "--remote-debugging-port=0",
        ])
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawning headless browser {browser}"))?;
    println!("[dev] headless web client: {browser} → {url}");
    Ok(child)
}

/// `env` (IDEALYST_BROWSER) wins outright — even if the path doesn't exist we
/// return None rather than silently falling back, so a typo'd override fails
/// loudly instead of running a different browser than the user asked for.
fn resolve_browser(env: Option<String>, candidates: &[&str]) -> Option<String> {
    if let Some(p) = env {
        return Path::new(&p).exists().then_some(p);
    }
    if let Some(found) = candidates.iter().find(|p| Path::new(p).exists()) {
        return Some(found.to_string());
    }
    playwright_chromium()
}

/// Last resort: a Playwright-managed Chromium under `~/.cache/ms-playwright/`.
/// Newest revision wins (numeric suffix ⇒ lexicographic order matches recency
/// for same-width revisions).
fn playwright_chromium() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let root = Path::new(&home).join(".cache/ms-playwright");
    let mut hits: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&root).ok()?.flatten() {
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let bin = if dir_name.starts_with("chromium_headless_shell-") {
            entry.path().join("chrome-linux/headless_shell")
        } else if dir_name.starts_with("chromium-") {
            entry.path().join("chrome-linux/chrome")
        } else {
            continue;
        };
        if bin.exists() {
            hits.push((dir_name, bin));
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().next().map(|(_, p)| p.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("idealyst-hc-test-{name}-{}", std::process::id()));
        std::fs::write(&p, "").unwrap();
        p
    }

    #[test]
    fn regression_headless_auto_fires_only_with_relay_and_no_display() {
        // The bug this feature fixes: in a display-less container the web app
        // was served but never loaded, so the robot bridge never registered
        // and MCP wait_for_app timed out (arena run-1, 2026-07-20).
        assert!(should_launch(false, false, true, false), "container + relay → auto");
        assert!(!should_launch(false, false, true, true), "desktop with display → no auto");
        assert!(!should_launch(false, false, false, false), "no relay → nothing to dial, no auto");
        assert!(should_launch(true, false, false, true), "--headless-client forces");
        assert!(!should_launch(true, true, true, false), "--no-headless-client wins over force-on");
    }

    #[test]
    fn env_override_beats_candidates_and_bad_env_fails_loudly() {
        let envp = tmp_file("env");
        let cand = tmp_file("cand");
        let cand_s = cand.to_string_lossy().to_string();
        let got = resolve_browser(Some(envp.to_string_lossy().to_string()), &[cand_s.as_str()]);
        assert_eq!(got.as_deref(), Some(envp.to_string_lossy().as_ref()));
        assert_eq!(
            resolve_browser(Some("/nonexistent/browser".into()), &[cand_s.as_str()]),
            None,
            "typo'd IDEALYST_BROWSER must not silently fall back"
        );
        std::fs::remove_file(envp).ok();
        std::fs::remove_file(cand).ok();
    }

    #[test]
    fn first_existing_candidate_wins() {
        let cand = tmp_file("cand2");
        let cand_s = cand.to_string_lossy().to_string();
        let got = resolve_browser(None, &["/nonexistent/one", cand_s.as_str()]);
        assert_eq!(got, Some(cand_s.clone()));
        std::fs::remove_file(cand).ok();
    }
}

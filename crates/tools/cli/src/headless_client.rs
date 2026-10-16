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

/// The files Chromium uses to claim a profile for one running instance.
/// `SingletonLock` is a symlink naming `<hostname>-<pid>`.
const SINGLETON_FILES: &[&str] = &["SingletonLock", "SingletonSocket", "SingletonCookie"];

/// How long after launch an exit counts as "the browser never started"
/// and is reported, rather than left to the log file.
const EARLY_EXIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Remove the profile's singleton claim, returning the names removed.
///
/// The CLI owns this profile: it is per-port, and a second session on the
/// same port cannot bind it, so no live browser of ours can hold it while
/// we launch. A claim left behind names a browser that is gone — killed
/// with its session, or on another HOST: when a project volume moves to a
/// new machine (a reclaimed cloud box), the lock names the dead host and
/// Chromium refuses the profile and exits at once ("The profile appears
/// to be in use by another Chromium process … on another computer"), so
/// the page never loads and nothing connects.
pub fn clear_stale_singleton(profile_dir: &Path) -> Vec<&'static str> {
    SINGLETON_FILES
        .iter()
        .copied()
        // `symlink_metadata`: the lock is a symlink to a name that is not
        // a file, so `exists()` reports it missing.
        .filter(|name| {
            let path = profile_dir.join(name);
            path.symlink_metadata().is_ok() && std::fs::remove_file(&path).is_ok()
        })
        .collect()
}

/// Copy the browser's stderr into `log`, and when the browser exits
/// within [`EARLY_EXIT`] of `started`, return its last lines: a browser
/// that dies on launch says why only there.
fn watch_stderr(
    stderr: impl std::io::Read,
    log: &Path,
    started: std::time::Instant,
) -> Option<String> {
    use std::io::{BufRead as _, Write as _};
    let mut file = std::fs::File::create(log).ok();
    let mut tail: std::collections::VecDeque<String> = Default::default();
    for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
        if let Some(f) = file.as_mut() {
            let _ = writeln!(f, "{line}");
        }
        if tail.len() == 8 {
            tail.pop_front();
        }
        tail.push_back(line);
    }
    // EOF: the browser exited (or closed stderr, which it does not).
    (started.elapsed() < EARLY_EXIT).then(|| {
        let lines: Vec<String> = tail.into_iter().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            "it printed nothing".to_string()
        } else {
            lines.join("\n")
        }
    })
}

/// Launch a headless browser at `url`, using `profile_dir` for the throwaway
/// user-data dir. The returned [`Child`] must be pushed into dev's `children`
/// list so Ctrl-C / session teardown kills it.
///
/// The browser's stderr goes to `<profile_dir>.log`; if it exits within
/// [`EARLY_EXIT`], its last lines are logged to the session as well.
pub fn launch(url: &str, profile_dir: &Path) -> Result<Child> {
    let browser = resolve_browser(std::env::var("IDEALYST_BROWSER").ok(), BROWSER_CANDIDATES)
        .context(
            "no headless browser found — install chromium (or google-chrome/brave), \
             or point IDEALYST_BROWSER at one",
        )?;
    let cleared = clear_stale_singleton(profile_dir);
    if !cleared.is_empty() {
        crate::dlog!("dev", "headless web client: cleared a stale profile lock ({})", cleared.join(", "));
    }
    let mut command = Command::new(&browser);
    // Chromium reserves huge virtual address regions (V8 pointer cage); under
    // the CLI's inherited RLIMIT_AS it dies instantly and silently. Lift the
    // soft cap for this child (hard limit permits it — see memory_limit).
    crate::memory_limit::unlimit_child(&mut command);
    let mut child = command
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            // Docker's default /dev/shm is 64MB; without this flag chromium's
            // renderer dies there SILENTLY (no console output, empty DOM) the
            // moment a multi-MB wasm app initializes — the page loads, the
            // app never mounts, the robot relay never sees a dial-in. Routes
            // shared memory to /tmp instead.
            "--disable-dev-shm-usage",
            "--no-first-run",
            "--no-default-browser-check",
            "--remote-debugging-port=0",
        ])
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning headless browser {browser}"))?;
    println!("[dev] headless web client: {browser} → {url}");
    if let Some(stderr) = child.stderr.take() {
        let log = log_path(profile_dir);
        let started = std::time::Instant::now();
        std::thread::spawn(move || {
            if let Some(tail) = watch_stderr(stderr, &log, started) {
                crate::dlog!(
                    "dev",
                    "headless web client exited {} s after launch, so the page will not \
                     connect. Its last output ({}):\n{tail}",
                    started.elapsed().as_secs(),
                    log.display(),
                );
            }
        });
    }
    Ok(child)
}

/// Where the browser's stderr goes: beside its profile directory.
fn log_path(profile_dir: &Path) -> std::path::PathBuf {
    let mut name = profile_dir.file_name().unwrap_or_default().to_os_string();
    name.push(".log");
    profile_dir.with_file_name(name)
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

    /// Regression (CrewForge): a project volume reattached to a new host
    /// kept the profile's `SingletonLock -> <dead host>-<pid>`, Chromium
    /// refused the profile and exited, and the session never said why.
    #[cfg(unix)]
    #[test]
    fn regression_a_lock_naming_another_host_is_cleared_before_launch() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("headless-client-profile-3200");
        std::fs::create_dir_all(&profile).unwrap();
        std::os::unix::fs::symlink("ip-10-20-2-90-1729924", profile.join("SingletonLock")).unwrap();
        std::os::unix::fs::symlink("/tmp/gone/SingletonSocket", profile.join("SingletonSocket")).unwrap();
        std::fs::write(profile.join("Preferences"), "{}").unwrap();

        assert_eq!(clear_stale_singleton(&profile), vec!["SingletonLock", "SingletonSocket"]);
        assert!(profile.join("SingletonLock").symlink_metadata().is_err());
        assert!(profile.join("Preferences").is_file(), "the rest of the profile stays");
        assert!(clear_stale_singleton(&profile).is_empty());
    }

    /// A browser that dies on launch is reported with its own words, and
    /// everything it printed is kept in the log file.
    #[test]
    fn an_early_exit_reports_the_browsers_last_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("client.log");
        let mut child = Command::new("sh")
            .args(["-c", "echo starting >&2; echo 'The profile appears to be in use' >&2; exit 21"])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let report = watch_stderr(child.stderr.take().unwrap(), &log, std::time::Instant::now());
        let _ = child.wait();
        let report = report.expect("an exit right after launch is reported");
        assert!(report.contains("The profile appears to be in use"), "{report}");
        assert!(std::fs::read_to_string(&log).unwrap().contains("starting"));

        // Long after launch (a teardown, a later crash): the log file only.
        let mut child = Command::new("sh").args(["-c", "echo bye >&2"]).stderr(Stdio::piped()).spawn().unwrap();
        let old = std::time::Instant::now() - EARLY_EXIT - std::time::Duration::from_secs(1);
        assert!(watch_stderr(child.stderr.take().unwrap(), &log, old).is_none());
        let _ = child.wait();
    }

    #[test]
    fn the_log_sits_beside_the_profile() {
        assert_eq!(
            log_path(Path::new("/p/.idealyst/headless-client-profile-3200")),
            Path::new("/p/.idealyst/headless-client-profile-3200.log")
        );
    }

    fn tmp_file(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("idealyst-hc-test-{name}-{}", std::process::id()));
        std::fs::write(&p, "").unwrap();
        p
    }

    #[test]
    fn regression_headless_auto_fires_only_with_relay_and_no_display() {
        // The bug this feature fixes: in a display-less container the web app
        // was served but never loaded, so the robot bridge never registered
        // and MCP wait_for_app timed out.
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

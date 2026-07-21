//! Arena consumption of robot-on-web.
//!
//! The `robot` verifier tier introspects the running app through its Robot
//! bridge. On web that bridge can't be hosted in the browser, so this module
//! stands up a `robot-relay` and a **headless browser** that loads the served
//! bundle — the wasm app dials the relay on boot, and the relay exposes the
//! ordinary TCP bridge that [`crate::verify::robot`] already discovers via
//! `~/.idealyst/apps`. No changes to the verifier: from its side a relayed web
//! app looks exactly like a native one.
//!
//! Flow per run (in [`crate::harness::run`]):
//!   1. build the bundle with `--robot`,
//!   2. [`start_relay`] (registers + injects the URL) — BEFORE serving,
//!   3. serve the bundle,
//!   4. [`host`] launches the browser and waits for the app to dial in,
//!   5. robot-tier items query the relay; the host stays up until the run ends.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Fixed install locations probed (in order) when `ARENA_CHROME` is unset.
/// Any Chromium-family binary works — the host only needs `--headless=new`.
const BROWSER_CANDIDATES: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/brave",
];

/// Holds the relay + headless browser alive for the duration of robot-tier
/// verification. Kills the browser on drop; the relay's own `Drop` removes its
/// `~/.idealyst/apps` registration.
pub struct RobotWebHost {
    _relay: robot_relay::RelayHandle,
    browser: Child,
}

impl Drop for RobotWebHost {
    fn drop(&mut self) {
        let _ = self.browser.kill();
        let _ = self.browser.wait();
    }
}

fn chrome_path() -> Option<String> {
    resolve_browser(std::env::var("ARENA_CHROME").ok(), BROWSER_CANDIDATES)
}

/// `env` (ARENA_CHROME) wins outright — even if the path doesn't exist we
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

/// Last resort: a Playwright-managed Chromium under `~/.cache/ms-playwright/`
/// (`chromium-<rev>/chrome-linux/chrome` or `chromium_headless_shell-<rev>/
/// chrome-linux/headless_shell`). Newest revision wins.
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
    // Sort by directory name descending — the numeric revision suffix makes
    // lexicographic order match recency for same-width revisions.
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter()
        .next()
        .map(|(_, p)| p.to_string_lossy().to_string())
}

/// Start a relay registered for `project_dir` (so `verify::robot` discovers it
/// by matching `project_root`) and inject its URL into the served bundle's
/// `index.html`. Call this BEFORE serving the bundle.
pub fn start_relay(project_dir: &Path, dist: &Path) -> anyhow::Result<robot_relay::RelayHandle> {
    let project_root =
        std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let name = project_root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("app")
        .to_string();
    let relay = robot_relay::start(robot_relay::RelayConfig {
        ws_port: 0,
        tcp_port: 0,
        register: true,
        identity: Some(robot_relay::Identity {
            name,
            bundle_id: None,
            project_root: Some(project_root.to_string_lossy().to_string()),
        }),
        screenshot_dir: None,
    })?;

    let index = dist.join("index.html");
    let html = std::fs::read_to_string(&index)?;
    let snippet = format!(
        "<script>window.IDEALYST_ROBOT_RELAY_URL=\"ws://127.0.0.1:{}\";</script>\n",
        relay.ws_addr.port()
    );
    let out = match html.find("</head>") {
        Some(i) => format!("{}{}{}", &html[..i], snippet, &html[i..]),
        None => format!("{snippet}{html}"),
    };
    std::fs::write(&index, out)?;
    Ok(relay)
}

/// Launch a headless browser at `base_url` (the served bundle) so the app dials
/// the relay, and block until it connects. Returns a host that tears the
/// browser down on drop.
pub fn host(
    relay: robot_relay::RelayHandle,
    base_url: &str,
    run_dir: &Path,
) -> anyhow::Result<RobotWebHost> {
    let chrome =
        chrome_path().ok_or_else(|| anyhow::anyhow!("no headless browser (set ARENA_CHROME)"))?;
    let tcp = relay.tcp_addr;
    let profile = run_dir.join("chrome-profile");
    let browser = Command::new(&chrome)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            // Docker's default /dev/shm (64MB) silently kills chromium's
            // renderer under multi-MB wasm init — no console error, empty
            // DOM, no relay dial-in. Required for in-container runs.
            "--disable-dev-shm-usage",
            "--no-first-run",
            "--no-default-browser-check",
            "--remote-debugging-port=0",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(base_url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let host = RobotWebHost {
        _relay: relay,
        browser,
    };
    wait_for_app(tcp)?;
    Ok(host)
}

/// Poll the relay's TCP bridge with `ping` until the browser app has dialed in
/// (the relay forwards `ping` to the app, so a `pong` means it's connected).
fn wait_for_app(tcp: SocketAddr) -> anyhow::Result<()> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        if ping_ok(tcp) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("browser app never connected to the relay within {CONNECT_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_browser;

    fn tmp_file(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("arena-browser-test-{name}-{}", std::process::id()));
        std::fs::write(&p, "").unwrap();
        p
    }

    #[test]
    fn regression_env_override_beats_candidates() {
        // The bug: DEFAULT_CHROME hardcoded the macOS path, so on Linux the
        // host errored even with browsers installed. Now candidates probe, but
        // an explicit ARENA_CHROME must still win over any candidate.
        let envp = tmp_file("env");
        let cand = tmp_file("cand");
        let cand_s = cand.to_string_lossy().to_string();
        let got = resolve_browser(
            Some(envp.to_string_lossy().to_string()),
            &[cand_s.as_str()],
        );
        assert_eq!(got.as_deref(), Some(envp.to_string_lossy().as_ref()));
        std::fs::remove_file(envp).ok();
        std::fs::remove_file(cand).ok();
    }

    #[test]
    fn regression_bad_env_override_fails_loudly_not_fallback() {
        // A typo'd ARENA_CHROME must yield None (loud error upstream), not
        // silently run a different browser than the user asked for.
        let cand = tmp_file("cand2");
        let cand_s = cand.to_string_lossy().to_string();
        let got = resolve_browser(Some("/nonexistent/browser".into()), &[cand_s.as_str()]);
        assert_eq!(got, None);
        std::fs::remove_file(cand).ok();
    }

    #[test]
    fn first_existing_candidate_wins() {
        let cand = tmp_file("cand3");
        let cand_s = cand.to_string_lossy().to_string();
        let got = resolve_browser(None, &["/nonexistent/one", cand_s.as_str()]);
        assert_eq!(got, Some(cand_s.clone()));
        std::fs::remove_file(cand).ok();
    }
}

fn ping_ok(tcp: SocketAddr) -> bool {
    let Ok(stream) = TcpStream::connect(tcp) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(4)));
    let Ok(mut writer) = stream.try_clone() else {
        return false;
    };
    if writer
        .write_all(b"{\"id\":1,\"cmd\":\"ping\",\"args\":{}}\n")
        .is_err()
    {
        return false;
    }
    let _ = writer.flush();
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line).is_err() {
        return false;
    }
    line.contains("\"ok\"") && line.contains("pong")
}

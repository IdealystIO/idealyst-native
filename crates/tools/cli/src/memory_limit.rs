//! Process-level memory cap for the CLI.
//!
//! Long-running CLI modes (`idealyst mcp`, `idealyst dev`,
//! `idealyst serve`) are stdio-attached children of an editor /
//! agent host. If one of them leaks — and there's a known suspect
//! around the runtime-server + MCP catalog paths — the parent host
//! buffers stdio and OOMs along with the child. A hard cap on the
//! CLI's memory turns a silent multi-GB drift into a loud,
//! debuggable abort.
//!
//! ## Enforcement strategy
//!
//! A background poll thread reads this process's RSS every few
//! seconds and `process::abort()`s when it crosses the cap. Polling
//! latency is fine for our purpose — runaway leaks grow at MB/s,
//! not GB/ms, and the goal is catching multi-GB drift before the
//! editor host buckles.
//!
//! - **Linux**: RSS from `/proc/self/statm` (resident pages ×
//!   page size).
//! - **macOS**: RSS via `proc_pidinfo(PROC_PIDTASKINFO)`. macOS
//!   cannot enforce `setrlimit` for memory at all (any
//!   `setrlimit(RLIMIT_AS|RLIMIT_DATA, ...)` returns `EINVAL` on
//!   darwin even though the constants are defined) — verified by
//!   direct experiment, the symbols accept the call but the kernel
//!   doesn't constrain the process.
//! - **Other**: silent no-op.
//!
//! **The cap must never reach a child process.** It bounds THIS
//! process only; `cargo` / `rustc` / `wasm-bindgen` / `wasm-opt`
//! run unconstrained. That is the whole contract of this module —
//! it does NOT constrain compilation memory.
//!
//! ### Why not `setrlimit(RLIMIT_AS)` on Linux
//!
//! It was the original Linux strategy, and it silently broke that
//! contract: `RLIMIT_AS` is **inherited across fork/exec**, so every
//! build subprocess the CLI spawns ran under the CLI's 4 GB cap.
//! `idealyst dev --web` on this repo's website died with
//! `memory allocation of 160 bytes failed` + SIGABRT — wasm-bindgen
//! needs ~6.4 GB of address space for the website's 419 MB debug
//! wasm, hit the inherited 4 GB ceiling, and aborted on the next
//! (tiny) allocation. The same latent failure sat behind all ~30
//! `Command::new("cargo")` sites across the build tools.
//!
//! Root-caused 2026-07-23. The fix is deliberately here rather than
//! at the call sites: sprinkling [`unlimit_child`] over 30 spawns
//! leaves every *future* spawn re-broken, and it kept Linux
//! observably different from macOS (where the cap never touched
//! children because the poll thread isn't inherited). One mechanism,
//! same observable behavior on both platforms.
//!
//! Trade-off accepted: RSS polling loses `RLIMIT_AS`'s exact
//! abort-at-the-allocation-site precision, and measures resident
//! rather than reserved memory. Both are fine here — RSS is the
//! better proxy for "will this OOM the host" anyway (address-space
//! reservations aren't what crater the machine), and macOS has run
//! on exactly these semantics since the cap was introduced.
//!
//! ## Override
//!
//! [`ENV_OVERRIDE`] — integer megabytes. `0` disables the cap (for
//! debugging the leak with a memory profiler that needs unbounded
//! growth).

/// Default cap. 4 GB is ~80× the steady-state RSS of an idle MCP
/// server and ~20× a typical `dev` orchestrator, so a leak still
/// trips the cap well before it can crater the host — while leaving
/// headroom for the CLI's legitimately heavy in-process work:
/// `wasm-split` on a debug-build wasm parses + rewrites the whole
/// module in memory and peaks past 2 GB on large apps (the website's
/// ~42 MB post-bindgen debug wasm aborted at the old 2 GB cap).
pub const DEFAULT_LIMIT_MB: u64 = 4096;

/// Env var name for override. `0` disables.
pub const ENV_OVERRIDE: &str = "IDEALYST_MEMORY_LIMIT_MB";

/// RSS poll cadence. Long enough that overhead is invisible, short
/// enough that we abort well before a leak that's growing at MB/s
/// exhausts the host.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

/// Apply the cap. Silent on default activation so short-lived
/// commands don't gain a startup banner; logs only when the user
/// has explicitly overridden the default (so they get confirmation
/// their override took effect).
pub fn apply(default_mb: u64) {
    let user_override = std::env::var(ENV_OVERRIDE)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    let mb = user_override.unwrap_or(default_mb);
    if mb == 0 {
        return;
    }
    let bytes = mb.saturating_mul(1024 * 1024);
    let log = user_override.is_some();

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    spawn_rss_monitor(bytes, mb, log);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (bytes, mb, log);
    }
}

/// Poll this process's RSS and abort if it crosses the cap.
///
/// Deliberately NOT `setrlimit` — see the module docs: an rlimit is
/// inherited by every build subprocess and would cap compilation.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn spawn_rss_monitor(limit_bytes: u64, mb: u64, log: bool) {
    if log {
        eprintln!(
            "[idealyst] memory cap: {mb} MB RSS via poll thread (via {ENV_OVERRIDE}; \
             0 disables)",
        );
    }
    let _ = std::thread::Builder::new()
        .name("idealyst-mem-monitor".to_string())
        .spawn(move || loop {
            std::thread::sleep(POLL_INTERVAL);
            if let Some(rss) = current_rss_bytes() {
                if rss > limit_bytes {
                    eprintln!(
                        "[idealyst] memory cap exceeded: RSS {} MB > cap {mb} MB; \
                         aborting to prevent host OOM. Override via {ENV_OVERRIDE}.",
                        rss / (1024 * 1024),
                    );
                    std::process::abort();
                }
            }
        });
}

/// Current resident set size in bytes, or `None` if it can't be read.
///
/// Linux: `/proc/self/statm` field 2 is the resident page count
/// (`statm` is the cheap one — `/proc/self/status` formats a dozen
/// fields we don't need on every poll).
#[cfg(target_os = "linux")]
fn current_rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf is a thread-safe read of a static system value.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return None;
    }
    Some(resident_pages.saturating_mul(page_size as u64))
}

#[cfg(target_os = "macos")]
fn current_rss_bytes() -> Option<u64> {
    let pid = std::process::id() as libc::c_int;
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: proc_pidinfo writes into `info`; we pass the correct
    // size. Return value is the number of bytes written, or -1 on
    // error; we only trust `info` when the call wrote exactly the
    // expected size (any partial write means the layout drifted
    // and the fields are not reliable).
    let ret = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if ret == size {
        Some(info.pti_resident_size)
    } else {
        None
    }
}

/// Lift the address-space cap for a child that legitimately reserves huge
/// address space. Headless Chromium is the motivating case: V8 reserves
/// multi-GB virtual regions (pointer-compression cage) at startup, so under
/// an inherited `RLIMIT_AS` it dies INSTANTLY and silently — dev's headless
/// web client spawned and vanished with no output (root-caused live
/// 2026-07-20). The child raises its soft cap back to the hard limit between
/// fork and exec. No-op off Linux.
///
/// Since [`apply`] no longer sets `RLIMIT_AS` (see module docs), this is now
/// belt-and-braces: it only matters when something *outside* the CLI — the
/// user's shell, a CI runner, a systemd unit — imposed a soft cap that a
/// memory-hungry child can't live within.
pub fn unlimit_child(cmd: &mut std::process::Command) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: pre_exec runs after fork, before exec — only
        // async-signal-safe calls are allowed; getrlimit/setrlimit qualify.
        unsafe {
            cmd.pre_exec(|| {
                let mut cur: libc::rlimit = std::mem::zeroed();
                if libc::getrlimit(libc::RLIMIT_AS, &mut cur) == 0 {
                    cur.rlim_cur = cur.rlim_max;
                    let _ = libc::setrlimit(libc::RLIMIT_AS, &cur);
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cmd;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RLIMIT_AS` is process-global state and cargo runs tests in
    /// parallel threads, so the two rlimit tests below MUST NOT overlap:
    /// `regression_unlimit_child_lifts_inherited_rlimit_as` temporarily
    /// lowers the soft cap, which lands inside the other test's
    /// before/after window and fails it spuriously. Observed flaking
    /// live — serialize them.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    static RLIMIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take [`RLIMIT_LOCK`], ignoring poisoning — a panicking rlimit test
    /// restores the limit before asserting, so the next test is still
    /// safe to run and shouldn't cascade into a second failure.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn rlimit_guard() -> std::sync::MutexGuard<'static, ()> {
        RLIMIT_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    // Sanity-check the macOS RSS binding. This is the load-bearing
    // platform syscall — if `proc_pidinfo` ever stops returning
    // PROC_PIDTASKINFO, the monitor thread silently never aborts
    // and the safety net disappears. A trivial round-trip catches
    // both the libc surface drift and any layout mismatch in
    // `proc_taskinfo` (the call returns -1 / wrong size on
    // mismatch and we'd see `None` here).
    // Regression for the silent headless-client death: a child spawned with
    // `unlimit_child` must see soft RLIMIT_AS == hard limit even when the
    // parent (this test) has a lowered soft cap. Uses `sh -c 'ulimit -v'`
    // (reports KB, or "unlimited") as the observer; restores the parent's
    // limit afterwards. 8 GB is far above the test runner's needs, so the
    // temporary cap can't disturb sibling tests.
    #[cfg(target_os = "linux")]
    #[test]
    fn regression_unlimit_child_lifts_inherited_rlimit_as() {
        let _serialized = rlimit_guard();
        unsafe {
            let mut orig: libc::rlimit = std::mem::zeroed();
            assert_eq!(libc::getrlimit(libc::RLIMIT_AS, &mut orig), 0);
            let capped = libc::rlimit {
                rlim_cur: 8 * 1024 * 1024 * 1024,
                rlim_max: orig.rlim_max,
            };
            assert_eq!(libc::setrlimit(libc::RLIMIT_AS, &capped), 0);

            let plain = std::process::Command::new("sh")
                .args(["-c", "ulimit -v"])
                .output()
                .unwrap();
            let mut lifted_cmd = std::process::Command::new("sh");
            lifted_cmd.args(["-c", "ulimit -v"]);
            unlimit_child(&mut lifted_cmd);
            let lifted = lifted_cmd.output().unwrap();

            // Restore before asserting so a failure can't leak the cap.
            let _ = libc::setrlimit(libc::RLIMIT_AS, &orig);

            let plain_out = String::from_utf8_lossy(&plain.stdout).trim().to_string();
            let lifted_out = String::from_utf8_lossy(&lifted.stdout).trim().to_string();
            assert_eq!(plain_out, "8388608", "child inherits the capped soft limit (KB)");
            assert_ne!(lifted_out, plain_out, "unlimit_child must lift the cap");
        }
    }

    // The bug this module was re-engineered for: `apply` used to
    // `setrlimit(RLIMIT_AS, 4GB)`, which fork/exec inherits, so every build
    // subprocess ran under the CLI's leak cap. `idealyst dev --web` aborted
    // in wasm-bindgen ("memory allocation of 160 bytes failed", SIGABRT) —
    // it needs ~6.4 GB for the website's 419 MB debug wasm. Asserts the cap
    // never touches the rlimit that children inherit.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn regression_cap_does_not_lower_rlimit_as_inherited_by_build_children() {
        let _serialized = rlimit_guard();
        unsafe {
            let mut before: libc::rlimit = std::mem::zeroed();
            assert_eq!(libc::getrlimit(libc::RLIMIT_AS, &mut before), 0);

            // A cap so high the monitor thread can never trip during tests,
            // while still exercising the real `apply` path.
            apply(1_000_000);

            let mut after: libc::rlimit = std::mem::zeroed();
            assert_eq!(libc::getrlimit(libc::RLIMIT_AS, &mut after), 0);
            assert_eq!(
                before.rlim_cur, after.rlim_cur,
                "apply() must not lower the soft RLIMIT_AS — build children inherit it \
                 and cargo/rustc/wasm-bindgen would die under the CLI's leak cap",
            );
            assert_eq!(
                before.rlim_max, after.rlim_max,
                "apply() must not touch the hard RLIMIT_AS",
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_rss_returns_plausible_value() {
        let rss = current_rss_bytes().expect("/proc/self/statm should be readable on Linux");
        // A test process is at least a few MB and well under 100GB.
        assert!(rss > 1024 * 1024, "RSS {rss} bytes too small");
        assert!(
            rss < 100 * 1024 * 1024 * 1024,
            "RSS {rss} bytes implausibly large",
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_rss_returns_plausible_value() {
        let rss = current_rss_bytes()
            .expect("proc_pidinfo(PROC_PIDTASKINFO) returned an unexpected size");
        // A test process is at least a few MB and well under 100GB.
        assert!(rss > 1024 * 1024, "RSS {rss} bytes too small");
        assert!(
            rss < 100 * 1024 * 1024 * 1024,
            "RSS {rss} bytes implausibly large",
        );
    }
}

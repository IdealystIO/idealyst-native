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
//! Differs by platform because macOS doesn't enforce `setrlimit`
//! for memory at all (any `setrlimit(RLIMIT_AS|RLIMIT_DATA, ...)`
//! returns `EINVAL` on darwin even though the constants are
//! defined). Verified by direct experiment — the symbols accept
//! the call but the kernel doesn't constrain the process.
//!
//! - **Linux**: `setrlimit(RLIMIT_AS, …)`. Kernel-enforced; any
//!   `mmap` / `sbrk` over the limit returns `ENOMEM` and the Rust
//!   allocator panics at the call site. No background overhead.
//! - **macOS**: a background poll thread reads RSS via
//!   `proc_pidinfo(PROC_PIDTASKINFO)` every few seconds and
//!   `process::abort()`s when RSS crosses the cap. Polling
//!   latency is fine for our purpose — runaway leaks grow at
//!   MB/s, not GB/ms, and the goal is catching multi-GB drift
//!   before the editor host buckles.
//! - **Other**: silent no-op.
//!
//! Children inherit `setrlimit` (Linux) but not the monitor thread
//! (macOS — it's per-process). Either way, the cap is per-process,
//! so `cargo`/`rustc` children get their own budget, not a shared
//! one. This does NOT constrain compilation memory.
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

/// macOS RSS poll cadence. Long enough that overhead is invisible,
/// short enough that we abort well before a leak that's growing at
/// MB/s exhausts the host.
#[cfg(target_os = "macos")]
const MACOS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

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

    #[cfg(target_os = "linux")]
    apply_rlimit_as(bytes, mb, log);
    #[cfg(target_os = "macos")]
    spawn_macos_monitor(bytes, mb, log);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (bytes, mb, log);
    }
}

#[cfg(target_os = "linux")]
fn apply_rlimit_as(bytes: u64, mb: u64, log: bool) {
    // SAFETY: getrlimit/setrlimit are thread-safe POD calls. We
    // preserve rlim_max so the hard limit stays where the parent
    // set it — lowering it would be permanent for this process
    // tree and impossible to raise back.
    unsafe {
        let mut current: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_AS, &mut current) != 0 {
            return;
        }
        let new = libc::rlimit {
            rlim_cur: bytes as libc::rlim_t,
            rlim_max: current.rlim_max,
        };
        if libc::setrlimit(libc::RLIMIT_AS, &new) == 0 && log {
            eprintln!(
                "[idealyst] memory cap: {mb} MB address space (via {ENV_OVERRIDE}; \
                 0 disables)",
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn spawn_macos_monitor(limit_bytes: u64, mb: u64, log: bool) {
    if log {
        eprintln!(
            "[idealyst] memory cap: {mb} MB RSS via poll thread (via {ENV_OVERRIDE}; \
             0 disables)",
        );
    }
    let _ = std::thread::Builder::new()
        .name("idealyst-mem-monitor".to_string())
        .spawn(move || loop {
            std::thread::sleep(MACOS_POLL_INTERVAL);
            if let Some(rss) = macos_current_rss_bytes() {
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

#[cfg(target_os = "macos")]
fn macos_current_rss_bytes() -> Option<u64> {
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
/// the CLI's inherited `RLIMIT_AS` it dies INSTANTLY and silently — dev's
/// headless web client spawned and vanished with no output (root-caused live
/// 2026-07-20). [`apply`] preserves `rlim_max`, so the child may raise its
/// soft cap back to the hard limit between fork and exec. No-op off Linux.
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_rss_returns_plausible_value() {
        let rss = macos_current_rss_bytes()
            .expect("proc_pidinfo(PROC_PIDTASKINFO) returned an unexpected size");
        // A test process is at least a few MB and well under 100GB.
        assert!(rss > 1024 * 1024, "RSS {rss} bytes too small");
        assert!(
            rss < 100 * 1024 * 1024 * 1024,
            "RSS {rss} bytes implausibly large",
        );
    }
}

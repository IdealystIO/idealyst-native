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

/// Default cap. 2 GB is ~40× the steady-state RSS of an idle MCP
/// server and ~10× a typical `dev` orchestrator, so a leak trips
/// the cap well before it can crater the host.
pub const DEFAULT_LIMIT_MB: u64 = 2048;

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

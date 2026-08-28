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
//! ## Aborting must not leave the session's servers behind
//!
//! `process::abort()` raises SIGABRT: no unwinding, no `Drop`, and none
//! of `dev`'s own teardown. Every child the session spawned — the
//! standalone server, the runtime-server host, the jobs worker — is
//! simply re-parented to init and keeps running, still holding its
//! port. The cap then reads as "the dev server randomly stopped
//! rebuilding", and the next session on that port fails to bind for a
//! reason nothing on screen explains.
//!
//! So the monitor kills what it knows about first. `dev` registers its
//! shared child vec through [`watch_children`] — one call, at the one
//! place the vec is created, rather than a `register` beside all ~8
//! `push` sites, so a child added later is covered without anyone
//! remembering this module exists. The full-stack web launcher owns its
//! server privately (it kills and respawns it on every rebuild, so the
//! handle cannot live in a shared vec) and registers the pid instead,
//! through [`watch_pid`] / [`unwatch_pid`].
//!
//! ### Killing the CHILD is not enough
//!
//! `dev` starts servers with `cargo run`, so the child it holds is
//! cargo and the server is cargo's child. Killing the handle leaves the
//! server running and re-parents it to init — which is precisely the
//! orphan this is meant to prevent, one generation down. Everything
//! here therefore kills the process TREE (see `kill_tree`).
//!
//! This is also why an orphaned dev server always shows PPID 1 and no
//! CLI anywhere: it never was the CLI's direct child.
//!
//! **This covers abort, not annihilation.** A SIGKILLed CLI, or one
//! whose parent shell dies, still orphans its children — nothing runs
//! in the process at that point. `PR_SET_PDEATHSIG` would cover those
//! too, but it fires on the death of the *thread* that forked, and
//! `dev` spawns its web server from a target worker thread; a thread
//! that returns while the session runs would take the server with it.
//! Not worth trading a rare orphan for a live session dying.
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

/// The child processes to take down before aborting, if a caller has
/// registered any. See [`watch_children`].
///
/// A `Weak`, so this static never keeps a dead session's `Child`
/// handles alive, and holding it costs nothing in the commands (`mcp`,
/// `serve`) that spawn nothing.
static WATCHED_CHILDREN: std::sync::Mutex<
    Option<std::sync::Weak<std::sync::Mutex<Vec<std::process::Child>>>>,
> = std::sync::Mutex::new(None);

/// Register the session's child processes, so tripping the cap kills
/// them instead of orphaning them.
///
/// Takes the same `Arc` the caller already keeps for its own Ctrl-C
/// teardown — one registration for the whole session, and anything
/// pushed into the vec afterwards is covered automatically.
pub fn watch_children(children: &std::sync::Arc<std::sync::Mutex<Vec<std::process::Child>>>) {
    if let Ok(mut slot) = WATCHED_CHILDREN.lock() {
        *slot = Some(std::sync::Arc::downgrade(children));
    }
}

/// Pids whose process TREE must not survive this process, registered by
/// an owner that cannot hand over its `Child` — see [`watch_pid`].
static WATCHED_PIDS: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// Register a process tree to take down on abort, by pid.
///
/// For an owner that keeps its `Child` to itself: the full-stack web
/// launcher kills and respawns its server on every rebuild, so the
/// handle cannot live in the shared vec [`watch_children`] takes.
/// Pair every call with [`unwatch_pid`] when that process is
/// deliberately replaced, so a stale pid can never be signalled after
/// the number is recycled.
pub fn watch_pid(pid: u32) {
    if let Ok(mut pids) = WATCHED_PIDS.lock() {
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
}

/// Drop a pid registered by [`watch_pid`] — the process is being
/// replaced, or has already been reaped by its owner.
pub fn unwatch_pid(pid: u32) {
    if let Ok(mut pids) = WATCHED_PIDS.lock() {
        pids.retain(|p| *p != pid);
    }
}

/// Every descendant of `root`, deepest first, `root` last.
///
/// Read from `ps` rather than `/proc` so Linux and macOS take the same
/// path — this runs once, on the way to an abort, where a fork costs
/// nothing and a platform ifdef costs a second untested code path.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn process_tree(root: u32) -> Vec<u32> {
    let mut edges: Vec<(u32, u32)> = Vec::new();
    if let Ok(out) = std::process::Command::new("ps")
        .args(["-eo", "pid=,ppid="])
        .output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split_whitespace();
            if let (Some(Ok(pid)), Some(Ok(ppid))) = (
                it.next().map(str::parse::<u32>),
                it.next().map(str::parse::<u32>),
            ) {
                edges.push((pid, ppid));
            }
        }
    }
    // Breadth-first from the root, then reversed: children die before
    // their parents, so a supervisor cannot restart one on the way out.
    let mut order = vec![root];
    let mut i = 0;
    while i < order.len() {
        let parent = order[i];
        for (pid, ppid) in &edges {
            if *ppid == parent && !order.contains(pid) {
                order.push(*pid);
            }
        }
        i += 1;
    }
    order.reverse();
    order
}

/// SIGKILL a process and everything below it. Returns true if the root
/// was still there to kill.
///
/// Public because the same need exists outside this module: `dev`
/// restarts its server by killing the `cargo run` it holds, and a plain
/// `Child::kill` there leaves the server itself running — holding the
/// port the restart is about to rebind.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
pub fn kill_tree(root: u32) -> bool {
    let mut root_killed = false;
    for pid in process_tree(root) {
        // SAFETY: kill(2) with a pid we read from the process table.
        let ok = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } == 0;
        if pid == root {
            root_killed = ok;
        }
    }
    root_killed
}

/// No process table to walk, and no `kill(2)`: callers fall back to
/// whatever they did before (`Child::kill`).
#[cfg(not(any(target_os = "linux", target_os = "macos", test)))]
pub fn kill_tree(_root: u32) -> bool {
    false
}

/// Kill every registered child and reap it. Returns how many were
/// killed — `0` when nothing is registered, which is the normal case
/// for every command but `dev`.
///
/// `try_lock` in a bounded retry rather than `lock`: this runs on the
/// monitor thread moments before the process dies, and blocking for
/// ever on a mutex some other thread is holding would strand the abort
/// that is the whole point of the cap. Half a second is far longer
/// than any hold in `dev` (a `push`, or one pass of the exit poll).
///
/// Each child is `wait`ed after the kill so it is not left as a zombie
/// on a host whose PID 1 does not reap — a container running a bare
/// `sleep infinity` accumulates one per abort otherwise.
///
/// Only the platforms that HAVE a monitor thread compile it; elsewhere
/// [`watch_children`] is a registration nothing ever reads, which is
/// the same no-op the cap itself is there.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn kill_watched_children() -> usize {
    let Some(weak) = WATCHED_CHILDREN.lock().ok().and_then(|slot| slot.clone()) else {
        return 0;
    };
    let Some(children) = weak.upgrade() else {
        return 0;
    };
    for _ in 0..10 {
        if let Ok(mut guard) = children.try_lock() {
            let mut killed = 0;
            for child in guard.iter_mut() {
                if kill_tree(child.id()) {
                    killed += 1;
                }
                // Reap the handle we own, so it is not left as a zombie
                // on a host whose PID 1 does not reap.
                let _ = child.wait();
            }
            return killed;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    0
}

/// [`kill_watched_children`] plus the pid-registered trees. The whole
/// of what the cap takes down with it.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn kill_watched_processes() -> usize {
    let mut killed = kill_watched_children();
    let pids = WATCHED_PIDS
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    for pid in pids {
        if kill_tree(pid) {
            killed += 1;
            // Reap it if it is ours — a pid registered this way is a
            // direct child of this process, so waitpid succeeds and the
            // zombie goes away.
            // SAFETY: WNOHANG never blocks; a foreign pid just fails.
            unsafe {
                libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), libc::WNOHANG);
            }
        }
    }
    killed
}

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
                    // Before the abort, not after: SIGABRT runs no
                    // teardown at all, so anything still alive here is
                    // an orphan holding a port.
                    let killed = kill_watched_processes();
                    let took = match killed {
                        0 => String::new(),
                        1 => " Took 1 child process with it.".to_string(),
                        n => format!(" Took {n} child processes with it."),
                    };
                    eprintln!(
                        "[idealyst] memory cap exceeded: RSS {} MB > cap {mb} MB; \
                         aborting to prevent host OOM. Override via {ENV_OVERRIDE}.{took}",
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

    /// The second half of the same regression: `dev` spawns servers
    /// through `cargo run`, so the thing that keeps the port is the
    /// GRANDCHILD. Killing the handle alone left it running.
    #[test]
    fn a_grandchild_dies_with_the_tree() {
        // sh -> sleep: one generation below the handle we hold, exactly
        // like cargo -> server.
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 60 & wait"])
            .spawn()
            .expect("spawn a two-generation stand-in for `cargo run`");
        let root = child.id();

        // Let the shell get as far as forking its own child.
        let mut grandchild = None;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let found: Vec<u32> = process_tree(root).into_iter().filter(|p| *p != root).collect();
            if let Some(pid) = found.first() {
                grandchild = Some(*pid);
                break;
            }
        }
        let grandchild = grandchild.expect("the shell should have forked a sleep");

        assert!(kill_tree(root), "the root must still have been there");
        let _ = child.wait();

        // "Dead" has to mean NOT RUNNING, not "the pid is gone".
        // Nothing reaps the grandchild once its shell is killed — on a
        // host whose PID 1 does not reap (a container running a bare
        // `sleep infinity`, which is where this was written), it stays
        // in the table as a zombie for ever and `kill(pid, 0)` keeps
        // succeeding. A zombie holds no port and runs no code, which is
        // all this kill is for.
        for _ in 0..50 {
            match std::process::Command::new("ps")
                .args(["-o", "state=", "-p", &grandchild.to_string()])
                .output()
            {
                Ok(out) => {
                    let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if state.is_empty() || state.starts_with('Z') {
                        return;
                    }
                }
                Err(_) => return,
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("grandchild {grandchild} was still running after the tree kill");
    }

    /// The regression this registry exists for: the cap used to abort
    /// with the session's servers still running, re-parented to init
    /// and still holding their ports.
    ///
    /// Both halves in one test on purpose — the registry is a global,
    /// and two tests writing it would race under cargo's thread pool.
    #[test]
    fn watched_children_are_killed_and_a_dead_session_kills_nothing() {
        let child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn a sleep to stand in for a dev server");
        let pid = child.id() as i32;
        let children = std::sync::Arc::new(std::sync::Mutex::new(vec![child]));
        watch_children(&children);

        assert_eq!(kill_watched_processes(), 1, "the registered child must be killed");
        // Killed AND reaped: `wait` ran, so the pid is gone rather than
        // sitting in the table as a zombie.
        // SAFETY: signal 0 checks for existence and delivers nothing.
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "pid {pid} still exists after the kill",
        );

        // A session that ENDED leaves only a dangling Weak. Nothing to
        // kill, and nothing to panic about either.
        drop(children);
        assert_eq!(kill_watched_processes(), 0, "a dropped session must kill nothing");
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

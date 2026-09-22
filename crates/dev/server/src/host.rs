//! Long-lived runtime-server dev-host runtime.
//!
//! Pre-refactor, every project that ran `idealyst dev --aas` got a
//! freshly-generated `<project>-runtime-server-host/src/main.rs` (~280 lines)
//! pasted out of a `format!` string in `build-runtime-server`. That template
//! reached into [`crate::sidecar::SidecarIn`] and the rest of this
//! crate's transport API directly, so any internal refactor of
//! `SidecarIn` / `SidecarOut` shape (struct ⇄ tuple variants,
//! field rename, …) shattered every project whose pinned framework
//! rev still emitted the old template — even though the runtime
//! API was perfectly capable of supporting an out-of-tree CLI.
//!
//! This module owns that loop. The generated host wrapper is now
//! ~25 lines: build a [`HostConfig`], optionally build a
//! [`HotPatchAdapter`], call [`run`]. Internal IPC churn stops at
//! this crate's boundary.
//!
//! ## What "host" means here
//!
//! The host is the long-lived dev-side process that:
//! - Listens for runtime-server client WebSockets (one per attached device /
//!   browser tab) and writes its bound port to
//!   `IDEALYST_RUNTIME_SERVER_PORT_FILE` so the CLI parent can bake
//!   `IDEALYST_DEV_ENDPOINT=ws://host:port` into platform wrappers.
//! - Spawns the *sidecar* child process and mirrors its outbound
//!   wire commands into one [`WireRecordingBackend`] per session.
//! - Watches the user-source tree and either ships a subsecond
//!   `JumpTable` into the running sidecar (fast path — clients
//!   never reconnect) or SIGKILL-and-respawns the sidecar
//!   (fallback — clients catch-up-replay).
//!
//! The host *does not* link the user crate. That's the sidecar's
//! job, kept separate so a build-time crash in the user crate only
//! takes down the sidecar; the host keeps every connected client
//! online while a fix is typed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::sidecar::{Sidecar, SidecarIn};
use crate::{
    serve_with_sidecar_and_tracker, spawn_change_loop, SessionMode, SessionTracker, SidecarSlot,
    WireRecordingBackend,
};

/// Re-export so the generated host wrapper can name `JumpTable`
/// through `dev_server::host::JumpTable` instead of needing a direct
/// `subsecond-types` dep. Same type, one fewer Cargo.toml entry for
/// the wrapper to keep in sync.
pub use subsecond_types::JumpTable;

/// Where the host's runtime points are anchored on the filesystem.
///
/// Every field is *project-specific*: paths produced by the build
/// orchestrator under `<project>/target/idealyst/<project>/aas/…`,
/// or identifiers read from the project's `Cargo.toml`. Nothing in
/// here references the framework's own checkout, which is what
/// lets out-of-tree projects run runtime-server without an
/// `idealyst-native/` ancestor on disk.
pub struct HostConfig {
    /// `addr:port` to bind the WebSocket listener. The host accepts
    /// `0.0.0.0:0` and lets the OS pick a free port; the chosen port
    /// is written to `IDEALYST_RUNTIME_SERVER_PORT_FILE` so the CLI
    /// parent can pick it up and bake `IDEALYST_DEV_ENDPOINT=ws://host:port`
    /// into platform wrappers at build time.
    pub bind_addr: String,
    /// Absolute path to the prebuilt sidecar binary. The host spawns
    /// this on startup and respawns it via cargo on hot-patch
    /// failure (see `sidecar_manifest` + `cargo_target` below).
    pub sidecar_path: PathBuf,
    /// `Cargo.toml` of the sidecar wrapper crate. Passed to
    /// `cargo build --manifest-path` during the respawn fallback.
    pub sidecar_manifest: PathBuf,
    /// Target dir shared with the sidecar wrapper's
    /// `.cargo/config.toml` — `cargo build --target-dir` for
    /// respawn lands the rebuilt binary back at `sidecar_path`.
    pub cargo_target: PathBuf,
    /// Directory the file watcher subscribes to. Conventionally the
    /// project's `src/`, but the build orchestrator picks the
    /// concrete path so the host stays agnostic to project layout.
    pub user_src: PathBuf,
    /// User crate name, threaded into the hot-patch adapter so the
    /// builder knows which captured rustc invocation to replay
    /// (only the user crate's rcgu objects get re-emitted per
    /// patch; framework crates stay cached).
    pub user_crate: String,
}

/// What the host knows about the running sidecar when it asks for a
/// patch.
///
/// A struct rather than three arguments because it has grown twice
/// already and the generated host wrapper is compiled from a template
/// — every signature change there is a wrapper every pinned project
/// has to regenerate.
pub struct PatchRequest<'a> {
    /// Which crate's captured rustc invocation to replay. Only that
    /// crate's objects are re-emitted; framework crates stay cached.
    pub user_crate: &'a str,
    /// The sidecar's runtime `main` address, against which the builder
    /// computes the ASLR slide.
    pub aslr_reference: u64,
    /// The sidecar's runtime address for the user crate's app ROOT fn,
    /// or `0` if it did not report one.
    ///
    /// The root is plain user code — not a `#[component]` — so it has
    /// no `__*_hot_impl` symbol to pair by name. Its address is how the
    /// builder finds the symbol anyway. An app whose whole tree lives
    /// in `app()` depends on this entry for a patch to change anything.
    pub app_reference: u64,
}

/// Bridge between the host's "I have a file change, please give me a
/// fresh `JumpTable`" expectation and whatever produces it on the
/// build side.
///
/// Defined here (not in `build-runtime-server`) so `dev-server` can call into
/// it without depending on `build-runtime-server`. The wrapper main wires up
/// the concrete impl, keeping the cross-crate edge thin.
pub trait HotPatchAdapter: Send + Sync {
    /// Produce a `JumpTable` for [`PatchRequest::user_crate`] against
    /// the running sidecar. Returning `Err` triggers respawn fallback —
    /// the host will SIGKILL + cargo-build + respawn the sidecar from
    /// scratch. The host logs `Err`'s `{e:#}` so implementers should
    /// include context.
    fn build(&self, req: &PatchRequest<'_>) -> anyhow::Result<JumpTable>;

    /// Extra environment the respawn fallback's `cargo build` must run
    /// with.
    ///
    /// A respawn is not a neutral operation for the fast path: the
    /// initial "fat" build sets `RUSTFLAGS` and a rustc wrapper that
    /// together keep the artifacts a patch needs (saved object files,
    /// undropped symbols, captured invocations). Rebuilding without
    /// them recompiles the whole graph from a different fingerprint,
    /// leaves the captures describing the PREVIOUS source, and relinks
    /// the sidecar with symbols stripped. The adapter owns that
    /// knowledge; the host just applies what it is handed.
    fn rebuild_env(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Called after the host has respawned the sidecar from a fresh
    /// binary.
    ///
    /// The adapter caches the sidecar's symbol table, and a relink
    /// moves every address in it. Patching against the stale copy does
    /// not render stale UI — it jumps into unrelated text and takes the
    /// process down. Returning `Err` therefore means "the fast path is
    /// no longer safe", and the host retires the adapter for the rest
    /// of the session rather than risk it.
    fn sidecar_rebuilt(&self, _sidecar_bin: &std::path::Path) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Run the dev-host loop. Blocks the calling thread until the
/// WebSocket listener exits (typically Ctrl-C / SIGTERM).
///
/// `hot_patch` is `Option` because the builder can fail to
/// initialize (corrupt symbol table, missing captures dir, …) and
/// we still want the host to come up — it just falls back to
/// SIGKILL-respawn on every file change, which is slower but
/// preserves the WebSocket listener (clients still don't
/// reconnect, just catch-up-replay).
pub fn run(
    cfg: HostConfig,
    hot_patch: Option<Box<dyn HotPatchAdapter>>,
) -> std::io::Result<()> {
    let HostConfig {
        bind_addr,
        sidecar_path,
        sidecar_manifest,
        cargo_target,
        user_src,
        user_crate,
    } = cfg;

    let recorder = WireRecordingBackend::new();
    let sidecar_slot: SidecarSlot = Arc::new(Mutex::new(None));
    let session_tracker = SessionTracker::new();

    // Install signal handlers BEFORE spawning the sidecar so a
    // ctrl-C / SIGTERM / SIGHUP between spawn and the main loop
    // still gets the sidecar killed cleanly. Without this, the
    // host would exit and the sidecar would reparent to PID 1 —
    // every dev session that ended via the orchestrator's
    // `child.kill()` (or a terminal close) leaked one
    // `nicho-portfolio-runtime-server-app` process. ctrlc's `termination`
    // feature catches SIGINT + SIGTERM + SIGHUP on Unix.
    //
    // The handler is install-once-per-process; calling it twice
    // returns `Error::MultipleHandlers`. We ignore that — the host
    // binary only ever calls `host::run` once, and a duplicate
    // install just means a previous installer (e.g. the orchestrator
    // for in-process tests) still owns the signal.
    {
        let sidecar_for_signal = sidecar_slot.clone();
        let _ = ctrlc::set_handler(move || {
            eprintln!("[runtime-server-host] received signal — killing sidecar before exit");
            if let Ok(mut guard) = sidecar_for_signal.lock() {
                if let Some(mut sidecar) = guard.take() {
                    sidecar.kill();
                }
            }
            std::process::exit(0);
        });
    }

    // Parent-PID watchdog. If the CLI orchestrator dies in a way
    // the signal handler can't catch — SIGKILL, force-quit from
    // Activity Monitor, panic-induced abort, system reboot mid-
    // session — we reparent to launchd (pid 1) and would otherwise
    // run forever, squatting on our WebSocket port. Future
    // `idealyst dev` runs would then collide on the port-file
    // sentinel, and any stale baked endpoint would land on a host
    // that's no longer watching files — manifesting as "hot-reload
    // silently does nothing."
    //
    // Poll `getppid()` every 500ms; if it ever returns 1 (or 0 on
    // weird kernels), the parent is gone — kill the sidecar and
    // exit ourselves. Cheap (one syscall), no extra deps. Unix-
    // only because Windows doesn't have an equivalent simple
    // parent-died signal (`JobObject` would be the proper port).
    #[cfg(unix)]
    {
        let sidecar_for_watchdog = sidecar_slot.clone();
        std::thread::spawn(move || {
            let original_parent = unsafe { libc::getppid() };
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let now_parent = unsafe { libc::getppid() };
                if now_parent != original_parent {
                    eprintln!(
                        "[runtime-server-host] parent pid changed ({} → {}); orchestrator \
                         is gone, killing sidecar + exiting",
                        original_parent, now_parent,
                    );
                    if let Ok(mut guard) = sidecar_for_watchdog.lock() {
                        if let Some(mut sidecar) = guard.take() {
                            sidecar.kill();
                        }
                    }
                    std::process::exit(0);
                }
            }
        });
    }

    match Sidecar::spawn(&sidecar_path) {
        Ok(s) => {
            *sidecar_slot.lock().unwrap() = Some(s);
            eprintln!("[runtime-server-host] sidecar spawned: {}", sidecar_path.display());
        }
        Err(e) => {
            eprintln!(
                "[runtime-server-host] sidecar spawn failed: {e} — host running idle (no UI will render)"
            );
        }
    }

    let hot_patch = hot_patch.map(Arc::new);
    if hot_patch.is_none() {
        eprintln!(
            "[runtime-server-host] hot-patch adapter unavailable — file changes will trigger \
             respawn instead of in-place patch (~slower, but clients stay attached)"
        );
    }

    let sidecar_for_rebuild = sidecar_slot.clone();
    let hotpatch_for_rebuild = hot_patch.clone();
    // The watch callback is the only reader/writer and the change loop
    // runs it on one thread, so a `Cell` is enough. See its use below.
    let retired = std::cell::Cell::new(false);
    let tracker_for_rebuild = session_tracker.clone();
    let sidecar_path_for_rebuild = sidecar_path.clone();
    let sidecar_manifest_for_rebuild = sidecar_manifest.clone();
    let cargo_target_for_rebuild = cargo_target.clone();
    let user_crate_for_rebuild = user_crate.clone();

    // The crate the sidecar was built from, for the overlay archive.
    // `user_src` is conventionally `<crate>/src`, and the archive the
    // build wrote lives under that crate's own target dir.
    let overlay_crate_dir = user_src.parent().map(|p| p.to_path_buf());
    let overlay_sidecar = sidecar_slot.clone();
    let mut overlay_archive: Option<dev_overlay::DescriptorSet> = overlay_crate_dir
        .as_ref()
        .and_then(|dir| dev_overlay::load_archive(dir, &package_name_of(dir)));
    if overlay_archive.is_none() {
        eprintln!(
            "[runtime-server-host] no overlay descriptor set — every save will rebuild \
             (run `idealyst dev` so the build writes one)"
        );
    }

    spawn_change_loop(
        vec![user_src.clone()],
        std::time::Duration::from_millis(100),
        Box::new(move |changed: &[std::path::PathBuf]| {
            // What does this save cost? Three live answers, cheapest
            // first, decided before anything expensive starts:
            //
            //   Patch     — literals only. No compiler at all. The
            //               sidecar edits its own mounted tree, so the
            //               resulting backend calls stream to every
            //               attached client as ordinary commands AND
            //               update the recorder's scene mirror, which
            //               is what a late joiner is snapshotted from.
            //   HotPatch  — function bodies only. Re-emit the user
            //               crate, link a dylib, rebind the jump table,
            //               re-run the mounted tree. Still in process:
            //               no respawn, no reconnect.
            //   Rebuild   — anything that moves a file's SHAPE.
            //
            // The last boundary is safety, not speed: a patch dylib is
            // spliced into a process where every other crate is still
            // the old build, so a changed layout would be corruption
            // rather than a stale screen. Which is why a hot patch is
            // attempted ONLY on an explicit `HotPatch` — an absent or
            // unreadable archive means "cannot tell", and cannot-tell
            // respawns.
            let mut hot_patch_allowed = false;
            if let (Some(dir), Some(archive)) =
                (overlay_crate_dir.as_ref(), overlay_archive.as_ref())
            {
                let started = std::time::Instant::now();
                let files = read_changed_files(dir, changed);
                match dev_overlay::decide(Some(archive), &files) {
                    dev_overlay::Decision::Patch(patches) => {
                        let count = patches.len();
                        send_overlay_patches(&overlay_sidecar, &patches);
                        if let Some(archive) = overlay_archive.as_mut() {
                            dev_overlay::advance_archive(archive, &files);
                        }
                        eprintln!(
                            "[dev] patched {count} site(s) in {} ms, no rebuild",
                            started.elapsed().as_millis()
                        );
                        return;
                    }
                    dev_overlay::Decision::HotPatch(files) => {
                        eprintln!("[dev] hot-patching bodies in: {}", files.join(", "));
                        hot_patch_allowed = true;
                    }
                    dev_overlay::Decision::Unchanged if !files.is_empty() => {
                        eprintln!("[dev] no UI or code change in this save, no rebuild");
                        return;
                    }
                    dev_overlay::Decision::Unchanged => {}
                    dev_overlay::Decision::Rebuild(why) => {
                        eprintln!("[dev] rebuilding: {why}");
                    }
                }
            }

            let t_total = std::time::Instant::now();
            let force_respawn = std::env::var("IDEALYST_RUNTIME_SERVER_NO_HOTPATCH")
                .ok()
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false);
            // `retired` latches once the adapter's cached view of the
            // sidecar binary can no longer be refreshed — see
            // `note_respawn`. Cheaper to lose the fast path than to
            // patch against addresses that have moved.
            let adapter: Option<&dyn HotPatchAdapter> = if retired.get() || !hot_patch_allowed {
                None
            } else {
                hotpatch_for_rebuild.as_deref().map(|b| &**b)
            };
            // A respawn keeps the adapter (it supplies the fat build
            // env); it is only the DECISION to patch that
            // `hot_patch_allowed` gates.
            let respawn_adapter: Option<&dyn HotPatchAdapter> = if retired.get() {
                None
            } else {
                hotpatch_for_rebuild.as_deref().map(|b| &**b)
            };
            let mut respawn = |why: &str| {
                respawn_sidecar(
                    &sidecar_for_rebuild,
                    &tracker_for_rebuild,
                    &sidecar_path_for_rebuild,
                    &sidecar_manifest_for_rebuild,
                    &cargo_target_for_rebuild,
                    respawn_adapter,
                );
                if !note_respawn(respawn_adapter, &sidecar_path_for_rebuild) {
                    retired.set(true);
                }
                eprintln!(
                    "[runtime-server-host] respawn applied in {}ms ({why})",
                    t_total.elapsed().as_millis()
                );
            };
            if force_respawn {
                respawn("force_respawn");
                rescan_archive(&mut overlay_archive, overlay_crate_dir.as_deref());
                return;
            }
            if let Err(e) = try_hotpatch(adapter, &sidecar_for_rebuild, &user_crate_for_rebuild) {
                if adapter.is_some() {
                    eprintln!("[runtime-server-host] hot-patch failed: {e:#} — respawning sidecar");
                }
                respawn("rebuild");
            } else {
                eprintln!(
                    "[runtime-server-host] hot-patch applied in {}ms",
                    t_total.elapsed().as_millis()
                );
            }
            // Either way the running binary is not the one the archive
            // describes any more: a rebuild relinks it, and a hot patch
            // splices in re-emitted `ui!` sites whose keys were computed
            // from the NEW line numbers. Re-scanning is what keeps the
            // next literal-only save addressable — without it the
            // overlay would emit patches keyed to sites the running code
            // no longer carries, and they would silently do nothing.
            rescan_archive(&mut overlay_archive, overlay_crate_dir.as_deref());
        }),
    );

    let port_mirror: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));

    if let Ok(path) = std::env::var("IDEALYST_RUNTIME_SERVER_PORT_FILE") {
        let port_for_file = port_mirror.clone();
        std::thread::spawn(move || {
            // Ensure the sentinel's parent dir exists before the
            // write loop starts. Pre-fix every fresh project printed
            // `could not write port sentinel … No such file or
            // directory (os error 2)` on first launch because the
            // path lives under `target/idealyst/<project>/aas/` —
            // a dir build-runtime-server creates only when the host wrapper
            // itself is compiled, not when the orchestrator points
            // a pre-built host at the path. mkdir_p is idempotent.
            if let Some(parent) = std::path::Path::new(&path).parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!(
                        "[runtime-server-host] could not create port-sentinel parent {}: {}",
                        parent.display(),
                        e
                    );
                }
            }
            for _ in 0..200 {
                if let Ok(g) = port_for_file.lock() {
                    if let Some(p) = *g {
                        if let Err(e) = std::fs::write(&path, p.to_string()) {
                            eprintln!(
                                "[runtime-server-host] could not write port sentinel {}: {}",
                                path, e
                            );
                        } else {
                            eprintln!("[runtime-server-host] wrote bound port {} to {}", p, path);
                        }
                        return;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            eprintln!(
                "[runtime-server-host] timed out waiting for serve to bind; no port sentinel written"
            );
        });
    }

    replay_sessions_to_sidecar(&sidecar_slot, &session_tracker);

    // NOTE: an earlier version of this file also spawned a 500ms
    // `try_wait` liveness watcher that auto-respawned on silent
    // sidecar crashes. It got reverted because respawn doesn't
    // resynchronize the host's per-session mirror with the fresh
    // sidecar's fresh-mount commands — existing client tabs ended up
    // seeing a frozen-but-stale UI (the mirror double-up-ed: old
    // CreateView/Insert + new CreateView/Insert for the same NodeIds).
    //
    // The clean fix needs: on detected sidecar death, force-close
    // every attached client WS so they reconnect from scratch (new
    // session id, fresh mirror, fresh mount). That's plumbing that
    // crosses host.rs ↔ transport.rs and warrants a focused design
    // pass. Until then: the fail-fast in
    // `crates/build/runtime-server/src/hotpatch/stub.rs` catches the most
    // common crash class (Rust-internal monomorphization deferrals)
    // up-front, routing through the existing `try_hotpatch` →
    // `respawn_sidecar` fallback — which DOES coordinate mirror
    // state because it runs synchronously through the watch loop.
    //
    // For other crash modes (e.g. `_sin`/`_cos`-only deferrals that
    // still SIGSEGV the rerender path on some incremental-build
    // states), the recovery is currently: Ctrl-C + restart
    // `idealyst dev --aas`. Or set `IDEALYST_RUNTIME_SERVER_NO_HOTPATCH=1` to
    // force every edit through the respawn path.

    let session_mode = SessionMode::from_env();
    eprintln!(
        "[runtime-server-host] starting (session mode = {:?})",
        session_mode,
    );
    serve_with_sidecar_and_tracker(
        bind_addr,
        recorder,
        port_mirror,
        sidecar_slot,
        session_tracker,
        session_mode,
    )
}

/// Send `CreateSession` to the live sidecar for every session id the
/// tracker knows about. No-op when the slot is empty. Called once on
/// startup (idempotent for an empty tracker) and after every respawn.
fn replay_sessions_to_sidecar(slot: &SidecarSlot, tracker: &SessionTracker) {
    let sessions = tracker.snapshot();
    if sessions.is_empty() {
        return;
    }
    let Ok(guard) = slot.lock() else {
        return;
    };
    let Some(sidecar) = guard.as_ref() else {
        return;
    };
    eprintln!(
        "[runtime-server-host] replaying {} session(s) to fresh sidecar",
        sessions.len(),
    );
    for (s, viewport) in sessions {
        // ensure_session records the create on this (fresh, post-
        // respawn) sidecar generation so the event-forward path won't
        // redundantly re-create it on every subsequent event.
        sidecar.ensure_session(&s, viewport);
    }
}

/// One hot-patch round. Pulls the cached ASLR reference out of the
/// running sidecar, asks the adapter for a fresh `JumpTable`, and
/// ships it back over the existing IPC. Any failure returns Err so
/// the caller can fall back to respawn.
fn try_hotpatch(
    builder: Option<&dyn HotPatchAdapter>,
    sidecar_slot: &SidecarSlot,
    user_crate: &str,
) -> anyhow::Result<()> {
    let builder = builder.ok_or_else(|| anyhow::anyhow!("hot-patch adapter unavailable"))?;
    let (aslr, app_reference) = {
        let g = sidecar_slot
            .lock()
            .map_err(|_| anyhow::anyhow!("sidecar slot lock poisoned"))?;
        let s = g
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no sidecar running"))?;
        let v = s.aslr_reference();
        if v == 0 {
            return Err(anyhow::anyhow!(
                "sidecar has not reported aslr_reference yet"
            ));
        }
        (v, s.app_reference())
    };
    let table = builder.build(&PatchRequest {
        user_crate,
        aslr_reference: aslr,
        app_reference,
    })?;
    let table_json = serde_json::to_string(&table)?;
    let g = sidecar_slot
        .lock()
        .map_err(|_| anyhow::anyhow!("sidecar slot lock poisoned"))?;
    let s = g
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("sidecar slot empty"))?;
    s.send(SidecarIn::ApplyPatch { table_json });
    Ok(())
}

/// Fallback path: rebuild the sidecar via cargo, kill the old, spawn
/// the new. After respawn we replay every live session id so
/// already-connected clients pick up where they left off without
/// reconnecting.
fn respawn_sidecar(
    sidecar_slot: &SidecarSlot,
    tracker: &SessionTracker,
    sidecar_path: &std::path::Path,
    sidecar_manifest: &std::path::Path,
    cargo_target: &std::path::Path,
    hot_patch: Option<&dyn HotPatchAdapter>,
) {
    let status = respawn_cargo_command(sidecar_manifest, cargo_target, hot_patch).status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!(
                "[runtime-server-host] respawn cargo build exited with {s} — sidecar unchanged"
            );
            return;
        }
        Err(e) => {
            eprintln!("[runtime-server-host] respawn cargo build spawn failed: {e}");
            return;
        }
    }
    if let Ok(mut g) = sidecar_slot.lock() {
        if let Some(mut old) = g.take() {
            old.kill();
        }
        match Sidecar::spawn(sidecar_path) {
            Ok(s) => {
                *g = Some(s);
                eprintln!("[runtime-server-host] sidecar respawned");
            }
            Err(e) => eprintln!("[runtime-server-host] sidecar respawn failed: {e}"),
        }
    }
    replay_sessions_to_sidecar(sidecar_slot, tracker);
}

/// Re-read the crate's `ui!` sites into the archive after the running
/// binary changed.
///
/// See the call site for why this has to happen on both the hot-patch
/// and the rebuild paths. Failure leaves the previous archive in place
/// and logs — a stale archive costs the overlay tier, never
/// correctness, because every patch it then proposes is addressed at a
/// site key the sidecar will simply not find.
fn rescan_archive(
    archive: &mut Option<dev_overlay::DescriptorSet>,
    crate_dir: Option<&std::path::Path>,
) {
    let Some(dir) = crate_dir else { return };
    match dev_overlay::scan_crate(dir) {
        Ok(fresh) => *archive = Some(fresh),
        Err(e) => eprintln!(
            "[runtime-server-host] could not re-scan `ui!` sites after the rebuild ({e:#}) — \
             literal-only saves will rebuild until the next restart"
        ),
    }
}

/// The respawn's `cargo build`, with the adapter's environment applied.
///
/// Split out so the env can be asserted without spawning cargo. The env
/// is the whole point: rebuilding the sidecar WITHOUT the fat build's
/// `RUSTFLAGS` + rustc wrapper keys a different cargo fingerprint (so
/// every fallback recompiles the entire graph instead of a few crates),
/// leaves the per-crate rustc captures describing the previous source,
/// and relinks the binary without `-Clink-dead-code` — which strips the
/// symbols the next patch's stub has to resolve. One dropped env var
/// turns the fast path off for the rest of the session, silently.
fn respawn_cargo_command(
    sidecar_manifest: &std::path::Path,
    cargo_target: &std::path::Path,
    hot_patch: Option<&dyn HotPatchAdapter>,
) -> std::process::Command {
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["build", "--manifest-path"])
        .arg(sidecar_manifest)
        .arg("--target-dir")
        .arg(cargo_target);
    if let Some(a) = hot_patch {
        for (k, v) in a.rebuild_env() {
            cmd.env(k, v);
        }
    }
    cmd
}

/// Tell the adapter the sidecar binary was relinked, and report whether
/// the fast path is still safe.
///
/// `false` means the adapter could not re-read the new binary, so every
/// address it holds is stale. A patch built from a stale cache does not
/// render old UI — it jumps into whatever now occupies those offsets and
/// kills the sidecar. The caller retires the adapter on `false`; respawn
/// alone is slower but always correct.
fn note_respawn(hot_patch: Option<&dyn HotPatchAdapter>, sidecar_path: &std::path::Path) -> bool {
    let Some(a) = hot_patch else { return true };
    match a.sidecar_rebuilt(sidecar_path) {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "[runtime-server-host] could not re-read the respawned sidecar ({e:#}) — \
                 hot-patching is DISABLED for the rest of this session (every save will \
                 respawn). Restart `idealyst dev` to get it back."
            );
            false
        }
    }
}

/// `[package] name` of the crate at `dir`, for locating its overlay
/// archive.
fn package_name_of(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("Cargo.toml"))
        .ok()
        .and_then(|t| toml::from_str::<toml::Value>(&t).ok())
        .and_then(|m| m.get("package")?.get("name")?.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Read the changed files the decision needs, as package-relative
/// paths.
///
/// A path outside the crate — a watched path dependency — is skipped,
/// so the decision never sees it and the save falls to a rebuild. That
/// is the right answer: the archive describes THIS crate, and a
/// dependency's sites are compiled into a different artifact.
fn read_changed_files(
    dir: &std::path::Path,
    paths: &[std::path::PathBuf],
) -> Vec<dev_overlay::ChangedFile> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(dir) else { continue };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if !seen.insert(relative.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        out.push(dev_overlay::ChangedFile { path: relative, text });
    }
    out
}

/// Hand decided patches to the sidecar, which applies each to its own
/// mounted scene.
///
/// One frame per site: a patch is per-site by construction, and keeping
/// them separate means one site's refusal cannot take another's edit
/// down with it.
fn send_overlay_patches(slot: &SidecarSlot, patches: &[dev_overlay::SitePatch]) {
    let Ok(guard) = slot.lock() else {
        eprintln!("[runtime-server-host] sidecar slot poisoned; overlay patch dropped");
        return;
    };
    let Some(sidecar) = guard.as_ref() else {
        eprintln!("[runtime-server-host] no sidecar; overlay patch dropped");
        return;
    };
    for patch in patches {
        match serde_json::to_string(&dev_overlay::wire_payload(patch)) {
            Ok(patch_json) => sidecar.send(SidecarIn::OverlayPatch { patch_json }),
            Err(e) => eprintln!("[runtime-server-host] cannot encode overlay patch: {e}"),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// An adapter that records what the host asked of it.
    struct FakeAdapter {
        env: Vec<(String, String)>,
        rebuilt_ok: bool,
        rebuilt_calls: AtomicUsize,
    }

    impl HotPatchAdapter for FakeAdapter {
        fn build(&self, _req: &PatchRequest<'_>) -> anyhow::Result<JumpTable> {
            anyhow::bail!("not used in these tests")
        }
        fn rebuild_env(&self) -> Vec<(String, String)> {
            self.env.clone()
        }
        fn sidecar_rebuilt(&self, _bin: &Path) -> anyhow::Result<()> {
            self.rebuilt_calls.fetch_add(1, Ordering::Relaxed);
            if self.rebuilt_ok {
                Ok(())
            } else {
                anyhow::bail!("symbol table unreadable")
            }
        }
    }

    fn fake(rebuilt_ok: bool) -> FakeAdapter {
        FakeAdapter {
            env: vec![
                ("RUSTFLAGS".into(), "-Csave-temps=true -Clink-dead-code".into()),
                ("RUSTC_WRAPPER".into(), "/path/to/idealyst".into()),
            ],
            rebuilt_ok,
            rebuilt_calls: AtomicUsize::new(0),
        }
    }

    fn envs_of(cmd: &std::process::Command) -> Vec<(String, String)> {
        cmd.get_envs()
            .filter_map(|(k, v)| {
                Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned()))
            })
            .collect()
    }

    /// The respawn fallback used to run a bare `cargo build`, dropping
    /// the fat build's `RUSTFLAGS` and rustc wrapper. Two consequences,
    /// both silent: cargo saw a different fingerprint and recompiled the
    /// WHOLE graph on every fallback (observed as a multi-minute
    /// "respawn" on a warm tree), and the relinked sidecar lost both the
    /// dead-code symbols and the refreshed rustc captures the next patch
    /// depends on — so the fast path stayed broken afterwards.
    #[test]
    fn regression_respawn_build_carries_the_adapters_fat_env() {
        let a = fake(true);
        let cmd = respawn_cargo_command(
            Path::new("/p/Cargo.toml"),
            Path::new("/p/target"),
            Some(&a),
        );
        let envs = envs_of(&cmd);
        assert!(
            envs.iter().any(|(k, v)| k == "RUSTFLAGS" && v.contains("-Clink-dead-code")),
            "respawn must keep the fat build's RUSTFLAGS: {envs:?}"
        );
        assert!(
            envs.iter().any(|(k, _)| k == "RUSTC_WRAPPER"),
            "respawn must keep the capture wrapper: {envs:?}"
        );
    }

    /// No adapter (hot-patching unavailable) is still a valid session —
    /// the respawn just runs plain cargo.
    #[test]
    fn respawn_without_an_adapter_sets_no_extra_env() {
        let cmd =
            respawn_cargo_command(Path::new("/p/Cargo.toml"), Path::new("/p/target"), None);
        assert!(envs_of(&cmd).is_empty(), "{:?}", envs_of(&cmd));
    }

    /// A respawn relinks the sidecar, so every link-time address the
    /// adapter cached moves. It MUST be told, and a refusal must retire
    /// the fast path rather than let the next patch resolve its stub
    /// trampolines and jump-table targets against addresses that are no
    /// longer there — which does not render stale UI, it jumps into
    /// unrelated text and kills the sidecar.
    #[test]
    fn regression_respawn_tells_the_adapter_the_binary_moved() {
        let a = fake(true);
        assert!(note_respawn(Some(&a), Path::new("/p/sidecar")));
        assert_eq!(a.rebuilt_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn regression_a_respawn_the_adapter_cannot_re_read_retires_the_fast_path() {
        let a = fake(false);
        assert!(
            !note_respawn(Some(&a), Path::new("/p/sidecar")),
            "an unreadable sidecar must disable patching, not be ignored"
        );
    }

    #[test]
    fn note_respawn_is_a_no_op_without_an_adapter() {
        assert!(note_respawn(None, Path::new("/p/sidecar")));
    }
}

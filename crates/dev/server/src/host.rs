//! Long-lived AAS dev-host runtime.
//!
//! Pre-refactor, every project that ran `idealyst dev --aas` got a
//! freshly-generated `<project>-aas-host/src/main.rs` (~280 lines)
//! pasted out of a `format!` string in `build-aas`. That template
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
//! - Listens for AAS client WebSockets (one per attached device /
//!   browser tab) and advertises itself via mDNS.
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
/// lets out-of-tree projects run AAS without an
/// `idealyst-native/` ancestor on disk.
pub struct HostConfig {
    /// `addr:port` to bind the WebSocket listener. The host accepts
    /// `0.0.0.0:0` and lets the OS pick a free port; the chosen port
    /// is then published over mDNS (so clients discover the host
    /// without anyone coordinating a fixed number) and, if
    /// `IDEALYST_AAS_PORT_FILE` is set, written there too so the CLI
    /// parent doesn't have to trust mDNS browse caching across
    /// restarts.
    pub bind_addr: String,
    /// mDNS-published service identifier. Clients filter the
    /// `_idealyst-dev._tcp` browse stream by this id to pick the
    /// right dev-server when multiple projects' hosts are running
    /// on the same network.
    pub app_id: String,
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

/// Bridge between the host's "I have a file change, please give me a
/// fresh `JumpTable`" expectation and whatever produces it on the
/// build side.
///
/// Defined here (not in `build-aas`) so `dev-server` can call into
/// it without depending on `build-aas`. The wrapper main wires up
/// the concrete impl, keeping the cross-crate edge thin.
pub trait HotPatchAdapter: Send + Sync {
    /// Produce a `JumpTable` for `user_crate` against the sidecar's
    /// current ASLR slide. Returning `Err` triggers respawn
    /// fallback — the host will SIGKILL + cargo-build + respawn
    /// the sidecar from scratch. The host logs `Err`'s `{e:#}` so
    /// implementers should include context.
    fn build(
        &self,
        user_crate: &str,
        aslr_reference: u64,
    ) -> anyhow::Result<JumpTable>;
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
        app_id,
        sidecar_path,
        sidecar_manifest,
        cargo_target,
        user_src,
        user_crate,
    } = cfg;

    let recorder = WireRecordingBackend::new();
    let sidecar_slot: SidecarSlot = Arc::new(Mutex::new(None));
    let session_tracker = SessionTracker::new();

    match Sidecar::spawn(&sidecar_path) {
        Ok(s) => {
            *sidecar_slot.lock().unwrap() = Some(s);
            eprintln!("[aas-host] sidecar spawned: {}", sidecar_path.display());
        }
        Err(e) => {
            eprintln!(
                "[aas-host] sidecar spawn failed: {e} — host running idle (no UI will render)"
            );
        }
    }

    let hot_patch = hot_patch.map(Arc::new);
    if hot_patch.is_none() {
        eprintln!(
            "[aas-host] hot-patch adapter unavailable — file changes will trigger \
             respawn instead of in-place patch (~slower, but clients stay attached)"
        );
    }

    let sidecar_for_rebuild = sidecar_slot.clone();
    let hotpatch_for_rebuild = hot_patch.clone();
    let tracker_for_rebuild = session_tracker.clone();
    let sidecar_path_for_rebuild = sidecar_path.clone();
    let sidecar_manifest_for_rebuild = sidecar_manifest.clone();
    let cargo_target_for_rebuild = cargo_target.clone();
    let user_crate_for_rebuild = user_crate.clone();
    spawn_change_loop(
        vec![user_src],
        std::time::Duration::from_millis(100),
        Box::new(move || {
            let t_total = std::time::Instant::now();
            let force_respawn = std::env::var("IDEALYST_AAS_NO_HOTPATCH")
                .ok()
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false);
            if force_respawn {
                respawn_sidecar(
                    &sidecar_for_rebuild,
                    &tracker_for_rebuild,
                    &sidecar_path_for_rebuild,
                    &sidecar_manifest_for_rebuild,
                    &cargo_target_for_rebuild,
                );
                eprintln!(
                    "[aas-host] respawn applied in {}ms (force_respawn)",
                    t_total.elapsed().as_millis()
                );
                return;
            }
            if let Err(e) = try_hotpatch(
                hotpatch_for_rebuild.as_deref().map(|b| &**b),
                &sidecar_for_rebuild,
                &user_crate_for_rebuild,
            ) {
                eprintln!("[aas-host] hot-patch failed: {e:#} — respawning sidecar");
                respawn_sidecar(
                    &sidecar_for_rebuild,
                    &tracker_for_rebuild,
                    &sidecar_path_for_rebuild,
                    &sidecar_manifest_for_rebuild,
                    &cargo_target_for_rebuild,
                );
                eprintln!(
                    "[aas-host] respawn applied in {}ms (after hot-patch failure)",
                    t_total.elapsed().as_millis()
                );
            } else {
                eprintln!(
                    "[aas-host] hot-patch applied in {}ms",
                    t_total.elapsed().as_millis()
                );
            }
        }),
    );

    let port_mirror: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));

    if let Ok(path) = std::env::var("IDEALYST_AAS_PORT_FILE") {
        let port_for_file = port_mirror.clone();
        std::thread::spawn(move || {
            for _ in 0..200 {
                if let Ok(g) = port_for_file.lock() {
                    if let Some(p) = *g {
                        if let Err(e) = std::fs::write(&path, p.to_string()) {
                            eprintln!(
                                "[aas-host] could not write port sentinel {}: {}",
                                path, e
                            );
                        } else {
                            eprintln!("[aas-host] wrote bound port {} to {}", p, path);
                        }
                        return;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            eprintln!(
                "[aas-host] timed out waiting for serve to bind; no port sentinel written"
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
    // `crates/build/aas/src/hotpatch/stub.rs` catches the most
    // common crash class (Rust-internal monomorphization deferrals)
    // up-front, routing through the existing `try_hotpatch` →
    // `respawn_sidecar` fallback — which DOES coordinate mirror
    // state because it runs synchronously through the watch loop.
    //
    // For other crash modes (e.g. `_sin`/`_cos`-only deferrals that
    // still SIGSEGV the rerender path on some incremental-build
    // states), the recovery is currently: Ctrl-C + restart
    // `idealyst dev --aas`. Or set `IDEALYST_AAS_NO_HOTPATCH=1` to
    // force every edit through the respawn path.

    let session_mode = SessionMode::from_env();
    eprintln!(
        "[aas-host] starting (advertising app_id={} via mDNS, session mode = {:?})",
        app_id, session_mode,
    );
    serve_with_sidecar_and_tracker(
        bind_addr,
        recorder,
        &app_id,
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
        "[aas-host] replaying {} session(s) to fresh sidecar",
        sessions.len(),
    );
    for (s, viewport) in sessions {
        sidecar.send(SidecarIn::CreateSession { session: s, viewport });
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
    let aslr = {
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
        v
    };
    let table = builder.build(user_crate, aslr)?;
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
) {
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
        ])
        .arg(sidecar_manifest)
        .arg("--target-dir")
        .arg(cargo_target)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!(
                "[aas-host] respawn cargo build exited with {s} — sidecar unchanged"
            );
            return;
        }
        Err(e) => {
            eprintln!("[aas-host] respawn cargo build spawn failed: {e}");
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
                eprintln!("[aas-host] sidecar respawned");
            }
            Err(e) => eprintln!("[aas-host] sidecar respawn failed: {e}"),
        }
    }
    replay_sessions_to_sidecar(sidecar_slot, tracker);
}

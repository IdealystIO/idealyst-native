//! Subsecond-driven hot-patch builder for the AAS sidecar.
//!
//! Lives next to the sidecar build code because every step is
//! orchestrated by the same host binary that owns the file watcher
//! and the sidecar pipe. The runtime side — `subsecond::apply_patch`
//! and `framework_hot::call` — already lives in `framework-hot`
//! and is exercised by the `#[component]` macro's split form.
//!
//! # Pipeline
//!
//! On initial fat build:
//!  1. Cargo builds the sidecar with [`crate::hotpatch::fat_build_env`]
//!     wired in. This sets `RUSTC_WORKSPACE_WRAPPER` to the running
//!     `idealyst` binary and `IDEALYST_RUSTC_CAPTURE_DIR` so each
//!     workspace member's rustc invocation is captured to disk.
//!  2. `RUSTFLAGS` is augmented with `-Csave-temps=true
//!     -Clink-dead-code` so the .rcgu.o files survive past link
//!     and every symbol stays in the bin's text section. Both flags
//!     are dx's idea — we lifted them as-is.
//!  3. After link succeeds, [`HostBinCache::load`] parses the
//!     sidecar bin once and stashes a symbol map + `__thread_data`
//!     + `$tlv$init` size table for the per-patch loop.
//!
//! On every source change:
//!  4. [`HostBinCache`] + a runtime `_main` address (the sidecar
//!     reports its `dlsym("main")` on its `Hello` frame) feed
//!     [`HotPatchBuilder::build`].
//!  5. [`replay::run_rustc_emit_obj`] replays the captured rustc
//!     invocation for the sidecar bin with `--emit=obj`. Output:
//!     one or more `.rcgu.o` files containing the tip's fresh code.
//!  6. [`stub::synthesize`] walks the .rcgu.o for undefined refs,
//!     resolves each against the cache, and writes a Mach-O object
//!     containing aarch64 trampolines (and, when needed, Mach-O
//!     `__thread_vars` TLV descriptors copying `__thread_data` from
//!     the host bin — see [`stub::synthesize`] for the TLS branch).
//!  7. [`link::link_dylib`] invokes `cc -dynamiclib -Wl,-dylib` over
//!     `tip.rcgu.o + stub.o → patch.dylib`. No rlib inputs, no
//!     framework-crate dylib deps; the stub satisfies everything
//!     that isn't a libSystem lazy bind.
//!  8. [`jumptable::build`] reads the patch's symbol table, pairs
//!     every `__*_hot_impl` symbol (the macro's split inner fns) +
//!     `main` between cache and patch, and produces a
//!     `subsecond_types::JumpTable`. The host then ships it to the
//!     sidecar over the existing host↔sidecar pipe; the sidecar
//!     calls `framework_hot::apply_patch`.
//!
//! Any step failing returns an `anyhow::Error`; the host's
//! generated main translates that into the existing sidecar
//! respawn (the production-stable fallback that always works).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

mod cache;
mod jumptable;
mod link;
mod replay;
mod stub;

pub use cache::HostBinCache;
pub use replay::CapturedInvocation;
pub use subsecond_types::JumpTable;

/// Per-rebuild patch builder. Created once after the initial fat
/// build; reused for the lifetime of the dev session.
pub struct HotPatchBuilder {
    /// Where `<crate>.<crate-type>.json` capture files live.
    captures_dir: PathBuf,
    /// The sidecar bin's symbol cache (parsed once).
    host_cache: HostBinCache,
    /// Where to drop the per-edit patch dylib. Each apply uses a
    /// unique filename to defeat dyld's path-keyed dlopen cache.
    target_dir: PathBuf,
    /// Sequence counter that suffixes `libpatch-N.dylib` per apply.
    seq: std::sync::atomic::AtomicU64,
}

impl HotPatchBuilder {
    pub fn new(
        captures_dir: PathBuf,
        host_bin: &Path,
        target_dir: PathBuf,
    ) -> Result<Self> {
        let host_cache = HostBinCache::load(host_bin)
            .with_context(|| format!("parsing host bin {}", host_bin.display()))?;
        std::fs::create_dir_all(&target_dir)
            .with_context(|| format!("create target dir {}", target_dir.display()))?;
        Ok(Self {
            captures_dir,
            host_cache,
            target_dir,
            seq: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Re-run the captured rustc invocation for `user_crate` with
    /// `--emit=obj`, synthesize the stub, link the dylib, and
    /// return the result + JumpTable.
    ///
    /// `user_crate` is the name of the rlib whose source has
    /// changed — typically the project's library crate
    /// (`docs`, `hello-world`, etc.). The patch dylib contains
    /// only that crate's freshly-emitted code; calls into the
    /// framework + libc are routed through the stub trampolines
    /// back to the running sidecar bin's text.
    ///
    /// `runtime_main_addr` is the running sidecar's `dlsym
    /// ("main")` value (cached by the host from the sidecar's
    /// `Hello` frame).
    pub fn build(&self, user_crate: &str, runtime_main_addr: u64) -> Result<HotPatchArtifact> {
        let t_total = std::time::Instant::now();

        let t_replay = std::time::Instant::now();
        let capture = replay::find_capture(&self.captures_dir, user_crate)
            .with_context(|| format!("loading capture for {}", user_crate))?;
        let objs = replay::run_rustc_emit_obj(&capture)
            .context("replaying rustc with --emit=obj")?;
        let replay_ms = t_replay.elapsed().as_millis();
        if objs.is_empty() {
            anyhow::bail!("rustc --emit=obj produced no object files");
        }

        let t_stub = std::time::Instant::now();
        let stub_obj = self.target_dir.join("patch.stub.o");
        let stub_s = self.target_dir.join("patch.stub.s");
        stub::synthesize(
            &objs,
            &self.host_cache,
            runtime_main_addr,
            &stub_s,
            &stub_obj,
        )
        .context("synthesizing stub object")?;
        let stub_ms = t_stub.elapsed().as_millis();

        let t_link = std::time::Instant::now();
        let seq = self
            .seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let out_dylib = self.target_dir.join(format!("libpatch-{}.dylib", seq));
        link::link_dylib(&objs, &stub_obj, &out_dylib)
            .context("linking patch dylib")?;
        let link_ms = t_link.elapsed().as_millis();

        let t_jt = std::time::Instant::now();
        let table = jumptable::build(&out_dylib, &self.host_cache, runtime_main_addr)
            .context("building jump table")?;
        let jt_ms = t_jt.elapsed().as_millis();

        eprintln!(
            "[hotpatch] timing: rustc {}ms stub {}ms link {}ms jt {}ms (total {}ms)",
            replay_ms,
            stub_ms,
            link_ms,
            jt_ms,
            t_total.elapsed().as_millis(),
        );

        Ok(HotPatchArtifact {
            dylib: out_dylib,
            table,
        })
    }
}

pub struct HotPatchArtifact {
    pub dylib: PathBuf,
    pub table: JumpTable,
}

/// Compute the env vars cargo needs for the initial fat build.
/// Caller adds these to the cargo invocation. `idealyst_bin` is
/// the running CLI binary's absolute path — set as
/// `RUSTC_WRAPPER` so cargo runs every per-crate rustc through
/// our capture handler.
///
/// Note we use `RUSTC_WRAPPER` rather than
/// `RUSTC_WORKSPACE_WRAPPER`: the user crate lives outside the
/// sidecar's generated workspace as a path dep, so the
/// workspace-only variant skips it. Wrapping all rustc
/// invocations is cheap (one extra fork per crate) and gives us
/// captures for every dep, which is what the per-edit replay
/// needs.
pub fn fat_build_env(idealyst_bin: &Path, captures_dir: &Path) -> Vec<(String, String)> {
    let extra_rustflags = "-Csave-temps=true -Clink-dead-code";
    let existing = std::env::var("RUSTFLAGS").unwrap_or_default();
    let combined = if existing.is_empty() {
        extra_rustflags.to_string()
    } else {
        format!("{} {}", existing, extra_rustflags)
    };
    vec![
        (
            "RUSTC_WRAPPER".into(),
            idealyst_bin.display().to_string(),
        ),
        (
            "IDEALYST_RUSTC_CAPTURE_DIR".into(),
            captures_dir.display().to_string(),
        ),
        // Discriminator the CLI's main() looks for to enter the
        // wrapper-mode dispatch instead of clap-parsing argv.
        ("IDEALYST_RUSTC_WRAPPER_ACTIVE".into(), "1".into()),
        ("RUSTFLAGS".into(), combined),
    ]
}

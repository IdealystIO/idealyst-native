//! `idealyst worker` — run the project's `jobs` queue worker as a dedicated
//! process. The standalone counterpart to `idealyst run server`; the same
//! worker `idealyst dev` auto-spawns, but run on its own so you can scale it
//! independently (12-factor style).
//!
//! Resolves the worker bin the same way `run server` resolves the server:
//! - `worker_manifest` set → `cargo run --manifest-path <it> [--bin <worker_bin>]`
//! - else `worker_bin` set → `cargo run -p <pkg> --bin <it> --features server`
//!
//! The queue backend comes from `dev.toml`'s `[jobs]` block (or the app's own
//! defaults); this command forwards it as `IDEALYST_JOBS_BACKEND` / `_URL` so a
//! worker `main` calling `jobs::configure_from_env()` picks it up.

use std::path::PathBuf;

use anyhow::Context;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory (defaults to the current directory).
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// Build the worker in release mode.
    #[arg(long)]
    pub release: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let manifest = build_ios::parse_manifest(&args.dir)
        .with_context(|| format!("read manifest for {}", args.dir.display()))?;
    let app = &manifest.app;

    if app.worker_manifest.is_none() && app.worker_bin.is_none() {
        anyhow::bail!(
            "no worker declared for this project. Add one to {}/Cargo.toml:\n\n\
             \x20 [package.metadata.idealyst.app]\n\
             \x20 worker_bin = \"worker\"   # → src/bin/worker.rs, built --features server\n\n\
             \x20 # …or a standalone worker workspace:\n\
             \x20 worker_manifest = \"../../worker/Cargo.toml\"\n\
             \x20 worker_bin      = \"worker\"   # optional: names the bin\n\n\
             then re-run `idealyst worker`.",
            args.dir.display(),
        );
    }

    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("run");
    let how: String;
    if let Some(rel) = &app.worker_manifest {
        let joined = args.dir.join(rel);
        if !joined.is_file() {
            anyhow::bail!(
                "worker_manifest points at {}, which doesn't exist. The path is resolved \
                 relative to the app crate dir ({}); fix \
                 `[package.metadata.idealyst.app].worker_manifest`.",
                joined.display(),
                args.dir.display(),
            );
        }
        let manifest_path = std::fs::canonicalize(&joined).unwrap_or(joined);
        cmd.arg("--manifest-path").arg(&manifest_path);
        if let Some(bin) = &app.worker_bin {
            cmd.arg("--bin").arg(bin);
        }
        how = format!(
            "--manifest-path {}{}",
            manifest_path.display(),
            app.worker_bin
                .as_ref()
                .map(|b| format!(" --bin {b}"))
                .unwrap_or_default(),
        );
    } else {
        let bin = app.worker_bin.as_ref().expect("checked above");
        cmd.arg("-p")
            .arg(&manifest.name)
            .arg("--bin")
            .arg(bin)
            .arg("--features")
            .arg("server");
        how = format!("-p {} --bin {} --features server", manifest.name, bin);
    }
    if args.release {
        cmd.arg("--release");
    }

    // Forward the local queue + pubsub backends so the worker connects to the
    // same brokers the server uses.
    if let Ok(cfg) = crate::dev_config::DevConfig::load(&args.dir) {
        if let Some(jobs) = &cfg.jobs {
            if let Some(b) = &jobs.backend {
                cmd.env("IDEALYST_JOBS_BACKEND", b);
            }
            if let Some(u) = &jobs.url {
                cmd.env("IDEALYST_JOBS_URL", u);
            }
        }
        if let Some(ps) = &cfg.pubsub {
            if let Some(b) = &ps.backend {
                cmd.env("IDEALYST_PUBSUB_BACKEND", b);
            }
            if let Some(u) = &ps.url {
                cmd.env("IDEALYST_PUBSUB_URL", u);
            }
        }
    }

    eprintln!("[idealyst worker] cargo run {how}");
    let status = cmd
        .status()
        .with_context(|| format!("spawn `cargo run {how}`"))?;
    if !status.success() {
        anyhow::bail!(
            "worker exited with {}",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into()),
        );
    }
    Ok(())
}

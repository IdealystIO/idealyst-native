//! `idealyst-cli` — the `idealyst` CLI itself, installed in the dev
//! container. An idealyst project's devcontainer is half-useless without it
//! (`idealyst dev`, `idealyst lint`, `idealyst configure`, …).
//!
//! The CLI is a source build from the framework's registry (`cargo install
//! idealyst-cli --index …`; no prebuilt binaries — see README "Installing"),
//! which takes several minutes cold. To avoid paying that on every container
//! rebuild, the install root lives in a named volume and the post-create
//! step skips the build when the binary is already there; a
//! `/usr/local/bin` symlink (refreshed each create) puts it on PATH
//! regardless of the image's profile. To move a cached CLI to the newest
//! release, run `idealyst update` inside the container (it reinstalls into
//! the same root), or drop the volume.
//!
//! Requires a Rust toolchain in the base image — true for the scaffolded
//! `mcr.microsoft.com/devcontainers/rust` base.

use crate::devcontainer::service::{Ctx, DevService, ServiceFragment};

/// Fixed install root (`cargo install --root`); the binary lands at
/// `<ROOT>/bin/idealyst`. Backed by [`VOLUME`] so it survives rebuilds.
const ROOT: &str = "/idealyst/cli";

const VOLUME: &str = "idealyst-cli-cache";

/// The framework's own registry, named to `cargo install` with `--index`.
///
/// Installing the PUBLISHED crate needs nothing else: a published manifest
/// records each dependency's registry by URL. (The old `cargo install --git`
/// form needed `CARGO_REGISTRIES_IDEALYST_INDEX` in the environment, because
/// a git checkout's manifests name the registry only as `"idealyst"` and
/// `cargo install` reads no project `.cargo/config.toml`.) The same index
/// the CLI's `framework_source` defaults to.
const REGISTRY_INDEX: &str = "sparse+https://crates.idealyst.io/index/";

pub struct IdealystCli;

impl DevService for IdealystCli {
    fn id(&self) -> &'static str {
        "idealyst-cli"
    }
    fn label(&self) -> &'static str {
        "Idealyst CLI"
    }
    fn description(&self) -> &'static str {
        "the `idealyst` CLI in the container (built from the registry, cached in a volume across rebuilds)"
    }

    fn fragment(&self, _variant: Option<&str>, _ctx: &Ctx) -> ServiceFragment {
        let install = format!(
            "{chown}; test -x {ROOT}/bin/idealyst || \
             cargo install idealyst-cli --index {REGISTRY_INDEX} --locked --root {ROOT}; \
             sudo -n ln -sf {ROOT}/bin/idealyst /usr/local/bin/idealyst 2>/dev/null || \
             ln -sf {ROOT}/bin/idealyst /usr/local/bin/idealyst 2>/dev/null || true",
            chown = super::chown_cmd(ROOT),
        );
        ServiceFragment {
            app_volumes: vec![format!("{VOLUME}:{ROOT}")],
            volumes: vec![VOLUME.into()],
            post_create: vec![("idealyst-cli-install".into(), install)],
            ..Default::default()
        }
    }
}

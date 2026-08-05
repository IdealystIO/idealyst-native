//! `idealyst-cli` — the `idealyst` CLI itself, installed in the dev
//! container. An idealyst project's devcontainer is half-useless without it
//! (`idealyst dev`, `idealyst lint`, `idealyst configure`, …).
//!
//! The only distribution channel today is a source build
//! (`cargo install --git`; no crates.io release, no prebuilt binaries — see
//! README "Installing"), which takes several minutes cold. To avoid paying
//! that on every container rebuild, the install root lives in a named volume
//! and the post-create step skips the build when the binary is already
//! there; a `/usr/local/bin` symlink (refreshed each create) puts it on PATH
//! regardless of the image's profile. To force a rebuild of a cached CLI,
//! `cargo install --force --git … --root /idealyst/cli` inside the
//! container (or drop the volume).
//!
//! Requires a Rust toolchain + git in the base image — true for the
//! scaffolded `mcr.microsoft.com/devcontainers/rust` base.

use crate::devcontainer::service::{Ctx, DevService, ServiceFragment};

/// Fixed install root (`cargo install --root`); the binary lands at
/// `<ROOT>/bin/idealyst`. Backed by [`VOLUME`] so it survives rebuilds.
const ROOT: &str = "/idealyst/cli";

const VOLUME: &str = "idealyst-cli-cache";

const REPO: &str = "https://github.com/IdealystIO/idealyst-native";

pub struct IdealystCli;

impl DevService for IdealystCli {
    fn id(&self) -> &'static str {
        "idealyst-cli"
    }
    fn label(&self) -> &'static str {
        "Idealyst CLI"
    }
    fn description(&self) -> &'static str {
        "the `idealyst` CLI in the container (source build, cached in a volume across rebuilds)"
    }

    fn fragment(&self, _variant: Option<&str>, _ctx: &Ctx) -> ServiceFragment {
        let install = format!(
            "{chown}; test -x {ROOT}/bin/idealyst || \
             cargo install --git {REPO} idealyst-cli --root {ROOT}; \
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

//! `codex` — OpenAI's Codex CLI installed in the dev container.
//!
//! OpenAI publishes no official devcontainer feature (openai/codex#19817), so
//! rather than depend on a third-party one we install through official
//! infrastructure only: the devcontainers `node` feature plus a keyed
//! `postCreateCommand` entry running `npm install -g @openai/codex`.
//!
//! Variants mirror `claude`: `host` bind-mounts the host's `~/.codex`
//! (`auth.json` and all — an existing host login carries over), `volume`
//! keeps an isolated login in a named volume. `CODEX_HOME` relocates the
//! CLI's config dir to the fixed mount target, home-dir-independent.

use crate::devcontainer::service::{Ctx, DevService, ServiceFragment, ServiceVariant};

/// Official devcontainers Node.js feature — Codex installs from npm.
const NODE_FEATURE: &str = "ghcr.io/devcontainers/features/node:1";

/// Fixed in-container config dir; `CODEX_HOME` targets it.
const CONFIG_DIR: &str = "/idealyst/agents/codex";

const VOLUME: &str = "idealyst-codex-config";

/// Global npm installs land in a user-writable prefix on the node feature's
/// default setup; the sudo fallback covers images where they don't (`-n`
/// fails fast instead of prompting when sudo isn't passwordless).
const INSTALL: &str = "npm install -g @openai/codex || sudo -n npm install -g @openai/codex";

const VARIANTS: &[ServiceVariant] = &[
    ServiceVariant { id: "host", label: "Share host login (bind-mount ~/.codex)" },
    ServiceVariant { id: "volume", label: "Isolated login (named volume, survives rebuilds)" },
];

pub struct Codex;

impl DevService for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn label(&self) -> &'static str {
        "Codex"
    }
    fn description(&self) -> &'static str {
        "OpenAI Codex CLI in the container — `host` shares your host login, `volume` keeps an isolated one"
    }
    fn variants(&self) -> &'static [ServiceVariant] {
        VARIANTS
    }

    fn fragment(&self, variant: Option<&str>, _ctx: &Ctx) -> ServiceFragment {
        let isolated = variant == Some("volume");
        let mut frag = ServiceFragment {
            app_env: vec![("CODEX_HOME".into(), CONFIG_DIR.into())],
            features: vec![(NODE_FEATURE.into(), serde_json::json!({}))],
            post_create: vec![("idealyst-codex-install".into(), INSTALL.into())],
            ..Default::default()
        };
        if isolated {
            frag.app_volumes = vec![format!("{VOLUME}:{CONFIG_DIR}")];
            frag.volumes = vec![VOLUME.into()];
            frag.post_create
                .push(("idealyst-codex-chown".into(), super::chown_cmd(CONFIG_DIR)));
        } else {
            frag.app_volumes = vec![format!("~/.codex:{CONFIG_DIR}")];
        }
        frag
    }
}

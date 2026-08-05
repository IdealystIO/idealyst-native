//! `claude` — Claude Code (Anthropic's coding agent CLI) installed in the
//! dev container via the **native installer** (`claude.ai/install.sh`).
//!
//! Not Anthropic's devcontainer feature: that feature still installs the
//! deprecated npm package (`@anthropic-ai/claude-code`; npm distribution was
//! deprecated in v2.1.15, 2026-01) and its Node auto-install fails on images
//! like `mcr.microsoft.com/devcontainers/rust:1`. The native installer is the
//! path Anthropic actually recommends, needs no Node.js, and self-updates.
//! It lands in `~/.local/bin`, which not every image's PATH includes, so the
//! install command also drops a `/usr/local/bin` symlink (best-effort).
//!
//! Variants pick where the CLI's config directory (credentials, settings,
//! history — everything) lives:
//!
//! - `host` (default): bind-mounts the host's `~/.claude` into the container,
//!   so an existing host login carries over with no extra step. On hosts that
//!   keep the OAuth token in the OS keychain instead of
//!   `~/.claude/.credentials.json` (macOS), the first `claude` run in the
//!   container asks for login once and then writes the credential into the
//!   mounted dir — automatic from then on, shared with the host dir.
//! - `volume`: a named Docker volume — nothing shared with the host; log in
//!   once per project and the login survives container rebuilds.
//!
//! The mount target is a fixed absolute path (not `~/.claude`) because a
//! compose mount target can't reference the container user's home, which
//! varies by base image (`vscode`, `node`, `root`, …). `CLAUDE_CONFIG_DIR`
//! points the CLI at it regardless of user — it relocates the *entire* config
//! dir, `.credentials.json` included.

use crate::devcontainer::service::{Ctx, DevService, ServiceFragment, ServiceVariant};

/// Fixed in-container config dir; `CLAUDE_CONFIG_DIR` targets it.
const CONFIG_DIR: &str = "/idealyst/agents/claude";

const VOLUME: &str = "idealyst-claude-config";

/// Native install, then a best-effort `/usr/local/bin` symlink so `claude` is
/// on PATH even when the image's profile doesn't add `~/.local/bin`.
const INSTALL: &str = "curl -fsSL https://claude.ai/install.sh | bash && \
                       { sudo -n ln -sf \"$HOME/.local/bin/claude\" /usr/local/bin/claude 2>/dev/null \
                         || ln -sf \"$HOME/.local/bin/claude\" /usr/local/bin/claude 2>/dev/null \
                         || true; }";

const VARIANTS: &[ServiceVariant] = &[
    ServiceVariant { id: "host", label: "Share host login (bind-mount ~/.claude)" },
    ServiceVariant { id: "volume", label: "Isolated login (named volume, survives rebuilds)" },
];

pub struct Claude;

impl DevService for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn label(&self) -> &'static str {
        "Claude Code"
    }
    fn description(&self) -> &'static str {
        "Claude Code CLI in the container — `host` shares your host login, `volume` keeps an isolated one"
    }
    fn variants(&self) -> &'static [ServiceVariant] {
        VARIANTS
    }

    fn fragment(&self, variant: Option<&str>, _ctx: &Ctx) -> ServiceFragment {
        let isolated = variant == Some("volume");
        let mut frag = ServiceFragment {
            app_env: vec![("CLAUDE_CONFIG_DIR".into(), CONFIG_DIR.into())],
            post_create: vec![("idealyst-claude-install".into(), INSTALL.into())],
            ..Default::default()
        };
        if isolated {
            frag.app_volumes = vec![format!("{VOLUME}:{CONFIG_DIR}")];
            frag.volumes = vec![VOLUME.into()];
            // A fresh named volume is mounted root-owned; hand it to the
            // container user so the CLI can write credentials into it.
            frag.post_create
                .push(("idealyst-claude-chown".into(), super::chown_cmd(CONFIG_DIR)));
        } else {
            frag.app_volumes = vec![format!("~/.claude:{CONFIG_DIR}")];
        }
        frag
    }
}

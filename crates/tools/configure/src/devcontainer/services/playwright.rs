//! `playwright` — the official Playwright MCP server (with its bundled
//! headless Chromium) as a sidecar, so AI agents in the dev container can
//! drive a real browser over MCP without Chromium ever being installed in the
//! dev image.
//!
//! Uses Microsoft's `mcr.microsoft.com/playwright/mcp` image. Its entrypoint
//! is `node /app/cli.js --headless --browser chromium --no-sandbox`
//! (inspected 2026-08 — the image only ships headless Chromium), and compose
//! `command:` args *append* to the entrypoint, so we pass only the transport
//! flags and inherit the browser setup. The image publishes no stable major
//! tag (only fast-moving `v0.0.x` patch tags), so like `minio/minio` it is
//! used untagged rather than pinned to a patch that would go stale.
//!
//! The MCP endpoint is streamable HTTP at `/mcp` (legacy SSE at `/sse`);
//! `PLAYWRIGHT_MCP_URL` on the app service points at it. Inside the dev
//! container, Claude Code hooks it up with
//! `claude mcp add --transport http playwright "$PLAYWRIGHT_MCP_URL"`.

use crate::devcontainer::service::{Ctx, DevService, ServiceFragment};

/// Server port on the compose network (no host port is published). Upstream's
/// documented example port for the HTTP transport; any fixed value works, it
/// just has to match the URL below and the `--allowed-hosts` entry.
const PORT: u16 = 8931;

pub struct Playwright;

impl DevService for Playwright {
    fn id(&self) -> &'static str {
        "playwright"
    }
    fn label(&self) -> &'static str {
        "Playwright MCP"
    }
    fn description(&self) -> &'static str {
        "Playwright MCP server + headless Chromium sidecar — exposes PLAYWRIGHT_MCP_URL for AI agents"
    }

    fn fragment(&self, _variant: Option<&str>, _ctx: &Ctx) -> ServiceFragment {
        // - `--host 0.0.0.0`: the server binds localhost by default, which is
        //   unreachable from the dev container.
        // - `--allowed-hosts playwright:{PORT}`: the server 403s any request
        //   whose Host header isn't allowlisted (DNS-rebinding guard), and the
        //   default allowlist is the *bind* host — which never matches the
        //   compose service name the dev container dials. Without this flag
        //   every request gets "Access is only allowed at localhost:8931".
        // - `init: true`: upstream runs the image with `docker run --init`;
        //   Chromium spawns subprocesses and PID 1 must reap them.
        // - `shm_size`: the launched Chromium does NOT pass
        //   `--disable-dev-shm-usage` (verified against the image), so with
        //   Docker's default 64 MB /dev/shm heavy pages crash the tab. A real
        //   shm keeps the fix container-local (vs. upstream's `--ipc=host`
        //   alternative, which shares the host IPC namespace).
        let service = format!(
            r#"
image: mcr.microsoft.com/playwright/mcp
restart: unless-stopped
init: true
shm_size: 2gb
command:
  - --host
  - 0.0.0.0
  - --port
  - "{PORT}"
  - --allowed-hosts
  - playwright:{PORT}
"#
        );
        ServiceFragment {
            service: Some(serde_yaml::from_str(&service).expect("valid playwright service yaml")),
            app_env: vec![(
                "PLAYWRIGHT_MCP_URL".into(),
                format!("http://playwright:{PORT}/mcp"),
            )],
            ..Default::default()
        }
    }
}

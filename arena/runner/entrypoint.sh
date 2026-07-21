#!/usr/bin/env bash
# Arena runner entrypoint: billing guard → credential seed → repo-drift
# warning → egress firewall → hand off to CMD (default: sleep infinity, the
# orchestrator drives everything via docker exec).
#
# Runs as `vscode` (passwordless sudo for the firewall). The home dir is an
# image layer, so every container starts with a CLEAN ~/.claude and seeds
# fresh credentials — the stale-volume-credentials failure class from the
# devcontainer cannot occur here.
set -euo pipefail

# --- Billing guard: subscription only, never usage-based API. ----------------
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  echo "ERROR: ANTHROPIC_API_KEY is set — arena runs must bill the subscription." >&2
  echo "Remove it from the container environment." >&2
  exit 1
fi
# A set-but-empty token var makes Claude Code fail login instead of using the
# credentials file (hit live in the devcontainer).
[ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ] || unset CLAUDE_CODE_OAUTH_TOKEN

# --- Seed subscription credentials from the read-only host mount. ------------
mkdir -p "$HOME/.claude"
if [ -f /host-claude/.credentials.json ]; then
  cp /host-claude/.credentials.json "$HOME/.claude/.credentials.json"
  chmod 600 "$HOME/.claude/.credentials.json"
  echo "auth: subscription credentials seeded (no login needed)"
elif [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]; then
  echo "auth: using CLAUDE_CODE_OAUTH_TOKEN"
else
  echo "auth: NO credentials — mount \$HOME/.claude at /host-claude:ro or set CLAUDE_CODE_OAUTH_TOKEN" >&2
fi
# `{}` not `touch`: claude treats an EMPTY .claude.json as corrupted JSON
# ("Unexpected EOF") and aborts headless runs (hit live on run-3).
[ -s "$HOME/.claude/.claude.json" ] || echo '{}' > "$HOME/.claude/.claude.json"
ln -sfn "$HOME/.claude/.claude.json" "$HOME/.claude.json"

# --- Repo drift check: the baked CLI must match the mounted framework. -------
REPO=/workspaces/idealyst-native
if [ -d "$REPO/.git" ] && command -v git >/dev/null; then
  head=$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)
  if [ "$head" != "${IDEALYST_BUILD_COMMIT:-unknown}" ]; then
    echo "WARN: mounted repo @ $head but image was baked @ ${IDEALYST_BUILD_COMMIT:-unknown} — rebuild the runner image after framework changes" >&2
  fi
fi

# --- Target overlay ownership: a fresh named volume mounts root-owned; the
# build user must own it or every cargo build dies on Permission denied.
if mountpoint -q "$REPO/target" 2>/dev/null; then
  sudo chown vscode:vscode "$REPO/target"
fi

# --- Egress firewall: allowlist-only (Anthropic + crates.io). ----------------
sudo bash /init-firewall.sh

exec "$@"

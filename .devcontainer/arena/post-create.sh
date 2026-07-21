#!/usr/bin/env bash
# Arena container provisioning. Runs ONCE on create, BEFORE the first
# init-firewall.sh (postStart), so everything that needs open egress —
# npm, rustup, wasm-bindgen download, crates.io — happens here. After this,
# runtime egress is Anthropic + crates.io only.
set -euo pipefail

REPO=/workspaces/idealyst-native

# --- Claude auth plumbing (same dance as the base post-create: keep
# .claude.json inside the persisted claude-home volume). -----------------------
sudo chown -R vscode:vscode /home/vscode/.claude

# Seed credentials from the host's read-only mount (Linux hosts) so a fresh
# claude-home volume needs no interactive login. Never overwrites: once the
# volume has credentials, the container's copy is authoritative — OAuth
# refresh rotates tokens, and clobbering a rotated token with the host's
# stale copy would log the container out.
# Seed when the volume has no credentials OR only EXPIRED ones (the shared
# claude-home volume can carry a long-dead login from an older container —
# hit live 2026-07-20: a June-era token 401'd every headless run). A LIVE
# credential is never overwritten: OAuth refresh rotates tokens, and
# clobbering a rotated token with the host's copy would log the container out.
seed=no
if [ -f /host-claude/.credentials.json ]; then
  if [ ! -f /home/vscode/.claude/.credentials.json ]; then
    seed=yes
  else
    expires=$(jq -r '.claudeAiOauth.expiresAt // 0' /home/vscode/.claude/.credentials.json 2>/dev/null || echo 0)
    now_ms=$(($(date +%s) * 1000))
    [ "${expires:-0}" -lt "$now_ms" ] && seed=yes
  fi
fi
if [ "$seed" = yes ]; then
  cp /host-claude/.credentials.json /home/vscode/.claude/.credentials.json
  chmod 600 /home/vscode/.claude/.credentials.json
  # Credentials ONLY — deliberately no settings.json/hooks/plugins from the
  # host: the in-container implementer must be a NAIVE agent (no user hooks,
  # no plugins, no skills), and host settings often reference host paths.
  echo "seeded Claude credentials from host"
fi

if [ ! -e /home/vscode/.claude/.claude.json ]; then
  if [ -f /home/vscode/.claude.json ] && [ ! -L /home/vscode/.claude.json ]; then
    mv /home/vscode/.claude.json /home/vscode/.claude/.claude.json
  else
    touch /home/vscode/.claude/.claude.json
  fi
fi
ln -sfn /home/vscode/.claude/.claude.json /home/vscode/.claude.json

# --- Billing guard: this container must bill the SUBSCRIPTION, never the API.
# Subscription = OAuth (.credentials.json or CLAUDE_CODE_OAUTH_TOKEN);
# usage-based API = ANTHROPIC_API_KEY. Refuse to provision if an API key
# leaked into the container env — silently proceeding could route every
# arena run to pay-as-you-go.
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  echo "ERROR: ANTHROPIC_API_KEY is set inside the arena container — this would" >&2
  echo "bill arena runs as usage-based API instead of the subscription." >&2
  echo "Remove it from the container environment and re-create." >&2
  exit 1
fi
if [ -f /home/vscode/.claude/.credentials.json ] || [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]; then
  echo "auth: subscription credentials ready (no login needed)"
else
  echo "auth: NO credentials found — run \`claude\` once in the container to log in"
  echo "      (persists in the claude-home volume), or set CLAUDE_CODE_OAUTH_TOKEN."
fi

# Defensive: a set-but-empty CLAUDE_CODE_OAUTH_TOKEN (e.g. from an env
# passthrough of an unset host var) makes Claude Code fail login instead of
# using the credentials file. Strip it in every interactive shell.
guard='[ -n "$CLAUDE_CODE_OAUTH_TOKEN" ] || unset CLAUDE_CODE_OAUTH_TOKEN'
grep -qF "$guard" /home/vscode/.bashrc 2>/dev/null || echo "$guard" >> /home/vscode/.bashrc

npm install -g @anthropic-ai/claude-code

# --- Rust toolchain for the web target ---------------------------------------
rustup target add wasm32-unknown-unknown

# wasm-bindgen-cli MUST match the wasm-bindgen crate version in Cargo.lock.
# Prebuilt release first (seconds); cargo install as the slow fallback.
WBG_VERSION=$(grep -A1 'name = "wasm-bindgen"' "$REPO/Cargo.lock" | grep version | head -1 | cut -d'"' -f2)
if ! wasm-bindgen --version 2>/dev/null | grep -q "$WBG_VERSION"; then
  arch=$(uname -m)
  tarball="wasm-bindgen-${WBG_VERSION}-${arch}-unknown-linux-musl.tar.gz"
  url="https://github.com/rustwasm/wasm-bindgen/releases/download/${WBG_VERSION}/${tarball}"
  if curl -fsSL "$url" -o "/tmp/${tarball}"; then
    tar -xzf "/tmp/${tarball}" -C /tmp
    sudo install "/tmp/wasm-bindgen-${WBG_VERSION}-${arch}-unknown-linux-musl/wasm-bindgen" /usr/local/bin/
    rm -rf "/tmp/${tarball}" "/tmp/wasm-bindgen-${WBG_VERSION}-${arch}-unknown-linux-musl"
  else
    echo "prebuilt wasm-bindgen unavailable for ${arch}; falling back to cargo install (slow)"
    cargo install wasm-bindgen-cli --version "$WBG_VERSION" --locked
  fi
fi

# --- idealyst CLI from THIS checkout (scaffold/build/mcp all ride on it) -----
cargo install --path "$REPO/crates/tools/cli" --locked || \
  cargo install --path "$REPO/crates/tools/cli"

# --- Prebuild the arena spine binary so bench steps don't compile-on-first-use
(cd "$REPO/arena" && cargo build -p arena-spine --bin arena)

# --- Verify: doctor is the authoritative toolchain probe ---------------------
idealyst doctor || true
echo "arena post-create done — restart the container (or wait for postStart) to apply the egress firewall"

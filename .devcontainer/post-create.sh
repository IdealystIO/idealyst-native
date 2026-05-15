#!/usr/bin/env bash
set -euo pipefail

sudo chown -R vscode:vscode /home/vscode/.claude

if [ ! -e /home/vscode/.claude/.claude.json ]; then
  if [ -f /home/vscode/.claude.json ] && [ ! -L /home/vscode/.claude.json ]; then
    mv /home/vscode/.claude.json /home/vscode/.claude/.claude.json
  else
    touch /home/vscode/.claude/.claude.json
  fi
fi
ln -sfn /home/vscode/.claude/.claude.json /home/vscode/.claude.json

npm install -g @anthropic-ai/claude-code
rustup target add wasm32-unknown-unknown aarch64-apple-ios aarch64-linux-android
curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh

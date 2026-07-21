# Arena runner: standalone, disposable bench container.
#
# Unlike the arena devcontainer (`.devcontainer/arena/` — the HUMAN surface:
# interactive debugging, auth bootstrap), this image bakes the entire
# toolchain + binaries at build time so bench containers are cattle:
# `docker run --rm` per run, N in parallel (per-container network namespaces
# isolate relay ports and ~/.idealyst/apps registrations), nothing to
# provision, nothing to reset, no VS Code lifecycle to trip over.
#
# Build (from the repo root; BuildKit picks up Dockerfile.dockerignore):
#   docker build -f arena/runner/Dockerfile \
#     --build-arg COMMIT=$(git rev-parse --short HEAD) \
#     -t idealyst-arena-runner .
#
# Rebuild whenever the framework/CLI changes — the entrypoint warns when the
# mounted repo's HEAD differs from the baked COMMIT.
#
# Run (per bench run; see .claude/skills/arena-bench):
#   docker run -d --rm --init --name arena-run-<id> --label arena-run \
#     --cap-add NET_ADMIN --cap-add NET_RAW \
#     -v <repo>:/workspaces/idealyst-native:ro \
#     -v arena-target-<id>:/workspaces/idealyst-native/target \
#     -v <runs-dir>:/workspaces/arena-runs \
#     -v $HOME/.claude:/host-claude:ro \
#     idealyst-arena-runner
#
# The repo mounts READ-ONLY (the agent physically cannot modify framework
# source); builds write through the target-volume overlay mounted on top of
# <repo>/target. Sweep everything: docker rm -f $(docker ps -aq -f label=arena-run)

FROM mcr.microsoft.com/devcontainers/rust:1

# Same runtime tool set the arena devcontainer proved out.
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        chromium binaryen iptables dnsutils python3 jq \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

USER vscode
ENV PATH=/home/vscode/.local/bin:$PATH \
    ARENA_CHROME=/usr/bin/chromium \
    IDEALYST_BROWSER=/usr/bin/chromium

# Claude Code as the standalone binary (no node dependency in this image).
RUN curl -fsSL https://claude.ai/install.sh | bash

# Bake the idealyst CLI + arena spine from the build context (the repo).
# The registry cache this warms also serves the agent's own builds at runtime.
COPY --chown=vscode:vscode . /build/repo
RUN cargo install --path /build/repo/crates/tools/cli \
    && cd /build/repo/arena \
    && cargo build -p arena-spine --bin arena \
    && install target/debug/arena /usr/local/cargo/bin/arena \
    && rm -rf /build/repo

ARG COMMIT=unknown
ENV IDEALYST_BUILD_COMMIT=${COMMIT}
LABEL org.idealyst.arena.commit=${COMMIT}

COPY --chown=vscode:vscode arena/runner/entrypoint.sh /entrypoint.sh
COPY --chown=vscode:vscode .devcontainer/arena/init-firewall.sh /init-firewall.sh

ENTRYPOINT ["bash", "/entrypoint.sh"]
CMD ["sleep", "infinity"]

# Arena dev service image: the same rust devcontainer base as the daily
# driver, plus everything an arena run shells out to at runtime — baked at
# image build so the runtime firewall can stay allowlist-only.
#   chromium   — local headless browser for the robot-tier web host
#   binaryen   — wasm-opt (idealyst build --web production pipeline)
#   iptables/ipset/dnsutils — init-firewall.sh
#   python3    — serves the built web bundle during outcome-tier verification
FROM mcr.microsoft.com/devcontainers/rust:1

RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        chromium \
        binaryen \
        iptables \
        dnsutils \
        python3 \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

#!/usr/bin/env bash
# Allowlist-only egress for arena runs (adapted from the Claude Code
# reference devcontainer's init-firewall.sh). Everything not listed is
# DROPped — this is what enforces the arena's "the idealyst MCP is the only
# documentation source" invariant at the network layer instead of trusting
# tool-level restrictions alone.
#
# Allowed:
#   * Anthropic endpoints        — Claude Code itself (subscription billing,
#                                  telemetry, MCP connector proxy)
#   * crates.io (+index/static)  — scaffolded projects still fetch third-party
#                                  crates (serde etc.). Serves code, not docs;
#                                  reading docs via crates.io would show up as
#                                  a doc-bypass pathology in the transcript.
#   * loopback + RFC1918         — the relay, served bundles, and the compose
#                                  `chrome` sidecar all live on local nets
#   * DNS + established flows
# Deliberately ABSENT: github.com, npmjs.org, docs.rs, and the web at large.
set -euo pipefail

ALLOWED_DOMAINS=(
  api.anthropic.com
  # Auth-critical: subscription OAuth token refresh goes through Anthropic's
  # account endpoints. Without these, login only works while CDN edge IPs
  # happen to overlap api.anthropic.com's — don't rely on that.
  claude.ai
  console.anthropic.com
  platform.claude.com
  statsig.anthropic.com
  statsig.com
  sentry.io
  mcp-proxy.anthropic.com
  crates.io
  index.crates.io
  static.crates.io
)

# Plain per-IP iptables rules, no ipset: ipset's hash:ip needs ip_set kernel
# modules loaded on the HOST (containers share the host kernel), which fails
# on hosts without them. The allowlist is ~8 domains, so a rule per resolved
# IP is cheap and portable.
iptables -F OUTPUT

# Loopback + local container networks (relay, served bundle, chrome sidecar).
iptables -A OUTPUT -o lo -j ACCEPT
iptables -A OUTPUT -d 10.0.0.0/8 -j ACCEPT
iptables -A OUTPUT -d 172.16.0.0/12 -j ACCEPT
iptables -A OUTPUT -d 192.168.0.0/16 -j ACCEPT

# DNS + established flows.
iptables -A OUTPUT -p udp --dport 53 -j ACCEPT
iptables -A OUTPUT -p tcp --dport 53 -j ACCEPT
iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT

for domain in "${ALLOWED_DOMAINS[@]}"; do
  ips=$(dig +short A "$domain" | grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$' || true)
  if [ -z "$ips" ]; then
    echo "WARN: could not resolve $domain (skipping)" >&2
    continue
  fi
  while IFS= read -r ip; do
    iptables -A OUTPUT -d "$ip" -j ACCEPT
  done <<<"$ips"
done

iptables -P OUTPUT DROP

# Self-check: Anthropic reachable, the open web not.
if curl -fsS --max-time 10 https://api.anthropic.com >/dev/null 2>&1 \
   || [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 https://api.anthropic.com)" != "000" ]; then
  echo "firewall ok: api.anthropic.com reachable"
else
  echo "WARN: api.anthropic.com NOT reachable — Claude Code will not work" >&2
fi
if [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 https://example.com)" = "000" ]; then
  echo "firewall ok: open web blocked"
else
  echo "WARN: example.com reachable — firewall NOT enforcing" >&2
fi

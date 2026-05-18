---
name: wire-protocol
description: Wire variant additions stay in sync across SceneModel snapshot, dev-server, and dev-client.
targets:
  - crates/framework/wire
  - crates/framework/dev-client
  - crates/dev/server
  - crates/dev/http
  - crates/dev/reload
severity: high
---

# Wire protocol

## Background

Fresh AAS clients receive a `SceneModel` snapshot rather than the full
command log (see auto-memory `project_aas_state_snapshot`). That means
**every new wire variant must be reflected in SceneModel** for late-join
clients to see correct state. Wire is also the contract between
`dev-server` and `dev-client` — a backward-incompatible change must be
versioned or the dev loop silently desyncs.

## Checklist

- [ ] **New wire variants** — for every `enum` variant in `wire` that
      mutates scene state, verify there is corresponding state in
      `SceneModel` and snapshot/apply logic that round-trips it.
- [ ] **Snapshot completeness** — anything stored in client memory that
      affects rendering should be reconstructable from `SceneModel`. Flag
      client-only state that has no snapshot representation.
- [ ] **Serialization stability** — flag wire types that derive
      `Serialize`/`Deserialize` without an explicit tag or version. Adding
      a variant to an untagged enum is silently breaking.
- [ ] **Server↔client schema parity** — search `dev-server` for senders
      and `dev-client` for matching receivers; every sent variant should
      have a handler.
- [ ] **`graphics` placeholder** — confirm `create_graphics` in AAS mode
      stays a placeholder (see auto-memory `project_aas_graphics_unsupported`).
      Flag any code that would attempt real GPU work in AAS.
- [ ] **Backwards compat** — when a wire field is removed or renamed, flag
      it for a version bump or migration plan.

## Output format

Report findings as a Markdown list. For each finding include:

- **Severity**: low / medium / high
- **Location**: `crate/src/file.rs:line` (cross-reference both sides where
  relevant)
- **Issue**: one-line description
- **Why**: brief reasoning — call out which side(s) of the protocol diverge.
- **Suggested fix**: actionable recommendation, or "needs design discussion"

End with a one-line summary: `Result: N high, M medium, K low findings.`

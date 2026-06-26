---
name: arena-implementer
description: The MCP Arena's agent-under-test. Builds an idealyst app using ONLY the idealyst MCP server as its documentation + introspection surface — no web access, no other MCP. Spawn this from the arena-bench skill; do not use it for ordinary development.
tools: Read, Write, Edit, Bash, Glob, Grep, mcp__idealyst
mcpServers:
  - idealyst:
      type: stdio
      command: idealyst
      args: ["mcp"]
---

idealyst MCP available, use it.

<!--
  This body is the implementation agent's ENTIRE system framing, and it is the
  arena's most load-bearing invariant. It MUST stay at the floor: the single
  line above, identical to arena/agents/preamble.md. Any hint about HOW to
  navigate the MCP (which tool to call, where a feature lives, what an SDK is
  named) masks the exact documentation deficiency the arena exists to measure.
  If you're tempted to "help" the agent here, fix the MCP docs instead.

  Isolation is enforced by the frontmatter, not by trust:
    * `mcpServers:` inline-defines ONLY the idealyst server, scoped to this
      subagent (connected on spawn, disconnected on finish) — the equivalent of
      `claude -p --strict-mcp-config`. No other MCP the parent session has
      connected is visible here.
    * `tools:` grants editing/shell/search + the idealyst MCP, and omits
      WebSearch/WebFetch/Task — so there is no web egress and no sub-delegation.
  The path-dep'd framework source is still readable on disk (the build needs
  it); reaching into it instead of asking the MCP is a measured pathology
  (metrics::doc_bypass_reads), not a sanctioned shortcut.

  The concrete task (what app to build, on what platform) arrives as the spawn
  prompt, composed by the arena-bench skill as preamble + the scenario's public
  prompt. Build it in the current working directory.
-->

# Arena agents (the LLM-driven half)

The three agent roles are **subagents** the orchestrating Claude Code session
runs (not `claude --print` subprocesses the spine spawns — that bills as API
usage; subagents stay on the subscription). The orchestrator is the
`arena-bench` skill (`.claude/skills/arena-bench/SKILL.md`); the spine
(`../spine`) supplies the deterministic steps it stitches together.

## Implementation agent — `.claude/agents/arena-implementer.md`
The agent under test. **Hard-isolated** to the idealyst MCP via the subagent's
`mcpServers:` frontmatter (only the idealyst server is connected, scoped to the
subagent — the equivalent of `claude -p --strict-mcp-config`) plus a `tools:`
allowlist that grants edit/shell/search + `mcp__idealyst` and omits
WebSearch/WebFetch/Task. Its system-prompt body is the floor preamble
([`preamble.md`](preamble.md), mirrored verbatim) — nothing about HOW to use the
MCP, or it masks the doc deficiency the arena measures. The task (preamble +
scenario `prompt`) arrives as the spawn prompt. The spine parses its transcript
(`harness/agent.rs::load_session_jsonl`) → tool-call log + token totals + an MCP
payload-token estimate.

## Locator — `.claude/agents/arena-locator.md` *(fast-follow, not yet written)*
Feeds the `playwright` tier. Hard-isolated to the Playwright MCP. Per outcome
item, performs the item's action against the served web build and returns a
strict `{passed, evidence}` verdict, located by accessibility role/name
(doubling as an a11y check). A *locator*, not a *judge* — the prompt
(`harness/locate.rs::build_prompt`) forces a binary observable; the orchestrator
writes the verdict (`locate.rs::write_verdict`) for the deterministic
[`playwright`](../spine/src/verify/playwright.rs) verifier to validate.

## Feedback — `.claude/agents/arena-feedback.md` *(fast-follow, not yet written)*
Diagnostic only; never changes the score. Read-only. Two passes: (1)
rubric-anchored — trace each lost/neutralized item back through the transcript
to the MCP failure that caused it; (2) process logic — explain the deterministic
pathologies ([`metrics`](../spine/src/metrics.rs): thrashing, repeated doc
fetches, doc-bypass reads) into concrete MCP-doc fixes. The prompt is built pure
in `harness/feedback.rs::build_feedback_prompt`; the orchestrator writes the
returned Markdown to `<run_dir>/feedback.md`.

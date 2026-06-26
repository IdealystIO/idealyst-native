# Arena skills (LLM-driven half)

These are the agents the deterministic [`spine`](../spine) orchestrates. None
are implemented yet — this file records the intended contract so the spine's
verifier tiers have a target to call into.

## `arena-implement`
The isolated implementation agent under test. Receives the fixed preamble
(*"idealyst MCP available, use it"*) + the scenario's `prompt`. Tools: code
editing + the idealyst CLI + the idealyst MCP server (docs **and** Robot).
**Denied:** Playwright, web search, framework source, any other doc source.
Emits a transcript (tool calls + token counts) and a project tree.

## `arena-locate` (feeds the `playwright` tier)
Drives Playwright against the running web build to check one `outcome` item.
May use an LLM to *locate and act*, but **must** return a binary observable
(element with role/name present + in the expected state) plus evidence — never
a judgement. Locates by accessibility role/name, which doubles as an a11y
check on the produced UI.

## `arena-feedback` (two passes, does not affect score)
1. **Rubric-anchored:** for each lost item, trace the transcript to the
   navigation/comprehension failure that caused it.
2. **Process logic:** consume the deterministic pathology metrics
   (`spine::metrics`) — thrashing, repeated doc fetches, flailing — and
   cluster/explain them into concrete MCP-improvement suggestions.

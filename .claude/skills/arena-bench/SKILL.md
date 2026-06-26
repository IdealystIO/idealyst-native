---
name: arena-bench
description: Run one MCP Arena scenario — scaffold an isolated idealyst app, have the idealyst-MCP-only arena-implementer subagent build it, then build + score it on the deterministic spine. Use when asked to run the arena, bench a scenario, or evaluate the idealyst MCP docs. Subscription-billed (subagents), never `claude -p`.
---

# Run an MCP Arena scenario

You are the **orchestrator**. You scaffold, spawn the agent-under-test, build,
and score. **You never implement the app yourself** — all implementation is done
by the `arena-implementer` subagent, in isolation. If you write app code, edit
the scaffolded project, or call the idealyst MCP on the agent's behalf, the run
is invalid: throw it away and start over.

Why this exists: the arena measures how well an agent can build an idealyst app
when its ONLY documentation is the idealyst MCP. The implementer is hard-isolated
to that MCP (see `.claude/agents/arena-implementer.md`). Running it as a subagent
keeps the work on the Claude **subscription** — do **not** shell out to
`claude -p` / `claude --print` (that bills as pay-as-you-go API usage).

## Inputs

- **Scenario id** — from the skill args (default `todo-app`). The scenario lives
  at `arena/scenarios/<id>/`.
- **Framework path** — the repo root (the current working directory,
  `/Users/nicho/Desktop/idealyst-native`). Scaffolds path-dep this checkout so
  the run tests *current* framework + MCP.
- **Run index** — default `0`; bump for repeat samples.

All `arena …` commands below are the spine CLI:
`cargo run -q -p arena-spine --bin arena -- <subcommand> …` run from `arena/`.
(Or use the prebuilt `arena/target/debug/arena` if present.)

## Steps

1. **Preflight.** Confirm the installed `idealyst` matches this checkout — the
   scaffold + build + the agent's MCP all use it. If unsure, the user can
   `cargo install --path crates/tools/cli`. Set:
   - `SCEN=arena/scenarios/<id>`
   - `RUN_DIR=arena/runs/<id>/run-<index>`

2. **Scaffold** the isolated project and capture its path:
   ```
   arena scaffold $SCEN <framework_path> --run-dir $RUN_DIR --index <index>
   ```
   The last stdout line is the **project dir** — call it `$PROJ`. (It also writes
   `$PROJ/.mcp.json` for the record; the subagent defines its own idealyst MCP.)

3. **Compose the task prompt.** Read the **public** prompt only — `$SCEN/scenario.toml`
   (the `prompt = """…"""` field). **Never read `$SCEN/rubric.toml`** — it is
   secret and must not enter your or the agent's context. The task prompt is:
   ```
   idealyst MCP available, use it.

   <the scenario prompt verbatim>
   ```

4. **Spawn the implementer subagent.** Use the Agent tool with
   `subagent_type: "arena-implementer"`, the task prompt from step 3, and a
   first instruction to `cd` into `$PROJ` and build the app there. Let it run to
   completion. Note the **agentId** from the tool result. Do not coach it,
   answer its questions, or supply MCP knowledge — its isolation is the point.

5. **Resolve its transcript.** The subagent's event log is at
   `~/.claude/projects/<enc>/$CLAUDE_CODE_SESSION_ID/subagents/agent-<agentId>.jsonl`.
   Resolve it robustly:
   ```
   TX=$(find ~/.claude/projects -path "*/$CLAUDE_CODE_SESSION_ID/subagents/agent-<agentId>.jsonl" 2>/dev/null | head -1)
   # fallback: newest agent-*.jsonl under this session if the id didn't match
   [ -z "$TX" ] && TX=$(ls -t ~/.claude/projects/*/$CLAUDE_CODE_SESSION_ID/subagents/agent-*.jsonl 2>/dev/null | head -1)
   ```

6. **Build** the web target (gives an early build gate + warms the cache the
   compile tier re-uses):
   ```
   arena build $PROJ
   ```
   A non-zero exit just means the compile tier will fail in scoring — continue.

7. **Score.** Verifies every rubric item at its tier, applies divergence
   neutralization + the token bonus, and writes `report.md` + `scored.json`:
   ```
   arena score $SCEN $PROJ --run-dir $RUN_DIR --impl-transcript "$TX"
   ```

8. **Report** to the user: the score line, the artifact paths
   (`$RUN_DIR/report.md`, `scored.json`), and — as a self-check — confirm the
   implementer stayed isolated by spot-checking `$TX`:
   - only `mcp__idealyst*` MCP tool calls appear (no `mcp__playwright`/web/other),
   - `doc-bypass reads` in the report is 0 (the agent didn't read framework
     source instead of asking the MCP).

## Scope notes

- **Core loop = source tiers** (compile + static). The `playwright` outcome tiers
  need the served build + an `arena-locator` subagent, and the `feedback` pass
  needs an `arena-feedback` subagent — both are the fast-follow, not wired here.
  Until then those items **skip** (they neither score for nor against the agent).
- For an N-run bench, repeat steps 2–8 with `--index 1,2,…`, then aggregate the
  per-run `scored.json` files (aggregate subcommand: fast-follow).
- One run spends real subscription tokens on the implementer subagent. Don't
  loop it unattended without the user's go-ahead.

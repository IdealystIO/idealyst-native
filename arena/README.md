# MCP Arena

A benchmark for how well an AI uses the **idealyst MCP server** to build
cross-platform apps. The implementation AI runs in isolation with the MCP
server as its *only* source of documentation and app introspection (Robot is
itself an MCP tool). Each scenario is scored against a secret, objective rubric
— never by an LLM judge.

## The pieces

```
arena/
  Cargo.toml          its OWN workspace (decoupled from the framework's)
  spine/              the deterministic Rust core — this is what's built so far
  scenarios/          tasks + secret rubrics (TOML)
    todo-app/
      scenario.toml   PUBLIC: the prompt the agent receives
      rubric.toml     SECRET: atomic, tiered, objectively-checkable items
  agents/             the LLM-driven half: implementation / locator / feedback
  runs/               per-run artifacts (gitignored)
```

## How a run works

1. **Isolate.** Scaffold a fresh idealyst app in a temp dir, repoint its
   idealyst deps at this working tree (so the run tests the *current* framework
   + MCP), connect only the idealyst MCP, and disable web/other doc sources.
2. **Preamble.** The agent gets one identical line — *"idealyst MCP available,
   use it"* — plus the scenario's `prompt`. Nothing about *how* to navigate the
   MCP; that would mask the doc deficiencies the arena exists to find.
3. **Implement.** The agent codes, building/running via the CLI and
   introspecting **only** through Robot (an MCP tool). It has no Playwright, no
   framework source, no web search.
4. **Verify.** Each rubric item is checked at its tier (below). The evaluator
   *does* get Playwright/CLI.
5. **Score + feed back.** A score per run, plus a two-pass feedback report that
   points at MCP improvements (it doesn't change the score).

## Two epistemologies, kept separate

- **Robot** = the framework's self-report — what the agent can see.
- **Playwright** = platform truth — what the evaluator can see.

When they disagree (the agent wrote the right code, Robot says fine, the
platform renders wrong), the agent is **not** penalized: the gap becomes a
*framework finding* in the feedback report. This is encoded as
**divergence neutralization** — an `outcome` item that fails while its
`depends_on` `decision` item passed is removed from the score and surfaced as a
finding.

## Rubric tiers

| tier | checks | cost |
|---|---|---|
| `compile` | `idealyst build <target>` / `cargo check` succeeds | low |
| `static` | regex/AST assertion over produced source | lowest |
| `robot` | a Robot verb against the app's self-report | mid |
| `playwright` | platform truth via the `arena-locate` skill | high |

`compile` + `static` are wired now. `robot` + `playwright` report `skip` until
their verifiers land (they need the framework path deps and the locator skill).

## Item classes

- `decision` — did the agent pick/wire the right thing (source/Robot-verifiable).
  Always the agent's responsibility.
- `outcome` — does it actually render/behave (platform truth). Gated on a
  `decision` via `depends_on` for the neutralization rule above.

## Scoring

```
final = rubric_points + token_bonus
  token_bonus ∈ [0, ε),  ε < smallest rubric item value
```

Rubric points dominate; the token bonus can never outrank a single rubric
point but always breaks ties toward the cheaper run. `mcp_payload_tokens` is
tracked separately — it's the dial *you* watch when deciding whether a doc edit
earned its length.

## Aggregation

Run each scenario N times. The headline signal isn't the mean — it's the
**per-item pass-rate**: an item passing 8/8 is well-documented; 3/8 is a doc
*ambiguity*, not mere model variance, and is exactly where to improve the MCP.

## CLI

```
cargo run -p arena-spine --bin arena -- verify scenarios/todo-app <project_dir>
cargo run -p arena-spine --bin arena -- metrics <transcript.json>
```

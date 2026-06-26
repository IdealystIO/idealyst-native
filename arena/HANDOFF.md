# MCP Arena — Handoff

You're picking up the **MCP Arena**: a benchmark that measures how well an AI
can build a cross-platform idealyst app when its **only** source of
documentation and app-introspection is the idealyst MCP server. The point isn't
"does the app work" — it's "does the AI make good decisions from the MCP docs +
Robot introspection," so we can find and fix the MCP's weak spots.

Read this whole file before changing anything. The architecture below is
**decided** — don't relitigate it; build on it. There's also a memory entry
`project_mcp_arena` with the running history.

> ## ⚠️ Architecture update (2026-06-26): agent-driving moved off `claude -p`
>
> The deterministic spine (scenario/rubric, verify tiers, score, metrics,
> report) is unchanged. What changed is **how the agent roles are driven**.
>
> **Why:** headless `claude --print` is billed as pay-as-you-go **API** usage,
> not covered by the Claude subscription (Anthropic announced a split in May
> 2026, paused on its June 15 launch but "under revision"; and a bug routes even
> OAuth `-p` to API billing). The spine also ran *inside* a sandboxed Claude
> Code session, which denies the spawned `claude` network access — so it hung.
>
> **Now:** a **Claude Code session is the entrypoint** and runs the agent roles
> as **subagents** (subscription-billed). The implementation agent is the
> `arena-implementer` subagent (`.claude/agents/arena-implementer.md`),
> hard-isolated to the idealyst MCP via `mcpServers:` frontmatter — the
> equivalent of `--strict-mcp-config`. The orchestrator skill is
> `.claude/skills/arena-bench`. The spine is now exposed as granular subcommands
> (`scaffold`/`build`/`score`) the skill stitches together; `arena run`/`bench`
> and the `claude -p` spawners (`harness/{run,bench}.rs`,
> `agent::implement`, `locate::run_item`, `feedback::synthesize`) are **removed**.
>
> **Status:** the **core loop** (scaffold → implementer subagent → build → score
> on the **compile + static** tiers) is built + unit-tested. Not yet exercised
> end-to-end. The Playwright-locate tier, the feedback pass, and the N-run bench
> aggregate are the **fast follow** (their `playwright`/feedback items currently
> *skip*). Sections 3–5 below describe the *original* `claude -p` design; read
> them for intent, but the file map (§4) and run steps (§5) reflect the new
> subagent flow.

---

## 1. Mission & scoring model

Per scenario:
1. An **implementation agent** runs in isolation (MCP-only) and produces a
   project tree + a transcript.
2. Each **rubric item** is verified objectively at its tier (no LLM-as-judge).
3. **Score = rubric_points + token_bonus**, where the token bonus is strictly
   smaller than the smallest rubric item — so correctness always dominates, but
   among equal-rubric runs the cheaper (fewer-token) run wins.
4. A separate **feedback agent** (diagnostic, doesn't affect score) explains
   *why* points were lost and proposes MCP-doc fixes.

Run each scenario **N times**; the headline metric is **per-item pass-rate**
across runs. An item that passes 8/8 is well-documented; 3/8 is a documentation
*ambiguity*, not just model variance — that's the signal you act on.

---

## 2. Status — what's built vs. what's unrun

### Built and verified (deterministic + integration)
- **Full deterministic spine** (`arena/spine/src`): scenario/rubric model, the 4
  verifier tiers, scoring with divergence neutralization, transcript pathology
  metrics, report rendering. Unit-tested.
- **Robot tier** is a real TCP bridge client; **verified end-to-end** against a
  relay-fronted web app (`tests/robot_web.rs`, `#[ignore]`).
- **Scaffold** verified against the real `idealyst new` (`tests/scaffold_smoke.rs`).
- **Harness** (the live half) is fully wired: scaffold → implement agent
  (`claude` headless, stream-json capture) → web build → locator pass → verify →
  score → feedback → aggregate.
- `arena verify` CLI runs the source tiers against any produced tree (used it to
  smoke the example).

### NOT yet run — the #1 next step
- **A full live `bench` has never been executed.** Every seam is unit-tested and
  the deterministic + robot-web paths are verified, but no real
  implementation-agent run has gone scaffold→agent→build→verify→score→feedback
  end to end. **Do this first** (see §5) — it will surface real integration
  issues (claude flags, transcript parsing, the locate agent, feedback prompt).
- Only **one scenario** exists (`todo-app`).
- The **locate (Playwright) tier** and the **feedback agent** are wired but never
  exercised live.

---

## 3. Architecture (decided — don't change without reason)

### Rubric: secret, atomic, objective
Each item declares a **tier** (how it's checked) and a **class** (whose fault a
failure is). No subjective scoring. Items derive strictly from the scenario's
stated requirements or framework-level docs — never penalize something the
scenario didn't ask for (e.g. don't require server-persistence if the prompt
only said "persists across reloads").

**Tiers** (cheapest first):
- `compile` — `idealyst build <target>` / `cargo check` succeeds.
- `static` — a regex/AST assertion over the produced source.
- `robot` — a Robot verb against the app's **self-report** (the framework's view).
- `playwright` — **platform truth** via a locator agent driving a browser.

**Classes:**
- `decision` — did the agent pick/wire the right thing (source- or Robot-
  verifiable). Always the agent's responsibility.
- `outcome` — does it actually render/behave (platform truth). Gated on a
  `decision` via `depends_on`.

### Two epistemologies — keep them separate
- **Robot** = the framework's self-report (what the agent can see).
- **Playwright** = platform truth (what the evaluator can see).

When they disagree (right code, Robot says fine, platform renders wrong), the
agent is **not** penalized — it's a *framework finding*. This is encoded as
**divergence neutralization**: an `outcome` item that fails while its
`depends_on` `decision` passed is removed from the score and surfaced as a
finding. See `score.rs`.

### Isolation
A run scaffolds a **standalone** app in a temp dir, **path-deps the local
framework** (so the run tests *current* code, not a published snapshot),
connects **only** the idealyst MCP (`claude --strict-mcp-config`), and denies
web/other doc sources. The preamble the agent receives is the floor —
`agents/preamble.md` is literally "idealyst MCP available, use it." Anything more
masks the doc deficiencies the arena exists to find. (Known leak: path-deps make
framework source readable; `metrics::doc_bypass_reads` flags reads outside the
project as a process pathology.)

### Feedback agent — two passes, diagnostic only
1. **Rubric-anchored:** for each lost/neutralized item, trace the transcript to
   the MCP navigation/comprehension failure that caused it.
2. **Process logic:** explain the deterministic pathologies (`metrics`:
   thrashing, repeated doc fetches, doc-bypass reads) into concrete doc fixes.
Pass 2's *detection* is pure code; the agent only clusters + explains.

---

## 4. File map

```
arena/
  Cargo.toml            its OWN workspace (excluded from the framework workspace)
  spine/
    src/
      scenario.rs       public Scenario (the prompt the agent sees)
      rubric.rs         secret Rubric: items, tiers, classes, assertions
      verify/
        compile.rs      compile tier
        static_ast.rs   static (regex) tier
        robot.rs        robot tier — real TCP bridge client + ~/.idealyst/apps discovery
        playwright.rs   playwright tier — reads the locator's {passed,evidence} verdict
      score.rs          score_from_results + divergence neutralization + token_bonus  ← pure, unit-tested
      metrics.rs        transcript pathologies (feeds feedback pass 2)
      report.rs         per-run + aggregate markdown
      harness/
        scaffold.rs     idealyst new + path-dep + isolated .mcp.json; build_web(dir, robot)
        agent.rs        parses a SUBAGENT transcript (agent-<id>.jsonl) → tool calls + tokens
        locate.rs       Playwright-tier contract: build_prompt / parse_verdict / write_verdict
        feedback.rs     build_feedback_prompt (pure) — the two-pass reviewer prompt
        robot_web.rs    relay + headless Chrome so robot-tier works on a web build (fast-follow)
      bin/arena.rs      CLI: verify | metrics | scaffold | build | score
    tests/              static_verify, robot_web (#[ignore]), scaffold_smoke (#[ignore])
  scenarios/
    todo-app/
      scenario.toml     PUBLIC: the prompt
      rubric.toml       SECRET: the items
  agents/
    preamble.md         the one identical-across-runs line (mirrored into the subagent body)
    README.md           contracts for the implement / locate / feedback agents
  runs/                 gitignored per-run artifacts

# orchestration (repo-root .claude/, NOT under arena/):
.claude/agents/arena-implementer.md   agent-under-test: idealyst-MCP-only, no web
.claude/skills/arena-bench/SKILL.md   orchestrator: scaffold → spawn → build → score
```

### Rubric/scenario schema (author scenarios against this)

`scenario.toml`:
```toml
id = "todo-app"
prompt = """<requirements only; no hints about how to use the MCP>"""
platforms = ["web"]      # which platforms outcome items verify on
token_budget = 2_000_000 # hard ceiling per agent run
runs = 5                 # statistical samples
```

`rubric.toml` (one `[[item]]` per check):
```toml
scenario_id = "todo-app"

[[item]]
id = "uses-list-primitive"
description = "Renders items through flat_list, not a hand-rolled loop."
points = 10
class = "decision"               # decision | outcome
tier = "static"                  # compile | static | robot | playwright
verifier = "static_ast"
assertion = { pattern = "flat_list|virtualizer", in = "src/**/*.rs" }

[[item]]
id = "add-item-renders"
points = 20
class = "outcome"
tier = "playwright"
verifier = "ux_locator"
depends_on = "uses-text-input"   # neutralization: if this passes but the outcome fails → framework finding
assertion = { action = "type 'Buy milk' and submit", expect_role = "listitem", expect_name = "Buy milk" }
```
Assertion fields by tier: static → `pattern`/`in`/`absent`/`min_count`;
compile → `target`; robot → `verb`/`expect_name`; playwright → `action`/
`expect_role`/`expect_name`. See `rubric.rs::Assertion`.

---

## 5. How to run it

### Prerequisites
- `claude` (headless agent), `idealyst` (CLI — **must be the build with the
  robot work**, see note below), `python3` (serves the web build), Google Chrome
  (robot-tier headless load), `npx` (Playwright MCP for the locate tier).
- **Important:** the arena scaffolds apps that path-dep this framework checkout.
  The CLI that scaffolds must match. Either `cargo install --path
  crates/tools/cli` or point the harness at `target/debug/idealyst`.

### Fast checks (no live agent)
```bash
cd arena
cargo test                                   # spine unit + static_verify
# robot-tier end-to-end (needs a prebuilt robot web bundle + Chrome):
#   idealyst new app (IDEALYST_FRAMEWORK_PATH=<repo>); idealyst build --web --robot
ARENA_ROBOT_DIST=/path/to/app/dist/web cargo test -p arena-spine --test robot_web -- --ignored --nocapture
```

### Score an existing tree (source tiers only)
```bash
cargo run -p arena-spine --bin arena -- verify scenarios/todo-app <project_dir>
```

### A full live run  ← DO THIS FIRST (now subagent-driven)
A run is orchestrated by a **Claude Code session** via the `arena-bench` skill,
not by a CLI subcommand — that's what keeps it on the subscription. From the repo
root, in a Claude Code session:
```
/arena-bench todo-app
```
The skill (`.claude/skills/arena-bench/SKILL.md`) drives the deterministic spine
subcommands around one subagent spawn:
```bash
# what the skill runs under the hood (from arena/):
arena scaffold scenarios/todo-app <framework_path> --run-dir runs/todo-app/run-0 --index 0
#   → spawns the `arena-implementer` subagent (idealyst-MCP-only) in the project
arena build  <project_dir>
arena score  scenarios/todo-app <project_dir> --run-dir runs/todo-app/run-0 --impl-transcript <agent.jsonl>
```
Artifacts land under `runs/<scenario>/run-*/` (`report.md`, `scored.json`). The
implementer's transcript is the subagent log at
`~/.claude/projects/<proj>/<session-id>/subagents/agent-<id>.jsonl`.

**First-run checks:** confirm the implementer stayed isolated (its transcript
shows only `mcp__idealyst*` MCP calls, `doc-bypass reads` = 0), the build
gated correctly, and — the whole point — usage landed on the **subscription**
(Console), not API billing. The `playwright` and feedback items **skip** until
the fast-follow (locator + feedback subagents) is wired.

---

## 6. Remaining work (prioritized)

1. **Run the core loop live (`/arena-bench todo-app`) and make it green.**
   Confirm the implementer subagent stays idealyst-MCP-only, the scaffold/build/
   score subcommands chain cleanly, the transcript resolves, and the score lands
   with the compile+static tiers. This is the gate for everything else.
1a. **Fast-follow: wire the locator + feedback subagents and the bench loop.**
   Add `.claude/agents/arena-locator.md` (Playwright-MCP-only) and
   `arena-feedback.md` (Read-only); reintroduce an `arena live` subcommand
   (serve + relay + headless host — `serve`/`ServeGuard` lived in the deleted
   `run.rs`, recoverable from git) so the locator + robot tiers have a running
   app; add `arena aggregate` + an N-run loop in the skill.
2. **Validate the feedback report** is actually actionable — that pass-1 traces
   lost items to MCP failures and pass-2 turns pathologies into concrete fixes.
   This is the arena's real product.
3. **Author more scenarios** (each = `scenario.toml` + secret `rubric.toml`).
   Cover different MCP surfaces: navigators, an SDK (storage/net/camera),
   server-fns, forms, theming. Keep items atomic + tier-tagged.
4. **Tune** `token_bonus` ε, `runs` (variance), and per-scenario `token_budget`.
   Watch `mcp_payload_tokens` (reported separately) — it's the doc-bloat dial.
5. **Close the loop on the MCP:** use per-item pass-rate + feedback to actually
   edit the MCP docs/tools (`crates/mcp/server`, `crates/mcp/catalog`), then
   re-bench to confirm the score moved. That's the whole purpose.
6. Optional ergonomics: an `idealyst robot <verb>` CLI that auto-discovers the
   running app and pretty-prints, so debugging scenarios doesn't need raw `nc`.

---

## 7. Constraints & gotchas

- **No LLM-as-judge.** Even the Playwright locate agent must return a *binary
  observable* (`{passed, evidence}`), never an opinion. Validation of that
  verdict happens in `verify/playwright.rs` against a fixed schema. Keep it that
  way.
- **The preamble stays minimal.** Don't add MCP-navigation hints; that hides the
  exact deficiencies you're trying to measure.
- **Don't penalize the unspecified.** Rubric items come from stated requirements
  or framework docs only.
- **Robot vs Playwright divergence = framework finding, not a deduction.** The
  neutralization logic is in `score.rs` — don't bypass it.
- **The arena is its own workspace.** It path-deps `robot-relay` and the
  framework; it's `exclude`d from the framework workspace so arena churn doesn't
  rebuild the framework. Keep it that way.

---

## 8. Background: robot-on-web/mobile (prerequisite, now DONE)

The arena needs to introspect a *running* app via Robot on every platform. That
was just finished and is the substrate the `robot` tier rides on:

- A **relay** (`crates/dev/robot-relay`) is now the universal robot server. Apps
  **dial out** to it over WebSocket (web via `web_sys`, native via
  `tungstenite`); the relay exposes the ordinary TCP bridge + `~/.idealyst/apps`
  registration, so the MCP/evaluator side is unchanged on every platform.
- **Robot is on by default in `idealyst dev`** (opt out with `--no-robot`):
  `dev` hosts the relay and wires the app to dial it — web-local injects the URL,
  desktop natives inherit it via env, iOS-sim via `simctl`, Android via a
  manifest meta-data placeholder + `adb reverse`. Verified live on web, macOS,
  iOS sim, and Android emulator.
- For the arena, `harness/robot_web.rs` does this itself (build `--robot`, host a
  relay, headless-Chrome the served bundle) so robot-tier items work on a web
  build without a `dev` session. **The arena's robot tier currently only does
  web** (headless Chrome). Native robot-tier in a run would mean launching the
  app natively in the run loop — not wired; web is the priority per the design.

If you need the deeper history, the `project_mcp_arena` memory entry has the full
blow-by-blow, and `arena/README.md` has the user-facing overview.

---
name: arena-bench
description: Run one MCP Arena scenario — scaffold an isolated idealyst app inside the arena container, drive the NAIVE in-container implementer (headless claude on seeded subscription creds), then build + score on the deterministic spine. Use when asked to run the arena, bench a scenario, or evaluate the idealyst MCP docs.
---

# Run an MCP Arena scenario

You are the **orchestrator**, running OUTSIDE the container (an ordinary host
session). You scaffold, drive the agent-under-test, build, and score. **You
never implement the app yourself** — all implementation is done by the
**naive in-container implementer**. If you write app code, edit the scaffolded
project, or call the idealyst MCP on the agent's behalf, the run is invalid:
throw it away and start over.

## Topology (v2 — decided 2026-07-20)

- **Implementer (agent-under-test)** — a SEPARATE headless `claude` session
  INSIDE the arena container (`.devcontainer/arena/`), started via
  `docker exec`. It is **naive by construction**: its cwd is the scaffolded
  project OUTSIDE the repo tree (no CLAUDE.md pickup, no repo skills/agents),
  `--strict-mcp-config` limits it to the project's idealyst `.mcp.json`, the
  container seeds no user settings/hooks/plugins, and the container firewall
  allows no doc sources beyond the MCP. It starts the project from scratch.
- **Orchestrator + reviewer agents** (you; later the locator/feedback/quality
  subagents) — OUTSIDE the container, with full tooling. Reviewers never enter
  the container; deterministic steps run inside it via `docker exec`.
- Billing: the implementer uses the container's **seeded subscription OAuth
  credentials** — the documented subscription path for headless use. On the
  first v2 run, verify in the Console that usage landed on the subscription
  (the old `claude -p` API-billing bug is why the previous architecture
  existed; trust but verify once).

Why: the arena measures how well a from-scratch agent builds an idealyst app
when its ONLY documentation is the idealyst MCP. (The legacy subagent flow —
`.claude/agents/arena-implementer.md` — leaked repo context: subagents receive
the repo's CLAUDE.md. Use it only as an explicitly-marked non-hermetic
fallback when no container is available, and say so in the report.)

## Inputs

- **Scenario id** — from the skill args (default `todo-app`); lives at
  `arena/scenarios/<id>/`.
- **Run index** — default `0`; bump for repeat samples.
- **Model** — pass `--model opus` to the implementer. Do not vary it unless
  the user asks to bench a different model; cross-run comparisons are only
  valid within one model (the report records which model ran).

## Execution environment: the arena RUNNER (per-run disposable container)

Each run gets its own container from the `idealyst-arena-runner` image
(`arena/runner/Dockerfile` — toolchain + claude + idealyst CLI + spine all
BAKED at image build). Per-container network namespaces isolate relay ports
and `~/.idealyst/apps`, so N runs can go in parallel with zero cross-talk,
and teardown is `docker rm`.

- **Image freshness**: rebuild after framework/CLI changes —
  ```
  docker build -f arena/runner/Dockerfile --build-arg COMMIT=$(git rev-parse --short HEAD) -t idealyst-arena-runner <repo_root>
  ```
  The entrypoint WARNs when the mounted repo's HEAD differs from the baked
  commit; treat that warning as "rebuild before benching".
- **Start one runner per run** (from the repo root; `<id>` = scenario+index):
  ```
  docker run -d --rm --init --name arena-run-<id> --label arena-run \
    --cap-add NET_ADMIN --cap-add NET_RAW \
    -v <repo_root>:/workspaces/idealyst-native:ro \
    -v arena-target-<id>:/workspaces/idealyst-native/target \
    -v <repo_root>/../arena-runs:/workspaces/arena-runs \
    -v $HOME/.claude:/host-claude:ro \
    idealyst-arena-runner
  ```
  The entrypoint seeds fresh credentials, applies the egress firewall, and
  idles. Wait for `docker logs` to show `firewall ok: open web blocked`
  before proceeding. Repo is READ-ONLY inside (builds write through the
  target volume overlay); runs live at `/workspaces/arena-runs/…`, outside
  the repo tree so the implementer's upward config traversal finds nothing,
  and host-visible at `../arena-runs` for review.
- Exec pattern for every step below:
  `docker exec -u vscode -w <dir> arena-run-<id> bash -lc '…'`.
  Binaries on PATH: `idealyst`, `arena`, `claude`, `chromium`.
- **Teardown after scoring**: `docker rm -f arena-run-<id> && docker volume rm arena-target-<id>`.
  Sweep strays: `docker rm -f $(docker ps -aq -f label=arena-run)`.
- The arena **devcontainer** (`.devcontainer/arena/`) is the HUMAN surface —
  interactive debugging, auth bootstrap — and the fallback if the runner
  image is unavailable; benching in it is single-instance and couples to the
  VS Code window lifecycle (closing it killed run-1). If you must use it,
  note that in the run report.

## Steps

1. **Preflight.** Ensure the `idealyst-arena-runner` image exists and is
   fresh (rebuild on the drift warning), start the per-run container, and
   wait for its firewall-ok log line. If a run dies mid-flight (host reboot,
   container killed), the implementer session is resumable:
   `claude -p --resume <session_id>` + a bare "Continue." — prefer that over
   rerunning. Set `SCEN=arena/scenarios/<id>` (repo-relative),
   `RUN_DIR=/workspaces/arena-runs/<id>/run-<index>` (container path).

2. **Scaffold** (in-container, from the repo root):
   ```
   arena scaffold $SCEN /workspaces/idealyst-native --run-dir $RUN_DIR --index <index>
   ```
   Last stdout line = project dir `$PROJ` (container path). Scaffold writes
   `$PROJ/.mcp.json` (idealyst only) — that file IS the implementer's MCP
   universe.

2b. **Warm the catalog — MANDATORY in a fresh runner.** The MCP server's
   catalog extraction builds the project (and, on a cold per-run target
   volume, the whole framework — 15–20 min). Claude's MCP connect times out
   long before that, the server sticks at `status: "pending"`, and the naive
   agent SILENTLY proceeds doc-less — invalidating the bench (hit live,
   run-3 first attempt: 0 MCP calls). From `$PROJ`:
   ```
   docker exec -u vscode -w $PROJ arena-run-<id> bash -c 'idealyst catalog-json > /dev/null'
   ```
   and only launch the implementer after it exits 0. Belt-and-braces: launch
   the implementer with `MCP_TIMEOUT=120000` in its env. NOTE the init event
   always reports `"status":"pending"` (handshake is async) — the real health
   check is that `mcp__idealyst__*` calls appear within the first ~100
   events. If they don't, the server never connected: kill, fix, relaunch. If scaffold prints a `NOTE:` about sidecar services, STOP and
   relay it: services are enabled via `idealyst configure devcontainer
   --config arena …` + a container restart; they can't come up mid-run.

3. **Compose the task prompt.** Read the **public** prompt only —
   `$SCEN/scenario.toml` (`prompt = """…"""`). **Never read `$SCEN/rubric.toml`**
   — it is secret and must not enter your or the agent's context. Task prompt:
   ```
   idealyst MCP available, use it.

   <the scenario prompt verbatim>
   ```

4. **Run the implementer** (headless, in-container, cwd = `$PROJ`):
   ```
   docker exec -u vscode -w $PROJ arena-run-<id> \
     bash -c 'unset CLAUDE_CODE_OAUTH_TOKEN; claude -p "$0" \
       --model opus \
       --output-format stream-json --verbose \
       --mcp-config .mcp.json --strict-mcp-config \
       --dangerously-skip-permissions' "$TASK_PROMPT" \
     > "$HOST_RUN_DIR/impl-transcript.jsonl" 2> "$HOST_RUN_DIR/impl-stderr.log"
   ```
   (`$HOST_RUN_DIR` = the same run dir via the host-side path.) Run it in the
   background and wait; a run takes tens of minutes. Do not coach it or answer
   its questions — its isolation is the point. `--dangerously-skip-permissions`
   is justified ONLY because the container is the sandbox (firewalled,
   disposable); never use it for host runs.

5. **Build** the web target WITH robot support (in-container — early gate,
   warms the compile tier's cache, and produces the bundle the outcome tiers
   serve): `arena build $PROJ --robot`. Non-zero exit just means the compile
   tier will fail in scoring — continue, but skip steps 6–7.

6. **Go live** (in-container, background — the outcome tiers need a running
   app). Publish the port when starting the runner (`-p 8095:8095`) so the
   host-side locator can reach it:
   ```
   docker exec -d -u vscode -w /workspaces/idealyst-native/arena arena-run-<id> \
     bash -lc './target/debug/arena live $PROJ --port 8095 > $RUN_DIR/live.log 2>&1'
   ```
   Wait for `LIVE bridge=… url=…` in `live.log` (the headless client dialing
   in is part of `live` — it errors within ~30s if the app never connects).

7. **Locator pass** (playwright tier — HOST-side reviewer). Get the prompts:
   ```
   arena locate-prompts $SCEN --base-url http://127.0.0.1:8095
   ```
   For each `{item_id, prompt}`: spawn an **arena-locator** subagent with the
   prompt verbatim, then pipe its ENTIRE final reply into
   ```
   echo "$REPLY" | arena verdict $RUN_DIR/locate <item_id>
   ```
   (in-container path for the locate dir). The parse is prose-tolerant and an
   unparseable reply becomes a deterministic FAIL verdict — never re-ask the
   locator to "fix" its answer.

8. **Score** (in-container — NEVER host-side: the project path-deps container
   paths, so a host score fails the compile tier and mis-counts doc-bypass):
   ```
   arena score $SCEN $PROJ --run-dir $RUN_DIR \
     --impl-transcript $RUN_DIR/impl-transcript.jsonl --locate-dir $RUN_DIR/locate
   ```
   Writes `report.md`, `scored.json`, `process.json`. Robot-tier items verify
   against the live bridge while step 6's `live` is still up. Tear `live`
   down after (or just tear the runner down at the end).

9. **Feedback pass** (model assessment — diagnostic, never changes a score):
   ```
   arena feedback-prompt $SCEN $RUN_DIR   # container paths in output —
   ```
   translate `/workspaces/arena-runs` → the host arena-runs path, then spawn
   an **arena-feedback** subagent with the result; save its Markdown reply to
   `$RUN_DIR/feedback.md`. This is the arena's real product — read it.

10. **Quality pass** (non-scoring):
    ```
    arena quality $PROJ --run-dir $RUN_DIR                # deterministic base (lint)
    arena judge-prompt $PROJ --screenshots <shots-dir>    # host paths for the judge
    ```
    Spawn an **arena-quality** subagent with the judge prompt, save its JSON
    reply to `$RUN_DIR/judge.json`, then merge + validate:
    ```
    arena quality $PROJ --run-dir $RUN_DIR --judge-file $RUN_DIR/judge.json
    ```
    A schema-invalid judge reply errors — discard it (deterministic base
    stands alone) rather than coaching the judge.

10b. **Adversarial review** (non-scoring — the expert skeptic):
    ```
    arena adversary-prompt <host-side $PROJ> --framework <host repo root>
    ```
    Spawn an **arena-adversary** subagent with the output; save its JSON
    reply to `$RUN_DIR/adversary-findings.json`, then validate + persist:
    ```
    arena adversary $PROJ --run-dir $RUN_DIR --findings-file $RUN_DIR/adversary-findings.json
    ```
    Schema-invalid output errors — discard it rather than coaching. Surface
    critical findings to the user prominently; `rubric_candidate` entries are
    proposals to harden into objective rubric items next iteration.

11. **Record** the run in the longitudinal ledger (repo-tracked CSVs at
    `arena/results/<scenario>.csv` + `<scenario>-items.csv`; commit them
    with the next checkpoint):
    ```
    arena record $SCEN $RUN_DIR
    ```
    Re-recording a re-scored run needs `--force` (append-only ledger).

12. **Report** to the user: the score line, artifact paths (`report.md`,
    `scored.json`, `process.json`, `quality.md`, `feedback.md`), and
    self-checks:
    - transcript shows only `mcp__idealyst*` MCP calls;
    - doc-bypass reads + MCP error count from `process.json`;
    - the report's **Model** line matches the requested model.

## N-run benches + aggregation

Repeat steps 2–11 with `--index 1,2,…` — with the runner, runs can go IN
PARALLEL (one container each, distinct published ports). Then:
```
arena aggregate <scenario_id> <run-0>/scored.json <run-1>/scored.json … > aggregate.md
```
Per-item pass-rate is the headline: a low-rate item is a doc ambiguity to fix,
not model variance. One run spends real subscription tokens — don't loop
unattended without the user's go-ahead.

## Scope notes

- Reviewer agents (`arena-locator`, `arena-feedback`, `arena-quality`) are
  registered from `.claude/agents/` at session start — a session older than
  those files must use general-purpose subagents with the same prompts.
- Scenario slate: `todo-app`, `nav-notes`, `themed-settings`, `debug-fix`
  (ships broken starting code via `assets/`). Never read any `rubric.toml`.

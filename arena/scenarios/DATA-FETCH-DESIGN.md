# Design note: a future `data-fetch` scenario (NOT a scenario yet)

Goal: exercise the **net SDK** (`net::Client` — HTTP/WS/SSE) plus the reactive
`resource`/loading pattern, scoring an app that fetches data and renders
**loading / error / empty / loaded** states.

## The constraint that shapes everything

The arena runner is firewalled to an **allowlist**: Anthropic endpoints +
crates.io only. No github, npm, docs.rs, or arbitrary web (`.devcontainer/arena/`
egress firewall; see HANDOFF §5). So a data-fetch scenario **cannot** point the
app at a public API — the fetch would just hang or get refused, and the run
would measure the firewall, not the MCP docs.

Two ways to give the app something real to fetch without leaving the box:

### Option (a) — a bundled local mock the scaffold serves

Ship a tiny mock HTTP server (or an idealyst `#[server]` server-fn sidecar)
**inside the run**, bound to `127.0.0.1`. Loopback is not egress, so the
firewall never sees it. The app fetches `http://127.0.0.1:<port>/…` and gets
deterministic responses.

- Delivered via the scenario `assets/` overlay (the same mechanism `debug-fix`
  already uses to ship files into a scaffold) — a small server binary or a
  server-fn module — plus, if it's a server-fn, the `services = [...]` field so
  the arena container enables the sidecar before the run (schema already
  supports this; see `scenario.rs::services`).
- The mock exposes fixture routes that force each state on demand:
  `/stats` → 200 JSON (loaded), `/stats?delay=2s` → slow (loading is
  observable), `/stats/boom` → 500 (error), `/stats/empty` → `[]` (empty).
- **Pro:** exercises the real net path end-to-end — DNS-free URL, real
  `net::Client`, real async, real deserialize. This is the capability the
  scenario claims to test.
- **Con:** more moving parts (a second process to build, launch, health-check,
  and tear down in the run loop); the prompt must name the base URL, which
  leaks a hint the pure-UI scenarios don't have.

### Option (b) — scope to STATE rendering, driven by a stubbed fetch

Don't do real I/O. The prompt asks for a screen that renders **loading**, then
**error** or **empty** or **loaded** from a data source the app models as an
async `resource`. The "fetch" is a local stub (a `spawn_async` that resolves a
fixture after a delay, or a seeded in-tree function) — no server at all.

- **Pro:** hermetic, zero sidecar, no URL hint, cheapest to run. Rubric items
  stay in the same shape as today (static: uses `resource`/async-state pattern;
  playwright: the spinner shows, then the loaded rows / the error banner / the
  empty state appears).
- **Con:** doesn't prove the app can talk to `net::Client` — it tests the
  reactive loading-state *UI* and the MCP's docs for `resource`/async, but not
  the network SDK's request surface. A stubbed fetch can pass while the net SDK
  docs are broken.

## Recommendation: **(a)**, with the mock built to also drive (b)'s states

Build option (a) — a bundled loopback mock — because the whole reason to add a
data-fetch scenario is to measure the **net SDK** docs (the arena already has a
storage scenario for persistence and can cover `resource` inside any scenario).
A stubbed fetch (b) can't surface net-SDK doc gaps.

But design the mock's routes to force **loading / error / empty / loaded** on
demand (the fixture routes above), so a single scenario scores both the network
capability *and* the async-state-rendering capability — you get (b)'s state
coverage for free on top of (a)'s real-I/O coverage.

Prefer the **server-fn sidecar** flavor of (a) over a hand-rolled mock binary if
the arena's `services` sidecar path is mature: it keeps the fetch inside the
idealyst stack (`#[server]` + `net::Client` are the documented pair), reuses the
existing `services`/`arena scaffold` devcontainer wiring, and avoids shipping a
second unrelated server toolchain into the run. Fall back to a minimal loopback
mock binary in `assets/` if standing up a sidecar per run proves too heavy.

Open items to settle before writing it:
- How the base URL reaches the app without over-hinting — inject it as an env
  var the scaffold sets (like the robot relay URL) rather than naming it in the
  prompt, so the prompt stays requirements-only.
- Health-check + teardown of the sidecar/mock in the run loop (mirror the
  `robot_web` relay lifecycle).
- Keep the playwright assertions on binary observables (spinner visible →
  rows visible / error text visible), never an LLM opinion (HANDOFF §7).

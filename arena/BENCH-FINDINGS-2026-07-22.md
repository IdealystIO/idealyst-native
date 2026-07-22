# Arena bench — 12 new scenarios, run-1 (2026-07-22)

First validation bench of the expansion scenarios (naive Opus implementer inside
the runner image at HEAD, MCP-connected). Deterministic score + doc-bypass +
feedback reviewers. Quality/adversary deferred. This file is the durable
backlog (task-board MCP is disconnected).

## Scoreboard (higher doc-bypass = rougher MCP docs)

| Scenario | Score | Doc-bypass | Verdict |
|---|---|---|---|
| modal-confirm | 110/110 | 38 | works; `overlay` primitive UNCATALOGUED |
| signup-form | 107/117 | 23 | works; `form` SDK is a framework bug; −10 rubric false-neg |
| big-list | 95/95 | 14 | clean — virtualizer discovered fine |
| shared-cart | 55/55 | 14 | clean — provide/inject docs work |
| data-table | 35/55 | 27 | External-registration trap (feedback pending) |
| i18n-greeting | 20/55 | 13 | never found `i18n!` → hand-rolled `match` |
| swap-tabs | 20/50 | 10 | never used SwapNavigator → hand-rolled tabs |
| animated-panel | 50/50 | 18 | panel never hides — presence undiscoverable (outcome neutralized) |
| clipboard-copy | 60/60 | 13 | clean — clipboard SDK found + used |
| code-split | 55/55 | 10 | clean — found `lazy!{block}` (docs wrongly say `lazy!(path)`) |
| markdown-viewer | 30/45 | 5 | renders fine; low bypass (code_block discoverable); REFUTES the strong registration-trap prediction — markdown renders despite empty register_extensions |
| server-guestbook | (running) | | compile+static only (no live #[server] in harness) |

Batch-3 note: markdown-viewer refuted the strong "external SDK won't render without register_extensions" prediction — it rendered correctly. So the registration trap is confirmed at SOURCE level for `table` (data-table feedback) but its runtime impact varies by SDK/approach; don't overclaim. clipboard + code-split both discovered + used their SDK/macro cleanly (low bypass) — these SDKs ARE discoverable, unlike i18n/swap/table.

## ESCALATE — framework bugs (NOT doc-fixes; code changes outside crates/mcp/catalog/)

These are broken APIs. Documentation cannot make them work — the CODE must
change. Doc edits here are interim stopgaps only, clearly labeled.

- **`form` SDK `Form!` is incompatible with `ui!`** (signup-form). `ui! { Form(...) }`
  does not compile: `FormProps` has no `BuildElement` impl and there's no
  `pub type Form = FormProps` alias; the crate ships a dead
  `#[macro_export] macro_rules! Form!` that `ui!` no longer invokes, and its own
  `form_via_ui_macro` test is written against the removed lowering (it would fail
  to build). CODE FIX in `crates/sdk/client/form/src/lib.rs`: give `FormProps` a
  real `BuildElement` (or `#[component(children)] fn Form`), delete the dead
  macro, fix the self-test. Interim doc stopgap: reword the terse `sdks.rs:209`
  entry to steer to `Field` + signal validation (the working path) — but the SDK
  is what's broken.
- **`text` f-string silently renders `{item.name}` as a literal** (modal-confirm).
  Only a bare identifier is interpolated; a field path / index / method call in
  braces passes through as literal text with NO compile error — a silent, clean-
  building rendering bug any list/detail screen hits. FRAMEWORK DECISION in
  `crates/runtime/macros/src/ui.rs`: either interpolate field paths, or emit a
  `compile_error!` for a non-identifier slot. Interim doc stopgap: caveat on the
  `text` primitive entry — but the silent-wrong-output is a macro defect.

(Borderline, noted not escalated: External UI SDKs render a placeholder instead
of erroring when unregistered under `--local` — a UX sharpness issue, but
registration IS required by design via `defer_external_registration`, so the
primary fix is documentation. See P1 below.)

## META-FINDING: SDK discoverability is the weakest link

Sharp split. **Primitive/reactivity** scenarios (big-list, modal-confirm,
shared-cart) score high — the agent finds the right tool even with thin docs.
**Opt-in-SDK** scenarios (i18n 20/55, swap-tabs 20/50, data-table 35/55) score
low — the agent can't discover the SDK and hand-rolls a worse version. In BOTH
low cases the agent never called `list_sdks`/`describe_sdk` — nothing on the path
it walked pointed at the SDK. Fixing SDK discoverability lifts the most scenarios.

## Prioritized doc-fix backlog (all feedback-reviewer-sourced, file:line cited)

### P0 — catalog gaps that block whole scenarios
1. **`overlay`/`anchored_overlay` primitives are uncatalogued.** `describe_primitive("overlay")` → "not found"; `list_primitives` omits it — yet `overlay` is THE modal tag (ui.rs:1438 `emit_overlay`). Add PrimitiveEntry(s) to `crates/mcp/catalog/src/primitives.rs` (placement/backdrop/on_dismiss/trap_focus props from ui.rs:2392-2431). Kills most of modal-confirm's 38 bypasses.
2. **`presence` props wrong + undiscoverable.** `primitives.rs:601-624` lists prop `when` (belongs to a DIFFERENT primitive); real API is `present`/`enter`/`exit` + `PresenceAnim`/`PresenceState` (presence.rs:198-248). Animation guide (reactivity.md:70-79) is imperative-only; no presence recipe. → animated-panel's panel doesn't hide. Fix props + add a "two animation tools" guide section + presence/toast recipe + cross-link animated! macros → presence.
3. **`form` SDK is a framework bug.** `ui! { Form(...) }` won't compile (no BuildElement impl / no `Form` type alias; ships a dead `macro_rules! Form!`; its own `form_via_ui_macro` test is against the removed lowering). Fix: give FormProps a real BuildElement (or `#[component(children)] fn Form`), delete the macro, fix the test. Reword the terse `sdks.rs:209` entry to steer to Field+signal validation.
4. **i18n SDK invisible.** `describe_sdk("i18n")` = one-liner (sdks.rs:90-95); sdks.md:45 row has no code; no recipe (i18n crate has no `catalog` feature); real API only in crate README. → agent never suspects it. Fix: code-bearing describe_sdk summary + `guides/i18n.md` + `crates/sdk/client/i18n/src/recipes.rs` (+ catalog feature, mirror storage) + sdks.md row.
5. **swap-navigator undiscoverable for "tabs".** Catalog is clean (no ghost entries — #15 held), but nav guide tags (navigation.md:1-5) = `["navigation"]` only — a "tabs" task has no lexical bridge. Fix: broaden tags to tabs/drawer/screens/swap; add a swap+TabBar section to navigation.md; add `crates/sdk/client/navigators/swap/src/recipes.rs` `swap_three_screens_tab_bar` (mirror stack's `stack_two_screens`); front-load "Tabs and drawers" in the swap-navigator summary (sdks.rs:247).

### P1 — high-reach correctness / rubric
6. **Silent `{item.name}` f-string bug.** In `text`, only a BARE identifier interpolates; a field path/index/call (`{item.name}`) renders LITERALLY (compile-clean). Any list/detail screen hits it. Doc caveat on the `text` primitive entry (primitives.rs:76-81) + ideally a macro compile_error.
7. **External-UI-SDK web-registration requirement undocumented — AND the guide misleads.** (data-table: agent never even discovered the table SDK → hand-rolled from view/text → 0/20 uses-table; pure discovery failure. Its 2 "skipped" outcome items were locate-dir skips NOT failures — the app renders+sorts, real score higher than 35/55.) The registration trap is real for a MORE successful agent: `inventory::submit!` self-registration dead-strips under CLI `--local` web build → renders `External "…Props" not supported on web` (runtime, not compile) unless the app calls `<crate>::register(&mut WebBackend)` from a `#[cfg(target_arch="wasm32")] register_extensions`. Undocumented across ALL 8 SdkKind::External UI SDKs (video/webview/maps/svg/markdown/codeblock/table/toolbar — sdks.rs:137-219, all thin summaries), and sdks.md:59-62 ACTIVELY MISLEADS ("add the crate and call the primitive in ui!" — omits register). Fixes: shared registration clause on all 8 external SDK summaries; a "Registering External UI SDKs (required for web)" section in sdks.md with the snippet; a discovery bridge in components.md ("before hand-rolling a table/grid, check list_sdks"); a register-correct table recipe. NOTE: markdown-viewer + codeblock (batch 3) likely hit the same trap.
8. **`portal`/`pressable` listed as primitives but aren't `ui!` tags** — mislabeling misled modal-confirm. Amend their catalog docs to point at `overlay`.
9. **`three-input-fields` rubric false-negative** (signup-form): counts `text_input(`/`Field(` DEFINITIONS; a DRY field helper collapses 3 fields to 1 definition. Count instantiations, or ≥3 on_change/value bindings.

### P2 — reduce source-diving
10. Core style value-types (`StyleRules`, `Shadow`, `Transform`) not answerable via `describe_type` (redirects to styling guide which lacks constructor shapes + import paths).
11. Reactive exports/signatures (`signal`/`memo`/`derived`/`ReadSignal`) + import paths should be in describe_utility/describe_type + reactivity guide.
12. Primitive prop lowering: `describe_primitive` should flag builder-only methods (`.on_key_down`) as distinct from `ui!` props.
13. Missing recipes: modal/confirm-dialog, validated-form, presence/toast, swap/tab, i18n locale-switch.

## Next
- Batch 3 (markdown/clipboard/server-guestbook/code-split) + data-table feedback → append.
- Outcome-tier locators pending for batch 2/3 (scripted; arena-locator agent browser still broken — `chrome-for-testing not installed`).
- Then a doc-fix iteration on P0/P1 + re-bench the low scorers (i18n/swap-tabs/data-table) to watch the scores climb — the same loop that took todo-app 60→125.

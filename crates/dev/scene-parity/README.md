# scene-parity — structural-op golden records (P1 exit gate)

This crate is the **exit gate for phase P1** of the idea-lite core
migration (`~/.claude/plans/idea-lite-core-migration.md`, §10): it pins
the EXACT structural-op sequences the current walker
(`crates/runtime/core/src/walker/{when_switch,each,dynamic,view}.rs`)
emits for a fixed scenario suite. The new scene core
(`runtime-scene`'s `Dyn`/`Keyed` drivers + `realize_children_into`) must
reproduce these sequences — same ops, same order, same indices — before
the walker port is considered done.

"Structural ops" means the 7-method `Host` seam from the plan's §4:

| golden line                      | old `Backend` call        | new `Host` call   |
|----------------------------------|---------------------------|-------------------|
| `create nK view/text/button/...` | `create_*`                | handler mount     |
| `create nK anchor`               | `create_reactive_anchor`  | `create_anchor`   |
| `insert p <- c`                  | `insert`                  | `insert`          |
| `insert_many p <- [..]`          | `insert_many`             | `insert_many`     |
| `insert_at p <- c @ i`           | `insert_at`               | `insert_at`       |
| `remove_child p -x c`            | `remove_child`            | `remove_child`    |
| `clear_children n`               | `clear_children`          | `clear_children`  |

Prop/style/handler calls (`update_text`, `apply_style`,
`install_touch_handler`, …) are deliberately **not** recorded — they are
P2's (capability-trait) concern, and excluding them keeps these goldens
stable across styling churn in the old core.

`cleanup <label>` lines are markers fired by `on_cleanup` hooks the
scenarios register inside branch/row scopes. They pin **dispose
ordering** relative to the structural ops (see `dispose_order_*`).

## Layout

- `src/lib.rs` — `ParityBackend` (recording `Backend`, structural ops
  only, `n0, n1, …` creation-order node names), the `Scenario`/`Cx`
  harness, golden serialization + comparison.
- `src/scenarios.rs` — the scenario registry. Each golden's header
  repeats the scenario's `about` text, so every `.golden` file is
  self-describing.
- `src/new_core.rs` — the NEW-core harness mode (P1): `SceneHost` (a
  recording `runtime_scene::Host` with the same op format + naming), the
  `NcView`/`NcText` test vocabulary + registry, the `NewCx` driver
  (`step` flushes the `World`), and the normalization/override machinery
  for the sanctioned divergences below.
- `src/scenarios_new.rs` — the same 13 scenarios re-targeted at
  `runtime-scene` per the contract below (same names, modes, labels,
  mutations; new element constructors + `runtime-world` signals).
- `tests/goldens.rs` — old core: one test per (scenario, mode) pair + a
  registry↔disk sync check.
- `tests/goldens_new_core.rs` — new core: the SAME pairs against the
  SAME goldens, plus registry-mirror and override-set sync checks.
- `goldens/*.golden` — the pinned sequences, one file per
  `(scenario, mode)`. Owned by the OLD core (matched byte-for-byte).
- `goldens_newcore/*.golden` — explicit new-core override files, ONLY
  for the closed sanctioned-divergence set (`NEWCORE_OVERRIDES`);
  regenerate with `UPDATE_NEWCORE_GOLDENS=1 cargo test -p scene-parity`.

Both structural strategies are covered per scenario where they exist:

- **anchored** (`supports_child_splice() = false`) — reactive regions
  nest under a `create_reactive_anchor`; swaps are `clear_children` +
  `insert`. (SSR, web-hydration, non-splicing native.)
- **spliced** (`supports_child_splice() = true`) — style-less regions
  splice directly into the real parent via `remove_child` +
  `insert_at(base_index)`. (Web CSR, splice-capable native.)

## Writing / re-targeting scenarios

Scenario bodies are intentionally dumb: build a tree through
runtime-core's **public** constructors (`view`/`text`/`when`/`switch`/
`dynamic`/`fragment`/`each_keyed`), mutate plain `Signal`s in `step()`
closures, and let the harness snapshot. No walker internals, no
old-core-specific cleverness. Re-targeting the suite at the new core
means swapping the internals of `Cx::mount` (realize against the new
`Host`-implementing recorder) and adding a `world.flush()` at the end of
`Cx::step` (the staged-commit core is not synchronous) — the scenario
bodies themselves must not change.

**Status: re-targeted (P1).** `src/scenarios_new.rs` carries the suite
against `runtime-scene`, body-for-body: same names, same modes, same
step labels, same signal mutations. The only mappings beyond the mount
internals + per-step flush the paragraph above anticipated:

- constructor lowering — `when`→`dyn_keyed(cond, …)` (guarded hole),
  typed `switch`→`dyn_keyed(discriminant, …)`, `dynamic`→`dyn_element`,
  `each_keyed`→`keyed(items, key, render)`, multi-node rows → a row that
  renders a `fragment`;
- cleanup markers — the kernel's `on_cleanup` needs a running effect, so
  scenarios register a probe `effect` returning the marker closure; it
  fires at the same observable moment (subtree-`Owned` drop).

## Regenerating goldens

```
UPDATE_GOLDENS=1 cargo test -p scene-parity
```

Only do this when the walker's behavior changed **intentionally**, and
review the golden diff as part of the change. A mismatch without an
intentional behavior change is a regression — fix the code, not the
golden.

## Where the new core is ALLOWED to differ

These are the known, deliberate divergence points. Everything not
listed here is a hard invariant.

1. **Anchor/node naming.** `n0, n1, …` are creation-order names minted
   by the recorder, not backend state. The new core must preserve
   *relative creation order and op targets*, but a comparison layer may
   rename freely (e.g. if the new core creates the anchor lazily,
   ids shift — compare shapes, not literal ids, when that lands).
   *Resolution:* the new drivers create anchors eagerly in the same
   creation order, so names align and no renaming layer was needed.
2. **First-fire `clear_children` on a fresh anchor.** The anchored
   `when`/`switch`/`dynamic`/`each` paths call `clear_children` on the
   just-created, still-empty anchor during the first Effect fire (see
   `when_toggle.anchored.golden`'s mount step). It's a harmless no-op
   the old code doesn't special-case. The new `Dyn` driver may skip it;
   if it does, regenerate the anchored goldens against the new
   sequence and note the delta in the migration PR.
   *Resolution:* the new drivers skip the virgin-anchor clear, but the
   shared goldens were NOT regenerated (the old core must keep matching
   them byte-for-byte while both cores are in-tree). Instead
   `new_core::normalize` strips exactly this pattern — `clear_children
   nX` immediately following `create nX anchor` — from both sides of
   the new-core comparison. The adjacency makes the pattern
   unambiguous; ids don't shift because `clear_children` mints nothing.
3. **Cross-effect firing order after an unsubscribe.** In
   `nested_when_in_each_row.spliced.golden`'s final step, row-3's
   `when` fires BEFORE row-2's: the old arena's subscriber list
   swap-removes freed subscriptions, so notification order stops being
   creation order once anything unsubscribed. The hard invariant is
   each row's own op sub-sequence (remove → create → insert_at) and
   that removed rows never fire; the *interleaving across sibling
   effects* follows the kernel's notification order and may differ
   under `runtime-world`'s scheduler.
   *Resolution:* `runtime-world`'s `retain`-based unsubscription keeps
   creation order, so exactly this one (scenario, mode) diverges (row-2
   then row-3, node numbering following suit). Its new-core expectation
   is an explicit override file in `goldens_newcore/`; the override set
   is closed (`new_core::NEWCORE_OVERRIDES`) and disk-synced by test.
4. **Owner-drop teardown.** Scenarios snapshot mutation steps only; the
   final unmount (dropping the `Owner`) is intentionally outside the
   goldens — drop-as-teardown in the new core is mechanically different
   by design. *Resolution:* the new runner drops the `Realized`, then
   the `World`, after the last snapshot.

Anything else — splice indices (`base_index` math through fragments),
the survivors-don't-move optimization, the last_active dedup guard, the
anchored-vs-spliced **opposite** dispose orderings (anchored: scope-drop
THEN `clear_children`; spliced: `remove_child` THEN scope-drop), full
per-step sequences — is the contract.

## Excluded from this suite (and why)

- **`presence`** — exit animations need scheduler/time hooks
  (`after_ms`, animation frames); out of scope for P1 *structural*
  parity. The retire-hook design (plan §4) is validated in P4 when the
  presence handler lands on the new contract.
- **Hydration-mode `when`/`switch`** — needs the web backend's
  hydration cursor (`is_hydrating` + adopt semantics); pinned separately
  by `crates/runtime/core/tests/walker/hydration.rs` and gated at P3.
- **Batched-Repeat fast path** (`execute_batch_with_attach`) — an
  optimization contract of the batching seam, not of the 7 structural
  ops; covered by `crates/runtime/core/tests/walker/batched_repeat.rs`.
- **Virtualizer** — row mount/release is driven by the platform from
  scroll/rAF, outside the structural walker; it has its own capability
  contract (plan §11).

## Running

```
cargo test -p scene-parity
```

41 tests across two binaries:

- `goldens` (old core): 19 golden comparisons + 1 registry↔disk sync
  check — pins the walker byte-for-byte.
- `goldens_new_core` (new core, the P1 gate): the same 19 pairs against
  the same goldens (modulo the sanctioned divergences above) + the
  registry-mirror check + the override-set sync check.

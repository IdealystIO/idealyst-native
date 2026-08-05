# Frozen old-core command streams (Roku backend)

The serialized `RokuCommand` streams the **old core** (`runtime_core::mount`,
the render walker — now deleted) emitted for each scene in
[`../newcore_parity.rs`](../newcore_parity.rs), indented for review. Each
`*.json` is the reference for a stream-parity gate; the test compares
this backend's drained + serialized stream against it byte-for-byte.

There is no BrightScript thin client in-tree, so **the stream IS the
observable output** for this backend — these files are its equivalent of
a screenshot.

## Why they exist

`newcore_parity.rs` originally proved parity by rendering the same scene
on both cores *in one process* and comparing the results. That assertion
could not survive the deletion of `runtime-core`, so the old core's output
was frozen to disk while it still existed. **The old core is now gone**:
these files are the only surviving record of what it produced, and the
comparison against them is the whole gate.

## Corpus

| File | Scene |
| --- | --- |
| `full_scene.json` | torture scene — every primitive family the backend implements: view/text (styled + flex), a `state hovered` sheet, button, toggle, slider (range + step), text input (placeholder), pressable, image (src + alt), icon, activity indicator, scroll view |
| `portal.json` | raw viewport portal behind a reactive branch (create + release inside the dispose window) |
| `dyn_branch_then.json` / `dyn_branch_else.json` | reactive branch, both committed initial states |
| `keyed_list_mount.json` | keyed rows, mount stream |
| `keyed_list_reorder_follow_up.json` | the follow-up stream after a device-event-driven reorder (button handler sets the items signal) — old = synchronous walker reconcile, new = staged write committed by `settle()` |
| `counter_mount.json` / `counter_follow_up.json` | button press → staged write → `settle()` → emitted commands (the embedder boundary) |

`full_scene.json` already exercises every caps family this backend
implements, so no extra breadth scene was needed here (contrast the CPU
and terminal corpora, which gained one).

## The two normalizations, and why neither is a loosening

1. **Sanctioned divergence #2 — virgin-anchor `ClearChildren`.** The
   frozen artifact is the old stream with
   `normalize_sanctioned_old(..)` already applied: the old walker emits a
   no-op `ClearChildren` on a reactive anchor it just created and never
   inserted into, and the new core deliberately skips it
   (`docs/migrating-to-runtime-v2.md` → "What is guaranteed", class 2;
   `crates/dev/scene-parity/README.md`). This is the SAME normalization
   the in-process compare uses today, applied to the SAME side (old
   only) — carried over verbatim, not widened. Roku is the first
   stream-visible instance of that class.

2. **`cache_key` interning (artifact serialization, not a divergence).**
   `CreateIcon`/`UpdateIconData` carry a `cache_key` derived from the
   icon's `paths` static ADDRESS (`src/lib.rs`:
   `data.paths.as_ptr() as u64 ^ filled`). Stable within a process — the
   in-process old-vs-new compare used the RAW value and therefore pinned
   its identity across cores — but it changes between processes under
   ASLR, so
   a raw value cannot be frozen. Each distinct key becomes `#0`, `#1`, …
   in first-appearance order, which preserves everything the value means
   to a consumer (same path set ⇒ same key; `filled` variants ⇒
   different keys; ordering of first use). No behavioral difference is
   hidden by this.

**Do not add a third normalization.** Anything else that differs between
the frozen stream and the new core's stream is a bug.

## Regenerating

```bash
IDEALYST_FREEZE_GOLDENS=1 cargo test -p backend-roku --test newcore_parity
```

**This is no longer a regeneration — it is a RE-BASELINE.** `runtime-core`
is deleted, so the only thing this command can do is overwrite the corpus
with **the current renderer's output**, permanently discarding the old
core's testimony with no way to recover it. So:

- A failure here is a **new-core bug** unless the difference is an
  already-sanctioned divergence. Fix the code.
- Never re-baseline to make a red test green.
- If a re-baseline is genuinely intended (a deliberate protocol change),
  review the JSON diff as the substance of the change and say so in the
  commit message.

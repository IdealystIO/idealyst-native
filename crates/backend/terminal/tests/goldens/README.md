# Frozen old-core grid dumps (terminal backend)

Cell-exact serializations of what the **old core** (`runtime_core::mount`,
the render walker — now deleted) painted for each scene in
[`../newcore_parity.rs`](../newcore_parity.rs). Each `*.grid` is the
reference for a grid-parity gate; the test compares this backend's
`render_to_grid()` output against it cell-for-cell.

## Format

Lossless and reviewable — nothing is normalized away:

```
cols=48 rows=10
r00 glyph |link text                                       |
r00 fg    9*dcdcdcff 39*-
r00 bg    44*101020ff 4*-
```

- `glyph` — one line per row, `|`-fenced so trailing spaces survive.
  Control characters and `\0` render as a space (matching the host's own
  `grid_to_rows` diagnostic).
- `fg` / `bg` — run-length-encoded `rrggbbaa` per cell, `-` for `None`.
  `9*dcdcdcff` = nine consecutive cells with that colour.

This carries exactly the information the gate compares (glyph + fg + bg
per cell).

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
| `full_scene.grid` | torture scene: styled views, text, button chrome `[ label ]`, toggle glyph, pressable, coloured backgrounds |
| `caps_breadth.grid` | **coverage-breadth scene**: image, icon, `link` (a real `NodeKind::Pressable` here — NOT the trait default), activity indicator, controlled text input, `scroll_view` clipping an oversized child, plus `slider` and `text_area` which this backend does NOT implement and therefore resolve to **trait defaults** (visible in the dump as the `[external "… not supported in terminal]` placeholder rows) |

`caps_breadth.grid` exists specifically to guard the de-trait pass. The
backend's caps impls used to UFCS-delegate to
`<TerminalBackend as Backend>::method`, so any method the backend never
overrode resolved to a **`Backend` trait default**; now they resolve to
**caps-trait defaults** instead. A frozen grid that actually paints the
default's output makes a silently-differing default fail loudly. See
`docs/runtime-v2-deletion-baseline.md` for the per-backend list of
default-resolved methods.

## Regenerating

```bash
IDEALYST_FREEZE_GOLDENS=1 cargo test -p backend-terminal --test newcore_parity
```

**This is no longer a regeneration — it is a RE-BASELINE.** The old core
is deleted, so the only thing this command can do is overwrite the corpus
with **the current renderer's output**, permanently discarding the old
core's testimony with no way to recover it. So:

- A failure here is a **new-core bug** unless the difference is a
  divergence already sanctioned in `docs/migrating-to-runtime-v2.md`
  ("What is guaranteed"). Fix the code.
- Never re-baseline to make a red test green.
- If a re-baseline is genuinely intended (a deliberate rendering
  change), review the dump diff as the substance of the change and say
  so in the commit message.

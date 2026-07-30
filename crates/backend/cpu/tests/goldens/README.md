# Frozen old-core framebuffers (CPU rasterizer)

Lossless RGBA8 PNGs of what the **old core** (`runtime_core::mount`, the
render walker — now deleted) painted for each scene in
[`../newcore_parity.rs`](../newcore_parity.rs). Each `*.png` is the
reference for a pixel-parity gate; the test compares this backend's
`MemSurface` framebuffer against it pixel-for-pixel
(120×80, alpha included).

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
| `styled_view_tree.png` | nested views: backgrounds, per-side borders + colors, corner radii, linear gradient, opacity compositing, static translate |
| `text_scale.png` | 8×8 bitmap-font text at default scale and `font_size: 16px` (scale 2), with fg colors |
| `button_pressable_scroll.png` | button label paint, pressable, `scroll_view` clipping an oversized child |
| `dyn_branch_on.png` / `dyn_branch_off.png` | reactive branch (`when` / `dyn_keyed`) between static siblings, both committed initial states |
| `keyed_list.png` | keyed list (`each_keyed` / `keyed`) with per-key row size + color |
| `caps_breadth_leaves.png` | **coverage-breadth scene**: image, icon, activity indicator, controlled text input, text area, toggle, slider — plus a `link`, which this backend does NOT implement and therefore resolves to a **trait default** (`create_link` → `create_view`) |
| `click_before.png` / `click_after.png` | the click cycle: initial paint, then the paint after `dispatch_click` → staged write → flush |

`caps_breadth_leaves.png` exists specifically to guard the de-trait
pass. The backend's caps impls used to UFCS-delegate to
`<CpuBackend as Backend>::method`, so any method the backend never
overrode resolved to a **`Backend` trait default**; now they resolve to
**caps-trait defaults** instead. A frozen frame that actually paints
those primitives makes a silently-differing default fail loudly. See
`docs/runtime-v2-deletion-baseline.md` for the per-backend list of
default-resolved methods.

## Regenerating

```bash
IDEALYST_FREEZE_GOLDENS=1 cargo test -p backend-cpu --test newcore_parity
```

**This is no longer a regeneration — it is a RE-BASELINE.** `runtime-core`
is deleted, so the only thing this command can do is overwrite the corpus
with **the current renderer's output**, permanently discarding the old
core's testimony with no way to recover it. So:

- A failure here is a **new-core bug** unless the difference is a
  divergence already sanctioned in
  `docs/migrating-to-runtime-v2.md` ("What is guaranteed"). Fix the
  code.
- Never re-baseline to make a red test green.
- If a re-baseline is genuinely intended (a deliberate rendering
  change), review the PNG diff as the substance of the change and say so
  in the commit message.

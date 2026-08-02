# Frozen old-core SSR output

The exact `html` and `head_css` the **old core** (`backend_ssr::render_path`,
walker + `Element` — now deleted) emitted for each corpus item in
[`../newcore_byte_identity.rs`](../newcore_byte_identity.rs). Each pair
(`<item>.html`, `<item>.head.css`) is the reference for a byte-identity
gate; the test compares this backend's
(`backend_ssr::newcore::render_path`) output against it byte-for-byte.

**Zero normalization**, on either the frozen or the actual side. This is
the hydration acceptance gate: the web backend's adopt-mode boot walks SSR
DOM cursor-style in creation order and adopted old-core output, so
byte-identical output adopts identically. Any divergence found here is a
hydration bug, not a formatting nit — do NOT normalize differences away.

## Why they exist

`newcore_byte_identity.rs` originally proved parity by rendering the same scene
on both cores *in one process* and comparing the results. That assertion
could not survive the deletion of `runtime-core`, so the old core's output
was frozen to disk while it still existed. **The old core is now gone**:
these files are the only surviving record of what it produced, and the
comparison against them is the whole gate.

## Corpus

| Stem | Item |
| --- | --- |
| `static_kitchen_sink` | static tree touching all 13 core primitives with static styles |
| `styled_sheet_tokens` | `stylesheet!`-shaped sheet: token-referencing base, a `size` variant axis with a defaulted arm, and a `state hovered` overlay (lowered to a `:hover` pseudo rule in `head_css`) |
| `dyn_branch_then` / `dyn_branch_else` | reactive branch, both committed initial states |
| `keyed_list` | keyed list |
| `styled_text_runs` | styled-text runs with token colours |
| `swap_navigator` / `swap_navigator__about` | swap navigator with author chrome, rendered at `/` and `/about` |
| `render_all` / `render_all__about` / `render_all__docs` | the SSG `render_all` crawl's pages (route discovery + parameterized-route skip are asserted separately, in-test) |
| `default_font_fill` | the walker's asymmetric default-text-font fill contract: STATIC applications fold the theme default into font-less rules, DYNAMIC (reactive) ones do not. The new core briefly folded on the dynamic path too, minting class hashes old-core SSR never mints and breaking website SSG byte-parity site-wide — this pair is the regression net for that whole class of drift. **Amended 2026-08-02** (the one deliberate re-baseline in this corpus): `default_font_fill.head.css` now carries `:root { --iy-default-font: …; font-family: var(--iy-default-font); }`. The old core's frozen output pinned a live-path bug — the document-level font publication was gated on premint use, so non-preminted builds published no document font and every reactively-styled node (plus `<body>` and plain containers) fell back to the browser serif. The `.html` half is byte-identical to the old core's — no minted class hash moved, which is the point: the fix supplies the font by document-root inheritance precisely so the dynamic path keeps not folding. Pinned by `regression_reactive_styled_node_inherits_theme_font` here and `regression_live_world_publishes_the_default_text_font` in runtime-vocabulary; the fold asymmetry itself is still asserted in-test (`folds == 2`), independent of the frozen bytes. |

Minted class hashes are content-derived and therefore process-stable, so
they are frozen as-is (verified: a freeze run followed by a plain run is
green).

`static_kitchen_sink` already reaches every `create_*` this backend
implements, so no extra breadth scene was added here. The rest of SSR's
default-resolved surface is imperative-handle and no-op plumbing with no
serialized output — see `docs/runtime-v2-deletion-baseline.md`.

## Regenerating

```bash
IDEALYST_FREEZE_GOLDENS=1 cargo test -p backend-ssr --test newcore_byte_identity
```

**This is no longer a regeneration — it is a RE-BASELINE.** `runtime-core`
is deleted, so the only thing this command can do is overwrite the corpus
with **the current renderer's output**, permanently discarding the old
core's testimony with no way to recover it. So:

- A failure here is a **new-core bug** (and specifically a hydration
  bug). Fix the code.
- Never re-baseline to make a red test green.
- If a re-baseline is genuinely intended, review the HTML/CSS diff as the
  substance of the change and say so in the commit message.

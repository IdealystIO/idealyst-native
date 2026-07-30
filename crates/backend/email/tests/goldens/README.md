# Frozen old-core email output

The exact bytes the **old core** (`backend_email::render_email`, walker +
`Element`) emitted for each corpus item in
[`../newcore_golden.rs`](../newcore_golden.rs):

- `<item>.html` — the inline-styled, email-safe HTML document.
- `<item>.txt` — a `subject: …` header line, `---`, then the plaintext
  alternative. One file for both non-HTML halves of a `RenderedEmail`;
  the subject is `Debug`-formatted so `Some("")` and `None`
  (`<none>`) are distinguishable.

The test compares this backend's
(`backend_email::newcore::render_email`) output against these
byte-for-byte, with **zero normalization** on either side.

## Why they exist

`newcore_golden.rs` originally proved parity by rendering the same scene
on both cores *in one process* and comparing the results. That assertion
could not survive the deletion of `runtime-core`, so the old core's output
was frozen to disk while it still existed. **The old core is now gone**:
these files are the only surviving record of what it produced, and the
comparison against them is the whole gate.

## Corpus

| Stem | Item |
| --- | --- |
| `static_styled_tree` | static tree with inline-style-lowered rules |
| `tokens_and_dropped_overlays` | theme tokens resolved to literals, and the overlays email deliberately drops |
| `setup_app_background` | the `setup(&mut EmailBackend)` seam's app-background application |
| `dyn_branch_then` / `dyn_branch_else` | reactive branch, both committed initial states |
| `idea_ui_mail_welcome` | **headline**: the real idea-ui-mail welcome template (body → container → section → heading, text, CTA button, divider, text) — see above |

The corpus is also what found a **real bug in both cores**: the new
core's theme-cohort reapply on an in-build `install_tokens` re-delivers
every node's rules, and email's append-only inline model duplicated the
CSS. Fixed by `push_style_dedup` (value-identical re-application is a
no-op) in `src/lib.rs`, with a unit regression there.

## Regenerating

```bash
IDEALYST_FREEZE_GOLDENS=1 cargo test -p backend-email --test newcore_golden
```

**This is no longer a regeneration — it is a RE-BASELINE.**
`runtime-core` is deleted, and with it the idea-ui-mail dev-dep that
rendered the headline item from the REAL components. The only thing this
command can do now is overwrite the corpus with **the current
renderer's output**, permanently discarding the old core's testimony
with no way to recover it. So:

- A failure here is a **new-core bug** unless the difference is a
  divergence already sanctioned in `docs/migrating-to-runtime-v2.md`
  ("What is guaranteed"). Fix the code.
- Never re-baseline to make a red test green.
- **When the idea-ui-mail template itself changes**, the replica in
  `newcore_golden.rs` must change in lockstep AND
  `idea_ui_mail_welcome.*` must be re-frozen. Before the deletion, do
  that with a freeze run. After the deletion, the replica IS the
  template's only in-tree definition for this test — say so explicitly
  in the commit message.

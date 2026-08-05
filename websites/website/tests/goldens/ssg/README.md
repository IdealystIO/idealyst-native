# Frozen SSG corpus (the whole website)

The complete SSG output of the **old core** for
[`../../ssg_parity.rs`](../../ssg_parity.rs): every one of the website's
33 literal routes as `<stem>.html` + `<stem>.head.css`, plus
`served.doc.html` (the `/quickstart` document served over real HTTP
through the production per-request path), plus `MANIFEST.txt` (the file
set — written last so a partial capture can never be used as a
baseline).

Produced by `backend_ssr::render_all` / `backend_ssr::serve` with the old
render walker. ~1.7 MB across 67 files.

## Why it exists

`ssg_parity.rs` used to compare two ephemeral dumps — one per core —
under `target/…/website-ssg/{oldcore,newcore}/`, a gate that only bit
when both legs had run. The old-core leg died with wave 2c's deletion of
`runtime-core`, and the ephemeral dump went with it. This directory is
the same corpus made permanent, so the gate keeps biting afterwards.

This is the broadest SSR/hydration coverage in the tree — 33 pages of a
real application, not synthetic fixtures — and byte-identity against it
is the website's hydration acceptance proof (the web adopt-mode boot
walks SSR DOM in creation order, so byte-identical server output adopts
identically).

## The one tolerated divergence (unchanged, not widened)

The comparison runs through `assert_bytes`: **strict byte equality
first**, and only if the two sides become identical after collapsing
`<div style="display: contents">` reactive-anchor wrappers on BOTH sides
is the difference accepted.

That covers exactly one thing: the renderer expresses `presence` as a
standard Dyn hole, which on an anchored host nests under a
`display: contents` anchor its own hydration boot adopts; the old walker
managed the presence swap imperatively with no anchor. Each side's SSR
output matches its own client's adoption contract and `display: contents`
is layout-inert. In practice it fires on exactly one page
(`primitives.html`, the overlay/presence demos) and the test logs when it
does.

Nothing else is normalized. A style-resolution drift (this gate already
caught the default-text-font fold-in, which shifted minted class hashes
site-wide) fails loudly.

## Running the gate

```bash
cargo test -p website --features ssr --test ssg_parity
```

## This corpus cannot be regenerated

`runtime-core` is gone. The freeze half of `ssg_parity.rs` (which only
the old-core leg was ever allowed to run) went with it, and the only
thing a "regeneration" could do now is re-baseline the corpus against
the current renderer's output — permanently discarding the old walker's
testimony, with no way to recover it. So:

- A failure here is a **bug**, and specifically a hydration bug. Fix the
  code.
- Never re-baseline to make a red test green.
- Adding or removing a website route changes the file set and trips the
  manifest assertion. That IS a deliberate re-baseline — do it in the
  same change as the route, review which pages' bytes moved, and say so
  in the commit message.
- **Editing a page's own copy** is the other deliberate case: the source
  text changed, so the frozen bytes must follow. Same discipline —
  review the diff before re-baselining and confirm it is confined to the
  text you edited.

### How to tell a content change from a renderer regression

The corpus is minted class hashes plus text. A pure copy edit moves
**text only**: the `<stem>.head.css` files stay byte-identical and the
set of `ui-…` class names used by the page is unchanged. A renderer or
style-resolution drift does the opposite — it moves `head.css` and/or
mints new class hashes (the default-text-font fold-in shifted them
site-wide). Check both before you re-baseline; if `head.css` moved, it is
not a copy edit.

## Deliberate re-baselines

| When | Pages | Why |
| --- | --- | --- |
| runtime-v2 deletion close-out | `code-splitting`, `further-reading`, `concepts`, `backends`, `targets`, `features__cross-platform`, `features__extensibility`, `comparisons__flutter` | Prose corrected for the v2 deletion. These pages taught deleted API as if it were current — `Bound<H>`, the 159-method `Backend` mega-trait, `Element::External`, the render walker, and `defer_external_registration`/`register_lazy()` on the code-splitting page. Text-only: no `head.css` moved and no class hash was added or dropped on any of the eight. |
| 2026-08-03 lazy-chunk filename correction | `code-splitting` | The page claimed chunk files are named after the component (`…_lazy_Editor.wasm`); they are emitted as `module_<n>___lazy_body.wasm` with the component's readable name in the `__wasm_split.js` loader symbol — the copy was corrected when the docs drift was found during the per-page-lazy work. Text-only, verified per the discipline above: `code-splitting.head.css` byte-identical, `ui-…` class set unchanged, and the stripped-text diff is exactly the corrected sentence. |
| 2026-08-02 default-font publication + ControlRow focus relocation | every page (34 `html` + 33 `head.css`) | Two sanctioned style divergences, both verified rule-by-rule before re-baselining (the union diff across the whole corpus is exactly these two): **(a)** every `head.css` gains `:root { --iy-default-font: …; font-family: var(--iy-default-font); }`. The frozen output pinned a live-path rendering bug — `apply_default_text_font` was gated on `premint_used`, so non-preminted builds published no document font and `<body>`, plain containers, and every reactively-styled node fell back to the browser serif (`runtime-vocabulary` `regression_live_world_publishes_the_default_text_font`, `backend-ssr` `regression_reactive_styled_node_inherits_theme_font`). No minted class hash moved for this half — the fix is document-level inheritance precisely so the reactive path keeps not folding. **(b)** one node per page (the sidebar "Dark mode" `Switch` row, `ControlRow`): `ui-59e3ff6bae21aa75` → `ui-4972737004f74596`, dropping the row's `:focus` ring rule and border-color transition. That is commit `432675b7`'s deliberate design change — the focus ring moved off the box+label row onto the control itself, pinned by its own regression tests (`regression_focus_ring_rings_the_box_not_the_label_row` and the Radio/Switch mirrors) — which landed hours after the corpus freeze without re-baselining, because this gate is behind `--features ssr` and outside the default workspace run. Two pages carry the same classes in other clothing: `served.doc.html` embeds the head CSS inline, so (a) and (b) appear inside its `<style>` block; `primitives.html` additionally now stores the presence `display: contents` anchor verbatim — the one divergence the gate has always tolerated by normalization (see above), baked in by this re-baseline so strict byte equality passes without invoking the tolerance. |

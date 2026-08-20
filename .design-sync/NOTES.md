# design-sync notes — idea-ui → claude.ai/design

## Why this repo is off the skill's normal path

`/design-sync`'s two converters (storybook, package) both assume a JS/TS
design system with a `dist/` that esbuild can bundle into React components.
idea-ui is Rust: components are `#[component]` fns compiled to wasm, and there
is no JS artifact to ship. So the layout is produced by a converter written
for this repo. `shape` in `config.json` is `custom-rust` for that reason.

## The pipeline

Two stages. Rebuild with both, in order:

```sh
RUSTFLAGS="--cfg idealyst_premint --cfg idealyst_premint_dump" \
  cargo run -p idea-ui --features style-dump,catalog \
  --example design_sync -- /tmp/ds-out

node .design-sync/build-bundle.mjs /tmp/ds-out ds-bundle
```

1. **`crates/ui/idea-ui/examples/design_sync.rs`** — emits `tokens.css`,
   `components.css`, `manifest.json`, `recipes.json`, `runtime.css`.
2. **`.design-sync/build-bundle.mjs`** — assembles the upload bundle.

`_vendor/react.js` and `_vendor/react-dom.js` are built once with the esbuild
already vendored at
`crates/tools/cli/examples/external-export-suite/consumers/react`
(React 19 ships no UMD build, so they are bundled to `window.React` /
`window.ReactDOM`). They only change when React does; the bundle script does
not rebuild them.

## Things that cost a debugging cycle

- **Both cfgs are required, and they do different jobs.** `idealyst_premint_dump`
  makes `stylesheet!` register into `PREMINT_SHEETS` at link time.
  `idealyst_premint` makes a `StyleApplication` *attach* as a preminted class
  stamp. With only the first, the render emits live-engine `ui-<hash>` classes
  that the dumped `iy-*` CSS has no rules for.
- **Render before dumping.** Sheets reach the registry two ways: `stylesheet!`
  at link time, and `premint_as` when a component assembles its sheet at
  *mount* (Select's dropdown, Slider's container, Table's cells, Toast). The
  converter renders every recipe first with the minted-class guard disarmed,
  then dumps, then re-renders with the guard armed. Skipping the first pass
  cost ~11 KB of CSS and produced the framework's "no CSS in the shipped
  asset" warning. All three stages must be one process — the registry is
  process-local.
- **`render_to_string` drops `head_css`.** Applications carrying overrides or
  a computed layer can't premint, so their rules land in
  `RenderedPage::head_css` instead of the asset. Dropping it collapsed Table's
  columns to min-content. The converter uses `render_path` and writes the
  remainder to `runtime.css`, which `styles.css` imports.
- **Only the root node's sheet may contribute props.** Parameterising nested
  nodes made Card and Stack inherit the Typography axes (`kind`, `weight`,
  `align`) of the text inside their own recipe, and same-named axes from
  different sheets collided — `Stack.align` documented Typography's
  `left | right | center | justify` instead of its own flex alignment.
- **Don't look an axis up by name across the manifest.** It returns whichever
  sheet declares it first; Button's docs quoted the macro sheet's
  `primary_solid` instead of the `primary_filled` its markup actually stamps.
  Carry the sheet entry the node matched.
- **Preview cells reproduce idea-ui's container model** (flex column,
  `align-items: stretch`). Components therefore fill the cell — that is real
  idea-ui behaviour. Switching to `flex-start` to make buttons look
  intrinsically sized collapses Table and Grid.

## Known gaps

- **39 of 47 components** are exported — the ones with a `recipe!` in
  `crates/ui/idea-ui/src/recipes.rs`. Missing: Autocomplete, Calendar,
  DateInput, DatePicker, SegmentedControl, TimeInput, TypedField, Toast,
  Surface, MenuPanel. Adding a recipe there exports the component on the next
  run — no converter change needed.
- **Shells cover style axes + `children` + `className`,** not behavioural props
  (`on_click`, `value`, `bind_to`). They are for composing on-brand layouts in
  the design tool; the Rust component is the real API.
- `components.css` carries one sheet from **idea-theme's example app-theme
  recipe** (`--app-surface` / `--app-text`). It is documentation, not part of
  idea-ui, and its rules carry literal fallbacks so nothing renders wrong.

## Re-syncing

`.design-sync/config.json` pins the project
(`f82ba3fb-ebd8-4ceb-b1cc-a13ac9977f60`). `ds-bundle/_ds_sync.json` holds
per-path content hashes; diff against a fresh build to find what changed.
`ds-bundle/` is gitignored — it is regenerable output.

`conventions.md` is prepended to the uploaded README and is what the design
agent reads. It is hand-authored and human-editable; the generator only
stitches it in. **Every class, token, axis value and component name in it was
validated against the built artifacts** — re-run that check after changing the
theme, and fix or cut any name that no longer verifies.

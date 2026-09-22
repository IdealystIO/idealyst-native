# `wasm-split` — vendored code-splitting toolchain

A snapshot of the wasm-split crates from DioxusLabs/dioxus (alpha-0.8.0),
owned here so bugs can be fixed as they fire on this codebase without
waiting on upstream. Four crates:

| Crate | Role |
| --- | --- |
| `wasm-split` | The runtime side: the `LazyLoader` / `LazySplitLoader` that a split call site awaits. Re-exported to authors as `runtime_core::__wasm_split`. |
| `wasm-split-macro` | The `#[wasm_split(module)]` attribute. `#[component(lazy)]` emits it around each lazy body on wasm32; native builds never see it. |
| `wasm-split-cli` | The post-link pass `build-web` runs on the wasm-bindgen output: cuts the module at the split exports and writes one chunk per split module plus the loader JS. |
| `wasm-used` | Liveness analysis the CLI uses to decide what each chunk must carry. |

## How the pieces link up

The macro emits, per split function, an export named
`__wasm_split_00___<module>___00_export_<id>_<fn>` and a matching import
`…00_import_<id>_<fn>`. The CLI pairs import to export by that exact
name, so the `<id>` only has to be the same on both sides of one call
site and different between call sites that share `<module>` and `<fn>`.

## Local changes from the upstream snapshot

- **`wasm-split-macro`: the split export name is derived from
  `(module, fn, source file)`, not from the ident's `Span` debug form.**
  Upstream hashed `format!("{span:?}")`, whose value is the ident's
  absolute byte range in the crate-wide source map. Any edit that changed
  the length of a file parsed *earlier* renamed every split export parsed
  *later*, and each rename is a new `DefPath`: the crate-wide reachability
  set reran, `is_reachable_non_generic` went red for every function, and
  nearly every codegen unit was rebuilt to a byte-identical object. On
  CrewForge (17 lazy areas) a one-byte edit re-codegened 238 of 256 CGUs
  and cost about 7s of LLVM per save on wasm32 while the same edit reused
  255 CGUs on native. The regression test
  `regression_export_ident_stable_across_byte_shift` in the macro crate
  holds the line. Line and column are deliberately not part of the name:
  an edit above a lazy component in its own file must not rename it.
  `Span::file()` comes from proc-macro2's `span-locations` feature.

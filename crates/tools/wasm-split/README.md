# `wasm-split` — code-splitting toolchain

How `#[component(lazy)]` becomes separately loaded wasm on the web. Three
crates:

| Crate | Role |
| --- | --- |
| `wasm-split` | The runtime side: the `LazyLoader` / `LazySplitLoader` that a split call site awaits. Re-exported to authors as `runtime_core::__wasm_split`. Vendored from DioxusLabs/dioxus (alpha-0.8.0). |
| `wasm-split-macro` | The `#[wasm_split(module)]` attribute. `#[component(lazy)]` emits it around each lazy body on wasm32; native builds never see it. Vendored from DioxusLabs/dioxus. |
| `wasm-carve` | The post-link pass `build-web` runs on the wasm-bindgen output: partitions the module at the split exports and writes main, one module per split point, the shared chunk, and the loader JS (`__wasm_split.js`). Ours; it replaced Dioxus's walrus-based `wasm-split-cli`. |

## How the pieces link up

The macro emits, per split function, an export named
`__wasm_split_00___<module>___00_export_<id>_<fn>` and a matching import
`…00_import_<id>_<fn>`. The splitter pairs import to export by that exact
name, so the `<id>` only has to be the same on both sides of one call
site and different between call sites that share `<module>` and `<fn>`.

## wasm-carve

Inputs: the rustc/LLD module (linked with `--emit-relocs`) and the
wasm-bindgen output. The partition — what main reaches, what each split
point reaches, what more than one split reaches (the shared chunk) — comes
from the rustc module's relocations (the only record of function pointers
stored in DATA: vtables, closures) plus the bindgened module's direct
`call` / `ref.func` operands, paired by function name.

No output is built through an instruction IR. Every output keeps the
source's type, table, memory and global numbering, so function bodies are
copied as bytes; only calls inside kept bodies are renumbered, by a
streaming re-encode. Main keeps what its exports, start function and
table reach. A split module holds its own bodies plus a trampoline
(`call_indirect` through a slot appended to the shared table) for every
main function it calls; it imports main's memory, tables and globals and
installs its functions into the table when it loads. It carries no data:
main initializes every data segment, so a split module only re-writes its
own data symbols when `--data-prune` zeroed main's copy of them. Main's split-point
imports become trampolines to the slot each module installs its entry at.

Measured on CrewForge (73 MB bindgened module, 17 split points): 1.5 s and
0.34 GB peak, where the walrus-based splitter took 9.5 s and 3.1 GB —
walrus holds ~60 bytes of IR per byte of code, and the old splitter held
one whole-program parse per output being built.

`wasm-carve/examples/carve.rs` runs the splitter on a real app's modules
outside the build, for measuring.

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

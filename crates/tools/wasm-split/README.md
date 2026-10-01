# `wasm-split` — code-splitting toolchain

How `#[component(lazy)]` becomes separately loaded wasm on the web. Three
crates:

| Crate | Role |
| --- | --- |
| `wasm-split` | The runtime side: the `LazyLoader` / `LazySplitLoader` that a split call site awaits. Re-exported to authors as `runtime_core::__wasm_split`. Vendored from DioxusLabs/dioxus (alpha-0.8.0). |
| `wasm-split-macro` | The `#[wasm_split(module)]` attribute. `#[component(lazy)]` emits it around each lazy body on wasm32; native builds never see it. Vendored from DioxusLabs/dioxus. |
| `wasm-carve` | The post-link pass `build-web` runs on the packaged module (the web-glue pass's output in own mode, wasm-bindgen's in hybrid mode): partitions the module at the split exports and writes main, one module per split point, the shared chunk, and the loader JS (`__wasm_split.js`). Ours; it replaced Dioxus's walrus-based `wasm-split-cli`. |

## How the pieces link up

The macro emits, per split function, an export named
`__wasm_split_00___<module>___00_export_<id>_<fn>` and a matching import
`…00_import_<id>_<fn>`. The splitter pairs import to export by that exact
name, so the `<id>` only has to be the same on both sides of one call
site and different between call sites that share `<module>` and `<fn>`.

## wasm-carve

Inputs: the rustc/LLD module (linked with `--emit-relocs`) and the
packaged module — in own mode (every framework-only app) the web-glue
pass's output, in hybrid mode wasm-bindgen's (still called "bindgened" in
the code). The partition — what main reaches, what each split point
reaches, what more than one split reaches (the shared chunk) — comes from
the rustc module's relocations (the only record of function pointers
stored in DATA: vtables, closures) plus the packaged module's direct
`call` / `ref.func` operands, paired by function name.

**Imports, glue included, stay in main.** A split module has no function
imports: index `i` below the source's import count becomes a defined
trampoline to main's import, reached through the shared table. So a
chunk whose code calls a web-glue binding nothing in main calls still
works in own mode: the binding is an import of main's module (named
`g<N>`, its JS in main's `pkg/<lib>.js`), and the chunk reaches it the way
it reaches any main function. Chunks never import `./__idealyst_glue.js`,
so the split loader supplies no glue namespace and needs none.
`tests/lazy-chunk-handoff` pins it in a browser (a chunk-only binding
renders `glue in chunk: 42`; `cargo run -p prune-regression -- --browser`).
The loader reaches main's instance through `initSync(undefined,
undefined)`, which both modes' `pkg/<lib>.js` answers with the raw exports
once instantiated.

No output is built through an instruction IR. Every output keeps the
source's type, table, memory and global numbering, so function bodies are
copied as bytes; only calls inside kept bodies are renumbered, by a
streaming re-encode. Main keeps what its exports, start function and
table reach. A split module holds its own bodies plus a trampoline
(`call_indirect` through a slot appended to the shared table) for every
main function it calls; it imports main's memory, tables and globals and
installs its functions into the table when it loads.

Data: main initializes every data segment. A release build then zeroes, in
main, every data byte main's code cannot reach (`liveness.rs`): the data
symbols the kept functions reference per the rustc module's relocations,
closed over data→data references, plus constant addresses in the functions
wasm-bindgen wrote (they have no relocations) and in global initializers.
A function main's live code or data holds a table index for keeps its slot
in main. Only bytes no live symbol covers are zeroed — symbols nest. Each
split module puts back the zeroed bytes its own code reads; bytes several
read go to the shared chunk, loaded once before them. A module whose
address relocations point at anything but defined data symbols is not
pruned. Main's split-point
imports become trampolines to the slot each module installs its entry at.

Measured on CrewForge (73 MB bindgened module, 17 split points): 1.5 s and
0.34 GB peak, where the walrus-based splitter took 9.5 s and 3.1 GB —
walrus holds ~60 bytes of IR per byte of code, and the old splitter held
one whole-program parse per output being built.

`wasm-carve/examples/carve.rs` runs the splitter on a real app's modules
outside the build, for measuring.

### Own-bindings passes (not part of splitting)

Two more streaming passes live here because they are the same kind of
byte-range rewrite, for [`docs/proposals/own-web-bindings.md`](../../../docs/proposals/own-web-bindings.md)
(driven by `build_web::own_glue`; every web build runs both):

- `glue.rs` — pulls `web-glue`'s JS out of a linked module (snippets
  carried in `./__idealyst_glue.js` import names, the runtime and crate
  modules in the `__idealyst_glue` custom section), renames those imports
  to `g0…`, strips the section. Re-encodes the import section only. It
  also reports whether the module uses wasm-bindgen (`Glue::wasm_bindgen`:
  a `__wbindgen*` import or the `__wasm_bindgen_unstable` section), which
  is how the build picks own or hybrid mode.
- `command_exports.rs` — repoints exports past LLD's command-export
  wrapper (`call __wasm_call_ctors; forward args; call inner`), so JS →
  Rust calls stop re-running static constructors; `main` keeps its wrapper
  (the one-time run). Only the export section changes. Own mode unwraps
  every export but `main` (`unwrap_command_exports`); hybrid mode only
  web-glue's own `__glue_*` exports (`unwrap_command_exports_where`).
- `glue_js.rs` — the JS: own mode's `pkg/<lib>.js` (`loader_js`), hybrid
  mode's `pkg/__idealyst_glue.js` (`hybrid_glue_js`).

`neutralize.rs` (wasm-bindgen 0.2.122's `*.command_export` wrappers) and
`strand.rs` (the hot-patch base's stranded `__wbindgen_placeholder__`
imports) exist only for wasm-bindgen's output; own-mode builds never run
them.

`wasm-carve/examples/glue_pass.rs` runs both over one module and prints
their cost.

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

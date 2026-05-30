# /demo OOB diagnosis — wasm-bindgen 0.2.122

Status: **not fixed**. Recommendation: **downgrade wasm-bindgen to 0.2.121**.

## Quick summary

`/demo` traps with `RuntimeError: memory access out of bounds`. Home renders fine. The trap chain is **not one bug, it's at least two**, both new in wasm-bindgen 0.2.122:

1. **`command_export` ctor re-run → inventory linked-list corruption.** wasm-bindgen 0.2.122 keeps LLD-emitted `*.command_export` wrappers around its internal helpers (`__wbindgen_malloc/realloc/free/exn_store/destroy_closure`, `__externref_table_alloc/dealloc`). Each wrapper's body is `call __wasm_call_ctors; <forward args>; call <bare>`. The generated JS calls these wrappers on every JS↔wasm round trip. `__wasm_call_ctors` re-runs every module ctor, including every `inventory::submit!`, which double-submits into `inventory`'s global linked list. The next traversal traps OOB. Earlier visible as `inventory::submit ← __ctor ← __wasm_call_ctors ← *.command_export`.
2. **`__externref_table_alloc` reentrant `RefCell::borrow_mut`.** Even with the wrappers neutralized, the bare `__externref_table_alloc` (which `borrow_mut`s wasm-bindgen's `HEAP_SLAB`) is being reentered during walker descent through Pressables — likely because a closure invoke shim allocates an externref slot while a parent frame is mid-borrow. Stack: `RefCell::borrow_mut ← __externref_table_alloc ← closures::1_::invoke (externref shim)`.

**0.2.121 does NOT have either issue.** The 0.2.121 wasm exported `__wbindgen_malloc` (etc.) bare with no wrappers, and the externref alloc didn't reenter on this codebase. The `command_export` LLD wrappers exist in the rustc-emitted wasm in both versions; 0.2.121's wasm-bindgen consumes/elides them, 0.2.122's preserves them.

## What's in the repo right now

### Edits made

- [`crates/tools/wasm-split/wasm-split-cli/src/lib.rs`](crates/tools/wasm-split/wasm-split-cli/src/lib.rs) — added `pub fn neutralize_command_export_wrappers(bindgened: &[u8]) -> Result<Vec<u8>>` with two passes:
  - **Pass A (body strip):** for each function whose internal name ends in `.command_export` AND whose export name (if any) ends in `_command_export` OR is unexported, remove the first `call __wasm_call_ctors` from the entry block. Leaves `main.command_export` and `host_reserve.command_export` alone (their unsuffixed export = legit one-time init).
  - **Pass B (export remap):** for each export named `*_command_export`, repoint at the bare function with the corresponding internal name (e.g. `__wbindgen_malloc_command_export` → `__wbindgen_malloc`).
- Regression tests: `neutralize_command_export_remaps_suffixed_export_to_bare` and `neutralize_command_export_leaves_unsuffixed_exports_alone`. **Both test Pass B only — Pass A is untested.**
- [`crates/tools/build/web/src/lib.rs`](crates/tools/build/web/src/lib.rs) — wired the patch in between `wasm_bindgen_build` and `run_wasm_split`. Emits `[build-web] command_export neutralized (N → M bytes)` line.
- [`Cargo.lock`](Cargo.lock) — bumped wasm-bindgen-family to 0.2.122 / 0.3.99 to match the installed CLI.
- `idealyst` CLI rebuilt via `cargo install --path crates/tools/cli --force`.

### What's known to work

- Export remap (Pass B) works: in `examples/website/pkg/website_bg.wasm`, the seven helper exports point at bare functions. Confirmed via `wasm-objdump -j Export`.
- The first stack-trace flavor (`inventory::submit ← __wasm_call_ctors ← *.command_export`) is **gone** after Pass B.

### What's known to be broken

- **Pass A silently fails.** Disassembly of the served wasm shows 4 functions still containing `call __wasm_call_ctors`:
  - `main.command_export` (intended — legit init)
  - `host_reserve.command_export` (intended — legit init)
  - `__externref_table_alloc.command_export` (BUG — should have been stripped)
  - `__externref_table_dealloc.command_export` (BUG — should have been stripped)

  My walrus IR mutation (`local_func.block_mut(entry).instrs.remove(0)`) didn't take effect on the emitted wasm for the latter two. Either the IR mutation didn't persist through `emit_wasm()`, or my filter logic missed those specific wrappers (e.g., the export-status lookup against `__externref_table_alloc_command_export` failed before Pass B remapped the exports). Pass A needs a regression test and likely a different walrus API approach.

  **Even with Pass A fixed, the second bug (`RefCell::borrow_mut` reentrance) likely persists** — it's a separate wasm-bindgen 0.2.122 issue not caused by the wrappers.

## Recommendation: downgrade to wasm-bindgen 0.2.121

This is the path that gets `/demo` working today with the least risk:

```bash
# 1. Revert the crate-side bump
cargo update -p wasm-bindgen --precise 0.2.121 \
            -p js-sys --precise 0.3.98 \
            -p web-sys --precise 0.3.98 \
            -p wasm-bindgen-futures --precise 0.4.71 \
            -p wasm-bindgen-test --precise 0.3.71

# 2. Match the CLI to the crate
cargo install wasm-bindgen-cli --version 0.2.121 --force

# 3. Optional: remove the now-unnecessary patch
#    (Pass A+B in wasm-split-cli + the call in build/web/lib.rs;
#     0.2.121 has no command_export wrappers to neutralize)

# 4. Rebuild + hard reload
```

The `neutralize_command_export_wrappers` patch is benign with 0.2.121 (it'll find no matching exports and no-op), so step 3 is optional cleanup, not required for the fix. Tests still pass either way.

## If you want to keep 0.2.122

Two issues to fix, separately and in order:

### A. Fix Pass A in `neutralize_command_export_wrappers`

The walrus body-edit code at [wasm-split-cli/src/lib.rs:124-146](crates/tools/wasm-split/wasm-split-cli/src/lib.rs#L124-L146) doesn't actually persist its changes. Verify by adding a test:

```rust
#[test]
fn neutralize_strips_ctor_call_from_helper_wrapper_body() {
    let bytes = build_wrapper_fixture("__wbindgen_malloc_command_export");
    let patched = neutralize_command_export_wrappers(&bytes).unwrap();

    // Re-parse and disassemble the wrapper's body — must NOT call ctors.
    let module = Module::from_buffer(&patched).unwrap();
    let wrapper_fid = module.funcs.iter()
        .find(|f| f.name.as_deref() == Some("__wbindgen_malloc.command_export"))
        .map(|f| f.id()).expect("wrapper still present");
    let func = module.funcs.get(wrapper_fid);
    let FunctionKind::Local(lf) = &func.kind else { panic!() };
    let entry = lf.entry_block();
    let block = lf.block(entry);
    let calls_ctors = block.instrs.iter().any(|(instr, _)| {
        matches!(instr, ir::Instr::Call(c)
                 if module.funcs.get(c.func).name.as_deref()
                    == Some("__wasm_call_ctors"))
    });
    assert!(!calls_ctors, "wrapper body must not call __wasm_call_ctors after the strip");
}
```

This will fail today and let you iterate. The fix is likely one of:
- Re-fetching the block via `local_func.builder_mut().func_body()` instead of `block_mut`.
- Reordering: run Pass A AFTER Pass B (so the export-status filter sees the post-remap state — though this is more about correctness of the filter than the mutation).
- Using walrus's `LocalFunction::block_mut` correctly — possibly need to invalidate a cache.

### B. The `__externref_table_alloc` reentrant borrow

Even with all wrappers neutralized, [externref.rs:130-133 in wasm-bindgen 0.2.122](file:///Users/nicho/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wasm-bindgen-0.2.122/src/externref.rs) does `HEAP_SLAB.0.borrow_mut().alloc()`. Something in the walker descent reentrantly enters `__externref_table_alloc` (probably a closure return value being boxed as an externref while the outer frame is mid-alloc). This is a wasm-bindgen / closure-shim issue, NOT a framework bug, and likely needs an upstream fix.

The clean test: would a minimal repro outside this framework also exhibit it on 0.2.122? If yes, file upstream. If no, our framework is doing something the 0.2.122 closure shim doesn't expect.

## What I changed that should be reverted if you go with 0.2.121

- `Cargo.lock` — revert via the `cargo update --precise` commands above.
- *Optional*: revert the two patch insertions (`crates/tools/wasm-split/wasm-split-cli/src/lib.rs` + `crates/tools/build/web/src/lib.rs`). They're benign no-ops with 0.2.121.

## Time-spent honesty

Most of this session went into chasing the symptom (`command_export` / `__wasm_call_ctors` re-run) without first verifying that 0.2.121 → 0.2.122 was the version we wanted to be on. The original `inventory` OOB the day's investigation started from was probably the same wasm-bindgen 0.2.122 regression — we ground forward instead of backward. The downgrade should have been the first thing tried.

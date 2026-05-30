//! Walker stack-depth regression — guards against `build_inner`
//! ballooning back over the per-call frame budget.
//!
//! ## Why this test exists
//!
//! `walker::build_inner` used to compile to a **77,504-byte** wasm
//! stack frame per call (the big match-on-`Element` reserved slots
//! for every arm's destructured locals AND every arm's
//! arg-marshalling area simultaneously). The default wasm-ld stack
//! is 1 MiB, so recursive tree descent overflowed at ~13 nested
//! elements. Symptom: `RuntimeError: memory access out of bounds`
//! on the website's `/demo` page (only `/demo` — the section's
//! `reactive_state()` subtree was one level deeper than the others
//! and pushed past the threshold). The trap address lands in
//! whatever leaf call needs a few bytes of stack and runs out first
//! (often `RefCell::borrow_mut` inside `__externref_table_alloc`),
//! making it look identical to a wasm-bindgen externref-reentrance
//! bug. It is not — it's stack underflow at the function prologue.
//!
//! The fix collapsed the frame to **~2,336 bytes** via:
//!   1. One `#[inline(never)] fn dispatch_X<B>` per variant, each
//!      destructuring inside its own frame so per-variant locals
//!      don't live on `build_inner`'s frame.
//!   2. A single fn-pointer dispatch call site so the 23 arg-
//!      marshalling areas collapse to one.
//!
//! This test pins the win.
//!
//! ## What it tests
//!
//! Builds a synthetic tree N levels deep (homogeneous nested
//! `view([…])` wrapping a leaf `text`) inside a thread whose stack
//! size is constrained to the wasm-ld default (1 MiB). If
//! `build_inner` or any of its callees (`view::build`,
//! `insert_children`, `build`, `with_current_identity`) regresses
//! over the per-level budget, the test thread overflows its stack
//! and aborts.
//!
//! At today's measurements (~2.3 KiB per build_inner call, ~13 KiB
//! per insert_children, ~3.5 KiB per dispatch_view) one level of
//! nesting costs ~20 KiB total. DEPTH=30 → ~600 KiB; comfortably
//! under 1 MiB with ~400 KiB headroom. A regression that doubled
//! `build_inner`'s frame would push past that headroom and crash
//! the test.
//!
//! ## When it fails
//!
//! Native: process aborts with "thread '…' has overflowed its
//! stack" or SIGSEGV. CI surfaces this as a hard test failure
//! rather than a normal `assert!` mismatch. The recovery is to
//! check what grew via `wasm-objdump -d <wasm>` then `awk
//! '/func\[NNN\] <build_inner/,/i32.sub/'` — the `i32.const NNN`
//! before `i32.sub` is the frame size. Compare against the value
//! recorded in [[project-walker-build-inner-stack-overflow]].
//!
//! Native code-gen ≠ wasm code-gen, but the two track each other
//! closely enough that a regression on one usually shows on the
//! other. This is a trend safeguard, not an exact-frame-size lock.

use std::thread;

use runtime_core::{text, view, Element};

use crate::common::TestRuntime;

/// Build N levels of nested `View`s around a leaf `Text`, then
/// render. Returns when the walker finishes the whole subtree.
/// Recursion depth equals N (each View is one walker level).
fn render_nested_views(depth: usize) {
    let mut tree: Element = text("leaf").into();
    for _ in 0..depth {
        tree = view(vec![tree]).into();
    }
    let rt = TestRuntime::new();
    let _owner = rt.render(tree);
}

/// 30 levels of nested Views render without overflowing a
/// wasm-sized 1 MiB stack. Pins the post-fix `build_inner` frame
/// size — see the module doc for the diagnostic story.
#[test]
fn deep_nested_views_render_within_wasm_stack_budget() {
    // 1 MiB matches `wasm-ld`'s default `-z stack-size=1048576`.
    // We want native CI to flag the regression at the same depth
    // wasm would, so the constraint here mirrors wasm's.
    const WASM_DEFAULT_STACK: usize = 1024 * 1024;
    // 30 levels of `View → … → Text`. Well past anything realistic
    // (the website's deepest screen sits around 18-20) but tight
    // enough that a partial regression in `build_inner`'s frame
    // is caught.
    const DEPTH: usize = 30;

    let handle = thread::Builder::new()
        .name("walker-stack-depth-regression".into())
        .stack_size(WASM_DEFAULT_STACK)
        .spawn(|| render_nested_views(DEPTH))
        .expect("spawn constrained-stack test thread");

    // If the walker overflowed the constrained stack, the thread
    // aborts (stack overflow isn't a recoverable panic). On Linux
    // this kills the process; the CI failure mode is "test
    // process exited unexpectedly" rather than a clean assert.
    // That's fine — the diagnostic signal is the same either way.
    handle
        .join()
        .expect("walker thread aborted — likely build_inner frame regression");
}

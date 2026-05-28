//! SCRATCH PLAYGROUND — for eyeballing rust-analyzer behavior with `ui!`.
//!
//! This page is intentionally NOT routed; it exists only so rust-analyzer
//! pulls it into its module tree and runs the `ui!` proc-macro on it.
//! Delete this file (and its `pub mod ra_playground;` line in `mod.rs`)
//! once you've finished evaluating.
//!
//! FIRST, reload the proc-macro server so the recovery change is live:
//!   Command Palette → "rust-analyzer: Restart server"
//! (the `ui!`/`jsx!` proc-macros are rebuilt on restart — without this you
//! are still testing the OLD macro.)

use runtime_core::{Element, Signal, component, signal, ui};
use idea_ui::{Stack, StackGap, Typography};

#[component]
fn Test123() -> Element {
    ui! {
        Typography(content = "a user component".to_string())
    }
}

#[allow(dead_code)] // scratch page, intentionally unrouted
pub fn page() -> Element {
    let count: Signal<i32> = signal!(0);

    ui! {
        Stack(gap = StackGap::Xl) {
            Test123()
            // ── TEST 1 — go-to-def on a USER COMPONENT ───────────────────
            // Put the cursor on `Typography` and "Go to Definition".
            // Expected: jumps to idea_ui's Typography. User components
            // dispatch to a real `Typography!` macro, so once the block
            // expands cleanly (the Layer-1 fix) this should resolve.
            Typography(content = "Hello".to_string())

            // ── TEST 2 — go-to-def on a PRIMITIVE ────────────────────────
            // Cursor on `Text` → "Go to Definition".
            // Expected (today): "no definition available". There is no
            // `Text` symbol — the macro emits a hardcoded `runtime_core::text`
            // with a call-site span. This is the gap the PascalCase-alias
            // (Layer-2) change would close. Note whether it fails.
            Text { "a bare primitive" }

            // ── TEST 3 — completion / hover on an EXPRESSION ─────────────
            // (a) Hover over `count` below: expected type `Signal<i32>`.
            // (b) Delete `.get()` so it reads `count`, then type `.` —
            //     expected: Signal's methods (`get`, `set`, …) complete.
            // This is the "lost functions/fields on an expression" path.
            // With a fully-parsing block it should work.
            Typography(content = count.get().to_string())

            // ── TEST 4 — mid-edit RECOVERY (the Layer-1 fix) ─────────────
            // Break the line below: change `count.get()` to `count.get(`
            // (delete the closing paren) so the whole `ui!` no longer parses.
            // Now go back up to TEST 3 and confirm hover/go-to-def on its
            // `count.get()` STILL works. Before the fix, one half-typed call
            // turned the entire block into an opaque `compile_error!` and
            // every expression in it went dark. After the fix, the complete
            // siblings stay type-checked.
            Typography(content = count.get().to_string())
        }
    }
}
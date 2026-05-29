//! SCRATCH PLAYGROUND — for eyeballing rust-analyzer behavior with `ui!`.
//!
//! This page is intentionally NOT routed; it exists only so rust-analyzer
//! pulls it into its module tree and runs the `ui!` proc-macro on it.
//! Delete this file (and its `pub mod ra_playground;` line in `mod.rs`)
//! once you've finished evaluating.
//!
//! FIRST, reload the proc-macro server so the latest macro is live:
//!   Command Palette → "rust-analyzer: Restart server"
//! (the `ui!`/`jsx!` proc-macros are rebuilt on restart — without this you
//! are still testing the OLD macro.)
//!
//! Components now dispatch through a plain struct literal
//! (`BuildElement::build(Typography { content: …, ..defaults() })`), not a
//! per-component macro — so field completion and go-to-def should work
//! natively.

use runtime_core::{Element, Signal, component, signal, ui};
use idea_ui::{Stack, StackGap, Typography};

// Props must derive `Default` (the dispatch base is `..Default::default()`).
// `#[component(default(count = 10))]` overrides the *omitted* value for one
// field: leave `count` off the call site and it comes in as 10, not i32's
// own default of 0. Other fields fall back to their type default.
#[derive(Default)]
struct Test123Props {
    label: String,
    count: i32,
}

/// Renders `label: count` as a Typography line. `count` defaults to 10
/// when omitted. (Hover the `Test123` tag in `page()` — this doc shows.)
#[component(default(count = 10))]
fn Test123(props: &Test123Props) -> Element {
    ui! {
        Typography(content = format!("{}: {}", props.label, props.count))
    }
}

#[allow(dead_code)] // scratch page, intentionally unrouted
pub fn page() -> Element {
    let count: Signal<i32> = signal!(0);

    ui! {
        Stack(gap = StackGap::Xl) {
            // ── TEST 5 — DECLARED DEFAULT + field completion ─────────────
            // `count` is omitted, so it comes in as 10 (the
            // `#[component(default(count = 10))]` value), not 0. Renders
            // "hi: 10". Also: inside the parens, completion offers the real
            // `Test123Props` fields (`label`, `count`) — and go-to-def on
            // `Test123` lands on the generated `pub type Test123 = Test123Props`.
            Test123(label = "hi".to_string())
            // ── TEST 1 — go-to-def on a USER COMPONENT ───────────────────
            // Cursor on `Typography` → "Go to Definition". Expected: jumps
            // to the `pub type Typography = TypographyProps` alias (one hop
            // from the props struct). The tag is a real type now, not a macro.
            Typography(content = "Hello".to_string())

            // ── TEST 1b — FIELD-NAME COMPLETION (the headline win) ───────
            // Inside the parens below, after a comma, type a letter and ask
            // for completion: expected the real `TypographyProps` fields
            // (`content`, `kind`, `tone`, `align`, `muted`, …). This is the
            // struct-literal payoff — the prop list is a real struct now.
            Typography(content = "type a field name here".to_string())

            // ── TEST 2 — go-to-def on a PRIMITIVE ────────────────────────
            // Cursor on `Text` → "Go to Definition". Primitives still emit a
            // hardcoded `runtime_core::text` (call-site span, no `Text`
            // symbol), so this may still say "no definition available" — the
            // component-dispatch change didn't touch primitives. Note whether
            // it differs from the user-component case in TEST 1.
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
//! Terminal drawer regression: the sidebar built via the new-shape
//! `leading_with(...)` slot must actually render in the terminal.
//!
//! The bug: the terminal handler read ONLY the legacy `presentation.sidebar`
//! slot. Apps using the new slot system (`.leading_with(...)` — the tutorial
//! and website) populate `presentation.leading_slot` instead, so the handler
//! skipped the sidebar build entirely and the column rendered permanently
//! BLANK. The iOS/Android/web handlers already preferred `leading_slot`; this
//! guards the terminal handler doing the same.
//!
//! Gated on `feature = "terminal"` because the terminal handler module only
//! compiles with it (the terminal target shares the host triple, so a Cargo
//! feature — not `cfg(target_os)` — selects the backend; see the drawer
//! Cargo.toml `[features]`).
#![cfg(all(not(target_arch = "wasm32"), feature = "terminal"))]

use drawer_navigator::{DrawerBuilder, DrawerNavigator};
use runtime_core::primitives::navigator::Screen;
use runtime_core::{text, view, Route};

const HOME: Route<()> = Route::<()>::new("home", "/");

#[test]
fn drawer_terminal_renders_leading_slot_sidebar() {
    // Mount a drawer whose sidebar comes from `leading_with` (the new slot),
    // headless on the terminal backend. ≥2 frames so the deferred sidebar
    // microtask drains before the snapshot. The drawer self-registers via
    // inventory at `TerminalBackend::new`, so no explicit registration.
    let rows = host_terminal::render_headless(
        || {
            DrawerNavigator::new(&HOME)
                .leading_with(|_slot| {
                    view(vec![text("LEADING_SIDEBAR_MARKER").into()]).into()
                })
                .screen(HOME, |_| {
                    Screen::new(view(vec![text("HOME_BODY").into()]))
                })
                .into()
        },
        |_backend| {},
        80,
        12,
        None,
        8,
    );
    let screen = rows.join("\n");

    // The regression: the `leading_with` sidebar must appear. Pre-fix the
    // terminal handler read only the legacy `sidebar` slot, so this marker
    // never rendered and the sidebar column was blank.
    assert!(
        screen.contains("LEADING_SIDEBAR_MARKER"),
        "the `leading_with` sidebar must render in the terminal (was blank \
         before the handler read `leading_slot`); got:\n{screen}"
    );
}

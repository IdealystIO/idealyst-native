//! The in-app conformance suite. Gated on `robot`: without it the Robot
//! API is a stub, so this module is never compiled and the app still runs
//! (just untested).
//!
//! Each test follows Playwright's `locate → act → assert`. Reactive updates
//! land synchronously relative to the next Robot query (a `when` branch that
//! mounts on `set_toggle` is visible to the assertion on the next line), so
//! no explicit waits are needed.

use robot_e2e::{expect, flow, run_suites, suite, test, Page};
use runtime_core::robot::ElementKind;

/// Entry point scheduled from `app()` ~1s after mount.
pub(crate) fn run_all() {
    run_suites(vec![primitives_suite(), modal_suite(), navigation_suite()]);
}

fn primitives_suite() -> robot_e2e::Suite {
    suite(
        "primitives",
        vec![
            test("static primitives render", |page: &Page| {
                expect(&page.get_by_test_id("title")).to_have_text("Conformance")?;
                expect(&page.get_by_test_id("counter")).to_have_text("Counter: 0")?;
                expect(&page.get_by_test_id("spinner")).to_be_visible()?;
                expect(&page.get_by_test_id("icon")).to_be_visible()?;
                expect(&page.get_by_test_id("greeting")).to_have_text("Hello, stranger")?;
                expect(&page.get_by_test_id("push-detail")).to_be_visible()?;
                // At least the four always-present root buttons.
                expect(&page.get_by_role(ElementKind::Button)).to_have_min_count(4)?;
                Ok(())
            }),
            test("counter via button, button, and pressable", |page: &Page| {
                let counter = page.get_by_test_id("counter");
                page.get_by_test_id("inc").click()?;
                page.get_by_test_id("inc").click()?;
                expect(&counter).to_have_text("Counter: 2")?;
                page.get_by_test_id("dec").click()?;
                expect(&counter).to_have_text("Counter: 1")?;
                // Pressable container is a distinct click path into the same
                // signal.
                page.get_by_test_id("press5").click()?;
                expect(&counter).to_have_text("Counter: 6")?;
                Ok(())
            }),
            test("toggle mounts/unmounts a when branch", |page: &Page| {
                // Hidden initially.
                expect(&page.get_by_test_id("extra")).not_to_be_visible()?;
                expect(&page.get_by_test_id("slider")).to_have_count(0)?;
                // Reveal.
                page.get_by_test_id("toggle").set_toggle(true)?;
                expect(&page.get_by_test_id("extra")).to_be_visible()?;
                expect(&page.get_by_test_id("slider-val")).to_have_text("Slider: 0")?;
                // Drag the slider — value text updates live.
                page.get_by_test_id("slider").set_slider(50.0)?;
                expect(&page.get_by_test_id("slider-val")).to_have_text("Slider: 50")?;
                // Hide again — the branch is disposed.
                page.get_by_test_id("toggle").set_toggle(false)?;
                expect(&page.get_by_test_id("extra")).not_to_be_visible()?;
                expect(&page.get_by_test_id("slider")).to_have_count(0)?;
                Ok(())
            }),
            test("text input echoes into greeting", |page: &Page| {
                expect(&page.get_by_test_id("greeting")).to_have_text("Hello, stranger")?;
                page.get_by_test_id("name").fill("Ada")?;
                expect(&page.get_by_test_id("greeting")).to_have_text("Hello, Ada")?;
                Ok(())
            }),
        ],
    )
}

fn modal_suite() -> robot_e2e::Suite {
    suite(
        "modal (portal + nested pressable)",
        vec![test(
            "confirm button nested in the card pressable still fires",
            |page: &Page| {
                expect(&page.get_by_test_id("confirmed")).to_have_text("Confirmed: 0")?;
                // Modal closed: its contents aren't in the tree.
                expect(&page.get_by_test_id("modal-confirm")).not_to_be_visible()?;
                // Open it.
                page.get_by_test_id("open-modal").click()?;
                expect(&page.get_by_test_id("modal-title")).to_be_visible()?;
                expect(&page.get_by_test_id("modal-confirm")).to_be_visible()?;
                // THE regression: a button nested inside the modal card's
                // own tap-recognizing Pressable must still fire its click.
                page.get_by_test_id("modal-confirm").click()?;
                expect(&page.get_by_test_id("confirmed")).to_have_text("Confirmed: 1")?;
                // Confirm closed the modal.
                expect(&page.get_by_test_id("modal-title")).not_to_be_visible()?;
                Ok(())
            },
        )],
    )
}

fn navigation_suite() -> robot_e2e::Suite {
    // A flow, not a sync test: stack pop completes on the browser's async
    // `popstate` (a macrotask), so the "detail gone" assertion must wait a
    // real tick — `poll` retries it across ticks. Push, by contrast, is
    // synchronous, so its assertions are plain `act` steps.
    suite(
        "stack navigator",
        vec![flow("push reveals detail, back pops it")
            .act(|p: &Page| expect(&p.get_by_test_id("detail-marker")).not_to_be_visible())
            .act(|p: &Page| p.get_by_test_id("push-detail").click())
            .act(|p: &Page| expect(&p.get_by_test_id("detail-marker")).to_be_visible())
            .act(|p: &Page| p.get_by_test_id("back").click())
            .poll(|p: &Page| expect(&p.get_by_test_id("detail-marker")).not_to_be_visible())
            .build()],
    )
}

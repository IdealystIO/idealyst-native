//! The in-app conformance suite. Gated on `robot`: without it the Robot
//! API is a stub, so this module is never compiled and the app still runs
//! (just untested).
//!
//! Each test follows Playwright's `locate → act → assert`. Reactive updates
//! land synchronously relative to the next Robot query (a `when` branch that
//! mounts on `set_toggle` is visible to the assertion on the next line), so
//! no explicit waits are needed.

use robot_e2e::{expect, flow, run_suites, skip, suite, test, ElementKind, Page};
use runtime_shared::introspect::keys;

/// Entry point scheduled from `app()` ~1s after mount.
///
/// All six suites run on every backend: primitives, modal, stack
/// navigation, idea-ui components, `#[method]` invocation, and media
/// (image sources + the bottom-anchored menu).
pub(crate) fn run_all() {
    run_suites(vec![
        primitives_suite(),
        modal_suite(),
        navigation_suite(),
        idea_ui_suite(),
        component_methods_suite(),
        media_suite(),
        capabilities_suite(),
    ]);
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
            // A flow: "visible" needs a laid-out, non-empty frame, and a
            // backend that lays out on the next turn (macOS) has none on the
            // tick the branch mounts.
            flow("toggle mounts/unmounts a when branch")
                .act(|page: &Page| {
                    // Hidden initially.
                    expect(&page.get_by_test_id("extra")).not_to_be_visible()?;
                    expect(&page.get_by_test_id("slider")).to_have_count(0)
                })
                // Reveal.
                .act(|page: &Page| page.get_by_test_id("toggle").set_toggle(true))
                .poll(|page: &Page| expect(&page.get_by_test_id("extra")).to_be_visible())
                .act(|page: &Page| {
                    expect(&page.get_by_test_id("slider-val")).to_have_text("Slider: 0")?;
                    // Drag the slider — value text updates live.
                    page.get_by_test_id("slider").set_slider(50.0)?;
                    expect(&page.get_by_test_id("slider-val")).to_have_text("Slider: 50")?;
                    // Hide again — the branch is disposed.
                    page.get_by_test_id("toggle").set_toggle(false)
                })
                .poll(|page: &Page| {
                    expect(&page.get_by_test_id("extra")).not_to_be_visible()?;
                    expect(&page.get_by_test_id("slider")).to_have_count(0)
                })
                .build(),
            test("text input echoes into greeting", |page: &Page| {
                expect(&page.get_by_test_id("greeting")).to_have_text("Hello, stranger")?;
                page.get_by_test_id("name").fill("Ada")?;
                expect(&page.get_by_test_id("greeting")).to_have_text("Hello, Ada")?;
                Ok(())
            }),
            // Per-row conditional drop on list shrink — the whiteboard
            // "delete button won't disappear on the last canvas" bug. Three
            // rows each render a `del-marker` gated on `rows.len() > 1`.
            // Removing rows down to one must drop EVERY marker (each surviving
            // row's `when` re-evaluates to false). If a kept row's conditional
            // doesn't drop, the count stays > 0 and this fails.
            test("per-row conditional drops when the list shrinks to one", |page: &Page| {
                let del = page.get_by_test_id("del-marker");
                expect(&del).to_have_count(3)?;
                page.get_by_test_id("remove-row").click()?; // 3 -> 2
                expect(&del).to_have_count(2)?;
                page.get_by_test_id("remove-row").click()?; // 2 -> 1: all markers gone
                expect(&del).to_have_count(0)?;
                Ok(())
            }),
        ],
    )
}

fn modal_suite() -> robot_e2e::Suite {
    suite(
        "modal (portal + nested pressable)",
        // A flow for the same reason as the toggle test: the opened modal
        // has a frame only after the next layout pass.
        vec![flow("confirm button nested in the card pressable still fires")
            .act(|page: &Page| {
                expect(&page.get_by_test_id("confirmed")).to_have_text("Confirmed: 0")?;
                // Modal closed: its contents aren't in the tree.
                expect(&page.get_by_test_id("modal-confirm")).not_to_be_visible()?;
                // Open it.
                page.get_by_test_id("open-modal").click()
            })
            .poll(|page: &Page| {
                expect(&page.get_by_test_id("modal-title")).to_be_visible()?;
                expect(&page.get_by_test_id("modal-confirm")).to_be_visible()
            })
            .act(|page: &Page| {
                // THE regression: a button nested inside the modal card's
                // own tap-recognizing Pressable must still fire its click.
                page.get_by_test_id("modal-confirm").click()?;
                expect(&page.get_by_test_id("confirmed")).to_have_text("Confirmed: 1")
            })
            // Confirm closed the modal.
            .poll(|page: &Page| expect(&page.get_by_test_id("modal-title")).not_to_be_visible())
            .build()],
    )
}

fn idea_ui_suite() -> robot_e2e::Suite {
    // idea-ui "as a key implementor": its Switch/Checkbox/Button forward a
    // `test_id` to their interactive primitive (always — the id is inert
    // until the runtime's robot is on), so the robot can locate + drive them. A flow because it navigates (push +
    // async pop) and each component's reactive status settles across ticks.
    suite(
        "idea-ui components",
        vec![flow("switch, checkbox, button forward test_id and respond")
            .act(|p: &Page| p.get_by_test_id("goto-components").click())
            .act(|p: &Page| expect(&p.get_by_test_id("components-marker")).to_be_visible())
            // Switch: toggles on click; status text reflects the bound signal.
            .act(|p: &Page| expect(&p.get_by_test_id("ui-switch-status")).to_have_text("switch=false"))
            .act(|p: &Page| p.get_by_test_id("ui-switch").click())
            .act(|p: &Page| expect(&p.get_by_test_id("ui-switch-status")).to_have_text("switch=true"))
            // Checkbox.
            .act(|p: &Page| p.get_by_test_id("ui-check").click())
            .act(|p: &Page| expect(&p.get_by_test_id("ui-check-status")).to_have_text("check=true"))
            // Button: each click increments the counter.
            .act(|p: &Page| p.get_by_test_id("ui-button").click())
            .act(|p: &Page| p.get_by_test_id("ui-button").click())
            .act(|p: &Page| expect(&p.get_by_test_id("ui-button-status")).to_have_text("clicks=2"))
            // Back to root (async pop).
            .act(|p: &Page| p.get_by_test_id("comp-back").click())
            .poll(|p: &Page| expect(&p.get_by_test_id("components-marker")).not_to_be_visible())
            // The screen under it comes back (a pop must re-show it, not drop it).
            .poll(|p: &Page| expect(&p.get_by_test_id("title")).to_be_visible())
            .build()],
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
            // The screen under it comes back (a pop must re-show it, not drop it).
            .poll(|p: &Page| expect(&p.get_by_test_id("title")).to_be_visible())
            .build()],
    )
}

/// `#[method]` invocation over the robot surface — the same
/// `list_components` → `invoke_method` path the MCP server and the Inspector
/// use. Also asserts the element↔component link the macro establishes
/// (the registration's realize hook — so the Inspector can
/// resolve a selected element to its
/// methods).
fn component_methods_suite() -> robot_e2e::Suite {
    use runtime_vocabulary::robot::{invoke_method, list_components};

    suite(
        "component methods",
        vec![test(
            "list_components + invoke_method drive a #[method] component; element link resolves",
            |page: &Page| {
                // Locate the live instance and confirm the macro linked it to
                // its root element id (what the Inspector resolves a selection
                // against).
                let comps = list_components();
                let counter = comps
                    .iter()
                    .find(|c| c.name == "MethodCounter")
                    .expect("MethodCounter registered its methods");
                assert!(
                    counter.element_id.is_some(),
                    "the component is linked to its root element",
                );

                // Starts at the mounted `initial = 10`.
                expect(&page.get_by_test_id("method-counter-val")).to_have_text("methods: 10")?;

                // increment() — no args (the inspector's easy manual case).
                invoke_method(counter.id, "increment", &runtime_vocabulary::glue::__serde_json::json!({}))
                    .expect("increment()");
                expect(&page.get_by_test_id("method-counter-val")).to_have_text("methods: 11")?;

                // bump_by(5) — args deserialized from JSON, same as the bridge.
                invoke_method(
                    counter.id,
                    "bump_by",
                    &runtime_vocabulary::glue::__serde_json::json!({ "n": 5 }),
                )
                .expect("bump_by(5)");
                expect(&page.get_by_test_id("method-counter-val")).to_have_text("methods: 16")?;

                // reset() — no args; visible because we started non-zero.
                invoke_method(counter.id, "reset", &runtime_vocabulary::glue::__serde_json::json!({}))
                    .expect("reset()");
                expect(&page.get_by_test_id("method-counter-val")).to_have_text("methods: 0")?;

                Ok(())
            },
        )],
    )
}

/// `image()` source coverage: each local source kind must deliver
/// `on_load` with its natural size, and an undecodable / unreachable one
/// `on_error` — the Android "image loads nothing" regression. (The remote
/// rows are network-dependent, so they're shown but not asserted.) Then
/// the bottom-anchored menu opens and shrinks; its placement is checked
/// visually (see `media_page`).
fn media_suite() -> robot_e2e::Suite {
    suite(
        "media (image sources + anchored menu)",
        vec![flow("every image source loads or errors; menu opens and shrinks")
            .act(|p: &Page| p.get_by_test_id("goto-media").click())
            .act(|p: &Page| expect(&p.get_by_test_id("media-marker")).to_be_visible())
            .poll(|p: &Page| expect(&p.get_by_test_id("img-png-status")).to_have_text("png: load 48x24"))
            .poll(|p: &Page| expect(&p.get_by_test_id("img-svg-status")).to_have_text("svg: load 32x16"))
            .poll(|p: &Page| expect(&p.get_by_test_id("img-svg5-status")).to_have_text("svg 5x: load 32x16"))
            .poll(|p: &Page| expect(&p.get_by_test_id("img-asset-status")).to_have_text("asset: load 48x48"))
            .poll(|p: &Page| expect(&p.get_by_test_id("img-bad-status")).to_have_text("bad data: error"))
            .poll(|p: &Page| expect(&p.get_by_test_id("img-broken-status")).to_have_text("broken: error"))
            .act(|p: &Page| p.get_by_test_id("menu-trigger").click())
            .act(|p: &Page| expect(&p.get_by_test_id("menu-panel")).to_be_visible())
            .act(|p: &Page| expect(&p.get_by_test_id("menu-state")).to_have_text("menu rows: 6"))
            .act(|p: &Page| p.get_by_test_id("menu-shrink").click())
            .act(|p: &Page| expect(&p.get_by_test_id("menu-state")).to_have_text("menu rows: 2"))
            .act(|p: &Page| p.get_by_test_id("menu-trigger").click())
            .act(|p: &Page| expect(&p.get_by_test_id("menu-panel")).not_to_be_visible())
            .act(|p: &Page| p.get_by_test_id("media-back").click())
            .poll(|p: &Page| expect(&p.get_by_test_id("media-marker")).not_to_be_visible())
            // The screen under it comes back (a pop must re-show it, not drop it).
            .poll(|p: &Page| expect(&p.get_by_test_id("title")).to_be_visible())
            .build()],
    )
}

// The sizes differ per backend's native control (16 / 20 small, 32 / 40 large):
// the check is that the frame crosses between them.
const SPINNER_SMALL_MAX: f32 = 24.0;
const SPINNER_LARGE_MIN: f32 = 30.0;

/// The spinner's laid-out height passes `ok`: the frame comes from layout, so
/// this catches a measurer still answering the old size as well as a widget
/// that ignored the new one.
fn spinner_height(p: &Page, ok: impl Fn(f32) -> bool) -> Result<(), String> {
    let height = p.get_by_test_id("cap-spinner").frame()?.height;
    if ok(height) {
        Ok(())
    } else {
        Err(format!("the spinner is {height} tall"))
    }
}

/// The four boxes of `RowFixture` on one line, left to right, as children of
/// the row. A `view` around any pair would stack that pair.
fn row_is_flat(p: &Page) -> Result<(), String> {
    let frames = ["cap-row-a", "cap-row-b", "cap-row-c", "cap-row-d"]
        .iter()
        .map(|id| p.get_by_test_id(id).frame())
        .collect::<Result<Vec<_>, _>>()?;
    for pair in frames.windows(2) {
        if (pair[0].y - pair[1].y).abs() > 0.5 || pair[1].x < pair[0].x + pair[0].width {
            return Err(format!("not on one line: {frames:?}"));
        }
    }
    Ok(())
}

/// The system check's automated half (`system_check.rs`): one test per
/// backend capability whose trait default silently does nothing. Test
/// names are what the screen's sections look results up by — rename both
/// together. Native input (`hover`, `press`) and native readback
/// (`to_show_native_text`) skip where the platform has neither, so a skip
/// says "not on this platform", never "broken".
fn capabilities_suite() -> robot_e2e::Suite {
    suite(
        "capabilities",
        vec![
            flow("open the system check")
                .act(|p: &Page| p.get_by_test_id("goto-system-check").click())
                .poll(|p: &Page| expect(&p.get_by_test_id("check-marker")).to_be_visible())
                .build(),
            flow("hover lights the hovered state")
                .act(|p: &Page| p.get_by_test_id("cap-hover").hover())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-hover")).to_be_hovered())
                .act(|p: &Page| p.get_by_test_id("cap-elsewhere").hover())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-hover")).not_to_be_hovered())
                .build(),
            flow("press lights the pressed state")
                .act(|p: &Page| p.get_by_test_id("cap-hover").press())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-hover")).to_be_pressed())
                .act(|p: &Page| p.get_by_test_id("cap-hover").release())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-hover")).not_to_be_pressed())
                .build(),
            flow("a value written from code reaches the field")
                .act(|p: &Page| p.get_by_test_id("cap-input").fill("hello"))
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-input-echo")).to_have_text("value: hello"))
                .act(|p: &Page| p.get_by_test_id("cap-input-clear").click())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-input-echo")).to_have_text("value: "))
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-input")).to_show_native_text(""))
                .build(),
            flow("focus and blur reach the app")
                .act(|p: &Page| p.get_by_test_id("cap-input").focus())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-input-events")).to_contain_text("focus: 1"))
                .act(|p: &Page| p.get_by_test_id("cap-input").blur())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-input-events")).to_contain_text("blur: 1"))
                .build(),
            flow("a tap outside a text input blurs it")
                // iOS dismisses through a UIKit tap recognizer, and the robot has no
                // real pointer there: its `tap` calls the handler directly.
                .act(|_p: &Page| if cfg!(target_os = "ios") { Err(skip("no real pointer to reach the recognizer")) } else { Ok(()) })
                .act(|p: &Page| p.get_by_test_id("cap-input").focus())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-input")).to_be_focused())
                // A plain view: a native button takes the press for itself on macOS.
                .act(|p: &Page| p.get_by_test_id("cap-hover").tap())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-input")).not_to_be_focused())
                .build(),
            flow("a text area is editable")
                .act(|p: &Page| p.get_by_test_id("cap-area").fill("two\nlines"))
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-area-echo")).to_have_text("area: two⏎lines"))
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-area")).to_show_native_text("two\nlines"))
                .build(),
            flow("a flat_list renders rows and follows its data")
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-row")).to_have_count(3))
                .act(|p: &Page| p.get_by_test_id("cap-add-row").click())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-row")).to_have_count(4))
                .build(),
            flow("a flat_list row sits in its slot")
                .poll(|p: &Page| {
                    let frame = p.get_by_test_id("cap-row").frame()?;
                    if (frame.height - 32.0).abs() > 0.5 {
                        return Err(format!("the first row is {} tall, its size says 32", frame.height));
                    }
                    Ok(())
                })
                .build(),
            flow("reaching the end of a flat_list is reported")
                .act(|p: &Page| p.get_by_test_id("cap-long-list").scroll_to(0.0, 100_000.0))
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-list-end")).to_have_text("list end reached: 1"))
                .build(),
            flow("reaching the end of a scroll view is reported")
                .act(|p: &Page| p.get_by_test_id("cap-scroll").scroll_to(0.0, 10_000.0))
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-scroll-end")).to_have_text("end reached: 1"))
                .build(),
            flow("a style that drops opacity goes back to opaque")
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-opacity")).to_have_native_opacity(0.5))
                .act(|p: &Page| p.get_by_test_id("cap-opacity-toggle").click())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-opacity")).to_have_native_opacity(1.0))
                .build(),
            flow("a style that drops a radius and a border loses them")
                .act(|p: &Page| p.get_by_test_id("cap-opacity-toggle").click())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-opacity")).to_have_native_length(keys::CORNER_RADIUS, 6.0))
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-opacity")).to_have_native_length(keys::BORDER_WIDTH, 2.0))
                .act(|p: &Page| p.get_by_test_id("cap-opacity-toggle").click())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-opacity")).to_have_native_length(keys::CORNER_RADIUS, 0.0))
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-opacity")).to_have_native_length(keys::BORDER_WIDTH, 0.0))
                .build(),
            flow("several ui! children join the parent's row")
                .poll(row_is_flat)
                .act(|p: &Page| p.get_by_test_id("cap-row-toggle").click())
                .poll(|p: &Page| expect(&p.get_by_test_id("cap-row-c")).not_to_be_visible())
                .act(|p: &Page| p.get_by_test_id("cap-row-toggle").click())
                .poll(row_is_flat)
                .build(),
            flow("a spinner follows a live size")
                .poll(|p: &Page| spinner_height(p, |h| h <= SPINNER_SMALL_MAX))
                .act(|p: &Page| p.get_by_test_id("cap-spinner-toggle").click())
                .poll(|p: &Page| spinner_height(p, |h| h >= SPINNER_LARGE_MIN))
                .act(|p: &Page| p.get_by_test_id("cap-spinner-toggle").click())
                .poll(|p: &Page| spinner_height(p, |h| h <= SPINNER_SMALL_MAX))
                .build(),
            test("external links have an opener", |_p: &Page| {
                if runtime_shared::can_open_urls() {
                    Ok(())
                } else {
                    Err("no URL opener installed: open_url (external links) does nothing".into())
                }
            }),
            flow("close the system check")
                .act(|p: &Page| p.get_by_test_id("check-back").click())
                .poll(|p: &Page| expect(&p.get_by_test_id("check-marker")).not_to_be_visible())
                // The screen under it comes back (a pop must re-show it, not drop it).
                .poll(|p: &Page| expect(&p.get_by_test_id("title")).to_be_visible())
                .build(),
        ],
    )
}

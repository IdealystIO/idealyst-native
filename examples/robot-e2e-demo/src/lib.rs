//! `robot-e2e-demo` — an in-app, **Playwright-style E2E suite** that
//! drives the app's own UI through the framework's Robot API and narrates
//! every step to the platform console.
//!
//! The screen is a small interactive app: a counter, a toggle-reveal
//! panel, and a name field with a live greeting. Built into it is an E2E
//! suite ([`e2e`]) that locates elements by `test_id`, clicks/fills them,
//! and asserts on the live rendered text — exactly the
//! `locate → act → assert` loop you'd write in Playwright, but running
//! in-process against *our* introspection registry, so the same suite
//! works on web, iOS, Android, macOS, and the terminal.
//!
//! ## Run it
//!
//! ```text
//! idealyst dev --macos --local     # or --android / --ios / --web / --terminal
//! ```
//!
//! The suite auto-runs ~1s after launch (and again whenever you press
//! **Run E2E suite again**). Watch the console:
//!
//! - **macOS / terminal**: stderr in the launching shell.
//! - **Android**: `adb logcat | grep e2e`.
//! - **web**: the browser devtools console.
//! - **iOS**: the device/simulator system log.
//!
//! Sample output:
//!
//! ```text
//! [e2e] ▶ suite: counter, panel & form  (4 tests)
//! [e2e] • test (1/4): counter increments and decrements
//! [e2e]   ✓ expect(getByTestId("counter")).toHaveText("Counter: 0")
//! [e2e]   ▸ getByTestId("inc").click()
//! [e2e]   ▸ getByTestId("inc").click()
//! [e2e]   ✓ expect(getByTestId("counter")).toHaveText("Counter: 2")
//! [e2e]   ✅ PASS: counter increments and decrements
//! ...
//! [e2e] ■ suite: counter, panel & form — ✅ all green: 4 passed, 0 failed
//! ```

use idea_ui::{
    install_idea_theme, light_theme, typography_kind, Stack, StackGap, StackPadding, Typography,
};
use runtime_core::{button, signal, text, text_input, ui, when, Element, IntoElement, Signal};

#[cfg(feature = "robot")]
mod e2e;

// ---------------------------------------------------------------------------
// Per-target registration hooks (no custom extensions; present for parity
// with the other examples — the sidecar hook is set only by the generated
// runtime-server wrapper).
// ---------------------------------------------------------------------------

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(_backend: &mut dev_server::WireRecordingBackend) {}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let count: Signal<i32> = signal!(0);
    let show_secret: Signal<bool> = signal!(false);
    let name: Signal<String> = signal!(String::new());

    // Kick off the E2E suite shortly after mount, once the first render
    // has populated the Robot registry. `after_ms_detached` self-manages
    // its handle (no cancel-on-drop), so a fire-and-forget schedule from
    // here is safe.
    runtime_core::after_ms_detached(INITIAL_RUN_DELAY_MS, run_suite);

    screen(count, show_secret, name)
}

/// ~1s gives the first layout/paint time to settle before the suite runs.
const INITIAL_RUN_DELAY_MS: i32 = 1000;

fn screen(count: Signal<i32>, show_secret: Signal<bool>, name: Signal<String>) -> Element {
    // Button handlers. `Signal` is `Copy`, so each closure captures its
    // own copy.
    let inc = move || count.update(|n| *n += 1);
    let dec = move || count.update(|n| *n -= 1);
    let toggle = move || show_secret.update(|v| *v = !*v);

    // The secret panel: a `when` branch that mounts (and registers) the
    // `secret` text only while `show_secret` is true — so the E2E suite
    // can assert it's *not* visible first, then visible after a click.
    let secret_branch = when(
        move || show_secret.get(),
        || {
            text("🔓 Secret unlocked")
                .test_id("secret")
                .into_element()
        },
        || runtime_core::view(vec![]).into_element(),
    );

    // Reactive labels via the builder form (`text(closure).test_id(...)`):
    // unambiguous, and the registry recomputes them on read so the E2E
    // assertions see live values. Assembled into a `Vec<Element>` (then
    // splatted into one `Stack`) to keep the `ui!` body unambiguous — same
    // pattern as `screenshot-demo`.
    let counter_label = text(move || format!("Counter: {}", count.get()))
        .test_id("counter")
        .into_element();

    let greeting = text(move || {
        let n = name.get();
        if n.is_empty() {
            "Hello, stranger".to_string()
        } else {
            format!("Hello, {n}")
        }
    })
    .test_id("greeting")
    .into_element();

    // NOTE: the interactive primitives use the *builder* form
    // (`button(...).test_id(...)`) rather than `ui! { button(test_id=...) }`.
    // The `ui!` macro only emits a primitive's `test_id` when the *example
    // crate's* own `robot` feature is active, whereas the builder method
    // sets it directly (gated only on `runtime-core/robot`, which `idealyst
    // dev` always turns on). Since an E2E demo lives or dies by its
    // `test_id`s resolving, the builder form is the reliable choice here.
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Robot E2E Demo".to_string(), kind = typography_kind::H1) },
        ui! {
            Typography(
                content = "An in-app, Playwright-style E2E suite drives this UI and logs every step to the console. It auto-runs ~1s after launch.".to_string(),
                muted = true,
            )
        },
        // — Counter —
        ui! { Typography(content = "Counter".to_string(), kind = typography_kind::H2) },
        counter_label,
        button("+ increment", inc).test_id("inc").into(),
        button("- decrement", dec).test_id("dec").into(),
        // — Reveal panel —
        ui! { Typography(content = "Reveal".to_string(), kind = typography_kind::H2) },
        button("Toggle secret", toggle).test_id("toggle-secret").into(),
        secret_branch,
        // — Name + greeting —
        ui! { Typography(content = "Greeting".to_string(), kind = typography_kind::H2) },
        text_input(name, move |s: String| name.set(s))
            .placeholder("Type a name".to_string())
            .test_id("name")
            .into(),
        greeting,
        // — Re-run —
        button("Run E2E suite again", run_suite)
            .test_id("run-suite")
            .into(),
    ];

    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) { children }
    }
}

// ---------------------------------------------------------------------------
// The suite. Gated on `robot`: without it the Robot API is a stub, so this
// compiles to a no-op and the app still runs (just untestable).
// ---------------------------------------------------------------------------

#[cfg(feature = "robot")]
fn run_suite() {
    use e2e::{expect, test, Page};

    e2e::run(
        "counter, panel & form",
        vec![
            test("counter increments and decrements", |page: &Page| {
                let counter = page.get_by_test_id("counter");
                expect(&counter).to_have_text("Counter: 0")?;
                page.get_by_test_id("inc").click()?;
                page.get_by_test_id("inc").click()?;
                expect(&counter).to_have_text("Counter: 2")?;
                page.get_by_test_id("dec").click()?;
                expect(&counter).to_have_text("Counter: 1")?;
                Ok(())
            }),
            test("secret panel hidden until toggled", |page: &Page| {
                expect(&page.get_by_test_id("secret")).not_to_be_visible()?;
                page.get_by_test_id("toggle-secret").click()?;
                // Assert via visible text (Playwright's getByText) to show
                // the locator works by content, not just test_id.
                expect(&page.get_by_text("Secret unlocked")).to_be_visible()?;
                Ok(())
            }),
            test("name field updates the greeting", |page: &Page| {
                expect(&page.get_by_test_id("greeting")).to_have_text("Hello, stranger")?;
                page.get_by_test_id("name").fill("Ada")?;
                expect(&page.get_by_test_id("greeting")).to_have_text("Hello, Ada")?;
                Ok(())
            }),
            test("the screen exposes four primitive buttons", |page: &Page| {
                use runtime_core::robot::ElementKind;
                expect(&page.get_by_role(ElementKind::Button)).to_have_count(4)?;
                Ok(())
            }),
        ],
    );
}

#[cfg(not(feature = "robot"))]
fn run_suite() {}

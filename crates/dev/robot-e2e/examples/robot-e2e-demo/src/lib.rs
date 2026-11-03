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

use idea_ui::theme::IdeaThemeRef;
use idea_ui::{install_idea_theme, light_theme, typography_kind, Stack, StackGap, StackPadding, Typography};
use runtime_core::{
    signal, stylesheet, text, ui, BlurOutcome, Element, FlexDirection, IntoElement, KeyEvent,
    KeyOutcome, Length, Signal,
};

#[cfg(feature = "robot")]
mod e2e;

// ---------------------------------------------------------------------------
// Per-target registration hooks (no custom extensions; present for parity
// with the other examples — the sidecar hook is set only by the generated
// runtime-server wrapper).
// ---------------------------------------------------------------------------

pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}

#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(_registry: &mut dev_server::newcore::SceneRegistry) {}

/// Android entry: the generated wrapper's `attach` mounts `scene_app()`.
pub fn scene_app() -> Element {
    app()
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub fn app() -> Element {
    install_idea_theme(light_theme());

    let count: Signal<i32> = signal(0);
    let show_secret: Signal<bool> = signal(false);
    let name: Signal<String> = signal(String::new());

    // Expose `count` by name so an out-of-process driver (`idealyst test`,
    // the inspector) can read it via the `read_signal` bridge verb — the
    // signal-assertion half of a cross-platform test. No cfg: outside a
    // robot build the call does nothing.
    runtime_vocabulary::robot::watch_signal("count", count);
    // The name field's own `on_focus` and `on_blur`, as an app observes
    // them: a test checks the author callbacks fire, not only the robot's
    // `focused` read.
    let name_focus = NameFocus { focused: signal(false), changes: signal(0), blurs: signal(0) };
    runtime_vocabulary::robot::watch_signal("name_focused", name_focus.focused);
    runtime_vocabulary::robot::watch_signal("name_focus_changes", name_focus.changes);
    runtime_vocabulary::robot::watch_signal("name_blurs", name_focus.blurs);

    // Robot functions a host-side test calls by name (`call_function`).
    // Registration needs no cfg: it is inert outside robot builds.
    COUNT.with(|c| c.set(Some(count)));
    runtime_vocabulary::robot_fn::register(set_count::robot());
    runtime_vocabulary::robot_fn::register(set_count_later::robot());
    // `Some(backdrop)` while the overlay is shown.
    let covered: Signal<Option<bool>> = signal(None);
    COVERED.with(|c| c.set(Some(covered)));
    runtime_vocabulary::robot_fn::register(set_cover::robot());
    let dialog_open: Signal<bool> = signal(false);
    DIALOG.with(|c| c.set(Some(dialog_open)));
    runtime_vocabulary::robot_fn::register(set_dialog::robot());

    // Kick off the E2E suite shortly after mount, once the first render
    // has populated the Robot registry. `after_ms_detached` self-manages
    // its handle (no cancel-on-drop), so a fire-and-forget schedule from
    // here is safe.
    //
    // Skip the auto-run when an external driver (`idealyst test`) owns the
    // app: that driver runs its own suite over the relay and the in-app
    // suite would race it for the same `count`/`secret` state.
    // `is_test_driven` reads the driver's flag on every platform (the
    // environment natively, a page global on web).
    if !runtime_vocabulary::robot::is_test_driven() {
        runtime_core::after_ms_detached(INITIAL_RUN_DELAY_MS, run_suite);
    }

    // An overlay shown by `demo.set_cover`, as an app writes one: the
    // default `Center` placement and, unless asked for none, the default
    // `Dismiss` backdrop — which covers the window on every backend, so a
    // native tap must not reach the page through it. With no backdrop only
    // the overlay's content takes taps and the page stays usable.
    let cover = ui! {
        if covered.get().is_some() {
            runtime_core::overlay(vec![text("Covered").test_id("cover-label").into_element()])
                .backdrop(if covered.get() != Some(false) {
                    runtime_core::primitives::overlay::BackdropMode::Dismiss
                } else {
                    runtime_core::primitives::overlay::BackdropMode::None
                })
                .on_dismiss(move || covered.set(None))
        }
    };
    ui! {
        view() {
            screen(count, show_secret, name, name_focus)
            cover
            dialog(dialog_open)
        }
    }
}

thread_local! {
    /// The counter, for the robot functions below (free functions: they
    /// can't capture the `app()` local).
    static COUNT: std::cell::Cell<Option<Signal<i32>>> = const { std::cell::Cell::new(None) };
}

thread_local! {
    static COVERED: std::cell::Cell<Option<Signal<Option<bool>>>> = const { std::cell::Cell::new(None) };
}

thread_local! {
    static DIALOG: std::cell::Cell<Option<Signal<bool>>> = const { std::cell::Cell::new(None) };
}

/// Open (or close) the [`dialog`].
#[runtime_core::robot_fn(name = "demo.set_dialog")]
fn set_dialog(open: bool) -> Result<(), String> {
    let dialog = DIALOG.with(|c| c.get()).ok_or("the app has not mounted")?;
    dialog.set(open);
    Ok(())
}

/// A modal with a field in it, opened by `demo.set_dialog`: Escape (and
/// Back on Android) close it while the field holds focus, as they do with
/// nothing focused.
fn dialog(open: Signal<bool>) -> Element {
    let draft: Signal<String> = signal(String::new());
    ui! {
        if open.get() {
            overlay(on_dismiss = move || open.set(false)) {
                text_input(value = draft, on_change = move |s: String| draft.set(s), test_id = "dialog-field")
            }
        }
    }
}

/// Show (or hide) an overlay over the screen; `backdrop: false` shows it
/// without its backdrop.
#[runtime_core::robot_fn(name = "demo.set_cover")]
fn set_cover(on: bool, backdrop: Option<bool>) -> Result<(), String> {
    let covered = COVERED.with(|c| c.get()).ok_or("the app has not mounted")?;
    covered.set(on.then_some(backdrop.unwrap_or(true)));
    Ok(())
}

/// Set the counter to `value`.
#[runtime_core::robot_fn(name = "demo.set_count")]
fn set_count(value: i32) -> Result<i32, String> {
    let count = COUNT.with(|c| c.get()).ok_or("the app has not mounted")?;
    count.set(value);
    Ok(value)
}

/// Set the counter to `value` after `delay_ms` — an async robot function:
/// the bridge answers at once and the test polls until it is done.
#[runtime_core::robot_fn(name = "demo.set_count_later")]
async fn set_count_later(value: i32, delay_ms: u32) -> Result<i32, String> {
    Delay::new(delay_ms).await;
    set_count(value)
}

/// A future that resolves after `ms`, on the app's own timer
/// (`after_ms_detached`), woken through the backend's executor.
struct Delay {
    state: std::rc::Rc<std::cell::RefCell<(bool, Option<std::task::Waker>)>>,
}

impl Delay {
    fn new(ms: u32) -> Self {
        let state = std::rc::Rc::new(std::cell::RefCell::new((false, None::<std::task::Waker>)));
        let fire = state.clone();
        runtime_core::after_ms_detached(ms as i32, move || {
            let mut s = fire.borrow_mut();
            s.0 = true;
            if let Some(waker) = s.1.take() {
                waker.wake();
            }
        });
        Delay { state }
    }
}

impl std::future::Future for Delay {
    type Output = ();
    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        let mut s = self.state.borrow_mut();
        if s.0 {
            std::task::Poll::Ready(())
        } else {
            s.1 = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    }
}

/// How many tabs [`tab_strip`] holds before its last one.
const TAB_COUNT: i32 = 40;
/// What the last tab sets the counter to.
const LAST_TAB: i32 = 100;

/// A tab bar wider than any window, as a phone's scrolling tab strip is:
/// its last tab starts off screen, so a test reaches it only by scrolling
/// the strip — what `click()` does first.
fn tab_strip(count: Signal<i32>) -> Element {
    ui! {
        scroll_view(horizontal = true, test_id = "tabs") {
            view(style = tab_row_style()) {
                for i in 1..=TAB_COUNT {
                    button(label = format!("Tab {i}"), on_click = move || count.set(i))
                }
                button(label = "Last tab".to_string(), on_click = move || count.set(LAST_TAB), test_id = "tab-last")
            }
        }
    }
}

stylesheet! {
    pub TabRow<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            gap: Length::Px(12.0),
        }
    }
}

/// A field that submits on Enter, as a search box or chat input does: its
/// `on_key_down` takes the Enter and records the text in `submitted` (a
/// watched signal), so a test can tell a key press reached the field's own
/// handler — and that typed keys and Backspace edited the text first.
fn submit_field() -> Element {
    let note: Signal<String> = signal(String::new());
    let submitted: Signal<String> = signal(String::new());
    runtime_vocabulary::robot::watch_signal("note", note);
    runtime_vocabulary::robot::watch_signal("submitted", submitted);
    let on_key_down = move |e: &KeyEvent| {
        if e.key == "Enter" {
            submitted.set(note.get());
            KeyOutcome::PreventDefault
        } else {
            KeyOutcome::Default
        }
    };
    ui! {
        text_input(
            value = note,
            on_change = move |s: String| note.set(s),
            on_key_down = on_key_down,
            placeholder = "Press Enter to submit".to_string(),
            test_id = "note",
        )
    }
}

/// Two fields side by side in the focus order, nothing focusable between
/// them: Tab from the first lands on the second, Shift+Tab back, on every
/// platform's own traversal.
fn tab_fields() -> Element {
    ui! {
        view() {
            text_input(placeholder = "First".to_string(), test_id = "tab-first")
            text_input(placeholder = "Second".to_string(), test_id = "tab-second")
        }
    }
}

/// A field outside a view with `on_focus`, then a field inside it: the
/// view's keyboard focus-within is the watched `focus_within` signal, so a
/// test can Tab into and out of the view and read what it heard.
fn focus_within_fields() -> Element {
    let within: Signal<bool> = signal(false);
    runtime_vocabulary::robot::watch_signal("focus_within", within);
    ui! {
        view() {
            text_input(placeholder = "Outside".to_string(), test_id = "focus-outer")
            view(on_focus = move |focused: bool| within.set(focused)) {
                text_input(placeholder = "Inside".to_string(), test_id = "focus-inner")
            }
        }
    }
}

/// ~1s gives the first layout/paint time to settle before the suite runs.
const INITIAL_RUN_DELAY_MS: i32 = 1000;

/// What the name field's `on_focus` and `on_blur` reported.
#[derive(Clone, Copy)]
struct NameFocus {
    /// `on_focus`'s latest argument.
    focused: Signal<bool>,
    /// How many times `on_focus` ran.
    changes: Signal<i32>,
    /// How many times `on_blur` ran (it lets every blur through).
    blurs: Signal<i32>,
}

fn screen(count: Signal<i32>, show_secret: Signal<bool>, name: Signal<String>, name_focus: NameFocus) -> Element {
    // Button handlers. `Signal` is `Copy`, so each closure captures its
    // own copy.
    let inc = move || count.set(count.get() + 1);
    let dec = move || count.set(count.get() - 1);
    let toggle = move || show_secret.set(!show_secret.get());
    let on_focus = move |f: bool| {
        name_focus.focused.set(f);
        name_focus.changes.set(name_focus.changes.get() + 1);
    };
    let on_blur = move || {
        name_focus.blurs.set(name_focus.blurs.get() + 1);
        BlurOutcome::Allow
    };

    // The `secret` text mounts (and registers) only while `show_secret` is
    // true — so the E2E suite can assert it's *not* visible first, then
    // visible after a click. The labels are reactive: the registry
    // recomputes them on read, so the assertions see live values.
    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Lg) {
            Typography(content = "Robot E2E Demo".to_string(), kind = typography_kind::H1)
            Typography(
                content = "An in-app, Playwright-style E2E suite drives this UI and logs every step to the console. It auto-runs ~1s after launch.".to_string(),
                muted = true,
            )
            // — Counter —
            Typography(content = "Counter".to_string(), kind = typography_kind::H2)
            text(test_id = "counter") { move || format!("Counter: {}", count.get()) }
            button(label = "+ increment", on_click = inc, test_id = "inc")
            button(label = "- decrement", on_click = dec, test_id = "dec")
            // — Reveal panel —
            Typography(content = "Reveal".to_string(), kind = typography_kind::H2)
            button(label = "Toggle secret", on_click = toggle, test_id = "toggle-secret")
            if show_secret.get() {
                text(test_id = "secret") { "🔓 Secret unlocked" }
            }
            // — Tabs —
            tab_strip(count)
            // — Submit on Enter —
            submit_field()
            // — Tab order —
            tab_fields()
            // — Keyboard focus within a view —
            focus_within_fields()
            // — Name + greeting —
            Typography(content = "Greeting".to_string(), kind = typography_kind::H2)
            text_input(
                value = name,
                on_change = move |s: String| name.set(s),
                placeholder = "Type a name".to_string(),
                test_id = "name",
                on_focus = on_focus,
                on_blur = on_blur,
            )
            text(test_id = "greeting") {
                move || {
                    let n = name.get();
                    if n.is_empty() {
                        "Hello, stranger".to_string()
                    } else {
                        format!("Hello, {n}")
                    }
                }
            }
            // — Re-run —
            button(label = "Run E2E suite again", on_click = run_suite, test_id = "run-suite")
        }
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
            // By test_id, not by counting every Button on screen: the
            // screen also carries the tab strip and other fixtures, whose
            // buttons are not this suite's concern.
            test("the screen exposes its four control buttons", |page: &Page| {
                for id in ["inc", "dec", "toggle-secret", "run-suite"] {
                    expect(&page.get_by_test_id(id)).to_have_count(1)?;
                }
                Ok(())
            }),
        ],
    );
}

#[cfg(not(feature = "robot"))]
fn run_suite() {}

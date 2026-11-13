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
    signal, stylesheet, text, ui, BlurOutcome, Element, FlexDirection, IntoElement, JustifyContent,
    KeyEvent, KeyOutcome, KeyboardKind, Length, Overflow, Signal, StyleRules,
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
    runtime_vocabulary::robot_fn::register(page_cookies::robot());

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
    let tall_page: Signal<bool> = signal(false);
    TALL_PAGE.with(|c| c.set(Some(tall_page)));
    runtime_vocabulary::robot_fn::register(set_tall_page::robot());
    runtime_vocabulary::robot_fn::register(page_scroll_y::robot());

    // The screen is taller than a phone's window, so it scrolls: a
    // vertical `scroll_view` filling the window holds it, as an app's
    // screen does on every platform — a native window doesn't scroll
    // content that overflows it, and a test reaches what is below the
    // fold by scrolling it (`click()` does).
    ui! {
        view() {
            scroll_view(style = PageScroll(), test_id = "page") {
                screen(count, show_secret, name, name_focus)
            }
            tall_page_end(tall_page)
            cover
            dialog(dialog_open)
        }
    }
}

thread_local! {
    static TALL_PAGE: std::cell::Cell<Option<Signal<bool>>> = const { std::cell::Cell::new(None) };
}

/// Show (or hide) [`tall_page_end`].
#[runtime_core::robot_fn(name = "demo.set_tall_page")]
fn set_tall_page(on: bool) -> Result<(), String> {
    let tall = TALL_PAGE.with(|c| c.get()).ok_or("the app has not mounted")?;
    tall.set(on);
    Ok(())
}

/// How far the web page itself (the document) is scrolled, in CSS px. Web
/// only: a native window doesn't scroll.
#[runtime_core::robot_fn(name = "demo.page_scroll_y")]
fn page_scroll_y() -> Result<f64, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let window = web_glue::JsValue::global();
        window
            .get("scrollY")
            .map_err(|e| format!("{e:?}"))?
            .as_f64()
            .ok_or_else(|| "window.scrollY is not a number".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Err("the page scrolls only on web".into())
    }
}

/// While `on`, a block taller than any window under the screen's scroll
/// view, outside it, with a button at its bottom: content no `scroll_view`
/// holds. On the web the document itself scrolls to reach the button —
/// the path `regression_click_reaches_a_button_below_the_fold_of_a_page`
/// tests. A native window doesn't scroll, so there it stays out of reach.
fn tall_page_end(on: Signal<bool>) -> Element {
    let taps: Signal<i32> = signal(0);
    runtime_vocabulary::robot::watch_signal("page_bottom_taps", taps);
    ui! {
        if on.get() {
            view(style = TallPage()) {
                button(label = "Page bottom", on_click = move || taps.set(taps.get() + 1), test_id = "page-bottom")
            }
        }
    }
}

stylesheet! {
    pub PageScroll<IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            flex_basis: Length::Px(0.0),
            min_height: Length::Px(0.0),
        }
    }
}

/// Taller than any window this demo runs at.
const TALL_PAGE_HEIGHT: f32 = 3000.0;

stylesheet! {
    pub TallPage<IdeaThemeRef> {
        base(_t) {
            flex_shrink: 0.0,
            min_height: Length::Px(TALL_PAGE_HEIGHT),
            justify_content: JustifyContent::FlexEnd,
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
    ui! {
        if open.get() {
            overlay(on_dismiss = move || open.set(false)) {
                dialog_field()
            }
        }
    }
}

/// The dialog's field, built fresh each time the dialog opens (its text
/// does not outlive the dialog). While it reads "keep" it claims Escape for
/// itself, as a field closing its own dropdown would, so the dialog must
/// stay open.
fn dialog_field() -> Element {
    let draft: Signal<String> = signal(String::new());
    let on_key_down = move |e: &KeyEvent| {
        if e.key == "Escape" && draft.get() == "keep" {
            KeyOutcome::PreventDefault
        } else {
            KeyOutcome::Default
        }
    };
    ui! {
        text_input(
            value = draft,
            on_change = move |s: String| draft.set(s),
            on_key_down = on_key_down,
            test_id = "dialog-field",
        )
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

/// The cookies the page's scripts can read (`document.cookie`): the
/// script-visible half of what the browser sends the server. Web only; a
/// native app's HTTP stack keeps its own cookies.
#[runtime_core::robot_fn(name = "demo.page_cookies")]
fn page_cookies() -> Result<String, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let document = web_glue::JsValue::global().get("document").map_err(|e| format!("{e:?}"))?;
        document
            .get("cookie")
            .map_err(|e| format!("{e:?}"))?
            .as_string()
            .ok_or_else(|| "document.cookie is not a string".to_string())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Err("page cookies exist only on web".into())
    }
}

/// A list row with a context menu, as a list app writes one: a right click
/// opens it with a mouse, a long press with a finger. One touch handler
/// takes both: a secondary button's press is the right click, any other
/// press goes to the long-press recognizer. Each way it opened is counted
/// (`menu_right_clicks`, `menu_long_presses`) and the last one is shown.
fn context_row() -> Element {
    use runtime_core::{long_press, pointer_button, LongPressRecognizer, TouchEvent, TouchPhase, TouchResponse};
    let right_clicks: Signal<i32> = signal(0);
    let long_presses: Signal<i32> = signal(0);
    let last: Signal<&'static str> = signal("closed");
    runtime_vocabulary::robot::watch_signal("menu_right_clicks", right_clicks);
    runtime_vocabulary::robot::watch_signal("menu_long_presses", long_presses);
    let held = long_press(LongPressRecognizer::new(), move || {
        long_presses.set(long_presses.get() + 1);
        last.set("long press");
    });
    let on_touch = move |e: &TouchEvent| {
        if pointer_button().opens_context_menu() {
            if e.phase == TouchPhase::Began {
                right_clicks.set(right_clicks.get() + 1);
                last.set("right click");
            }
            return TouchResponse::CONSUMED;
        }
        held(e)
    };
    ui! {
        view(on_touch = on_touch, test_id = "menu-row") {
            text(test_id = "menu-state") { move || format!("Row menu: {}", last.get()) }
        }
    }
}

/// A to-do the user marks done — struck through once done — and an email
/// field, so a test can read an element's resolved style (`get_style`) and a
/// field's soft-keyboard kind.
fn todo_section() -> Element {
    let done: Signal<bool> = signal(false);
    runtime_vocabulary::robot::watch_signal("todo_done", done);
    ui! {
        view() {
            text(test_id = "todo-item", style = move || todo_style(done.get())) { "Buy milk" }
            button(label = "Mark done", on_click = move || done.set(!done.get()), test_id = "todo-toggle")
            text_input(placeholder = "Email".to_string(), keyboard = KeyboardKind::Email, test_id = "email")
        }
    }
}

/// A to-do's text: struck through once done.
fn todo_style(done: bool) -> StyleRules {
    StyleRules { strikethrough: Some(done), ..Default::default() }
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
    pub NavSection<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
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

    // Taps on the button at the end of the screen, below the fold of a
    // phone's window: reached by scrolling the screen's `scroll_view`.
    let page_end_taps: Signal<i32> = signal(0);
    runtime_vocabulary::robot::watch_signal("page_end_taps", page_end_taps);

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
            // — A to-do with an email field —
            todo_section()
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
            // — A row's context menu —
            context_row()
            // — Re-run —
            button(label = "Run E2E suite again", on_click = run_suite, test_id = "run-suite")
            // — Deep links through navigators that mount late —
            deep_link_navigators()
            // — A layout that changes at `md` —
            responsive_section()
            // — The end of a screen taller than a phone's window —
            button(label = "Page end", on_click = move || page_end_taps.set(page_end_taps.get() + 1), test_id = "page-end")
        }
    }
}

/// A layout that changes at `md`, the shapes the robot has to measure
/// across a resize: a step rail its base style collapses (zero high,
/// clipped) and its `md` overlay opens, and a panel stacked under a record
/// taller than a phone's window, beside it from `md`.
fn responsive_section() -> Element {
    ui! {
        view(style = SplitLayout()) {
            view(style = StepRail(), test_id = "rail") { text { "Steps" } }
            view(style = RecordBox(), test_id = "record") { text { "Record" } }
            view(style = PanelBox(), test_id = "panel") { text { "Panel" } }
        }
    }
}

/// Taller than a phone's window, so the panel under it starts below the fold.
const RECORD_HEIGHT: f32 = 900.0;

stylesheet! {
    pub SplitLayout<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
        }
        breakpoint md(_t) {
            flex_direction: FlexDirection::Row,
        }
    }
}

stylesheet! {
    pub StepRail<IdeaThemeRef> {
        base(_t) {
            height: Length::Px(0.0),
            min_height: Length::Px(0.0),
            overflow: Overflow::Hidden,
        }
        breakpoint md(_t) {
            height: Length::Auto,
            min_height: Length::Auto,
            width: Length::Px(160.0),
            overflow: Overflow::Visible,
        }
    }
}

stylesheet! {
    pub RecordBox<IdeaThemeRef> {
        base(_t) {
            flex_shrink: 0.0,
            height: Length::Px(RECORD_HEIGHT),
        }
    }
}

stylesheet! {
    pub PanelBox<IdeaThemeRef> {
        base(_t) {
            flex_shrink: 0.0,
            height: Length::Px(120.0),
        }
    }
}

const NAV_HOME: runtime_vocabulary::glue::Route<()> = runtime_vocabulary::glue::Route::new("home", "/");
const NAV_PROJECTS: runtime_vocabulary::glue::Route<()> =
    runtime_vocabulary::glue::Route::new("projects", "/projects");
const NAV_LIST: runtime_vocabulary::glue::Route<()> = runtime_vocabulary::glue::Route::new("list", "");
const NAV_PROJECT: runtime_vocabulary::glue::Route<()> = runtime_vocabulary::glue::Route::new("project", "/:id");
const NAV_OVERVIEW: runtime_vocabulary::glue::Route<()> = runtime_vocabulary::glue::Route::new("overview", "");
const NAV_SCHEDULE: runtime_vocabulary::glue::Route<()> =
    runtime_vocabulary::glue::Route::new("schedule", "/schedule");
/// How long a screen "loads its data" before it shows its nested
/// navigator — long enough that the navigator mounts on a later event-loop
/// turn than the link's navigation.
const NAV_LOAD_MS: i32 = 150;

/// Navigators nested three deep, each level built only once its screen's
/// data "arrives": `/projects/:id/schedule` from anywhere must open in one
/// `open_link` call, as a cold load of the URL would, and the screen's
/// query (a filter) is readable on the navigator state.
///
/// Styled to hug its content: a navigator's default is to fill its
/// parent (`height: 100%`, `flex-grow: 1`), which in this page's column
/// would take the window's height and squeeze the shrinkable tab strip
/// above it to nothing.
fn deep_link_navigators() -> Element {
    runtime_vocabulary::builders::swap_navigator(&NAV_HOME)
        .style(nav_section_style())
        .screen(NAV_HOME, |_| ui! { text(test_id = "nav-home") { "Projects closed" } })
        .screen(NAV_PROJECTS, |_| {
            after_load(|| {
                runtime_vocabulary::builders::stack_navigator(&NAV_LIST)
                    .screen(NAV_LIST, |_| ui! { text(test_id = "nav-list") { "Project list" } })
                    .screen(NAV_PROJECT, |_| {
                        after_load(|| {
                            runtime_vocabulary::builders::swap_navigator(&NAV_OVERVIEW)
                                .screen(NAV_OVERVIEW, |_| ui! { text(test_id = "nav-overview") { "Overview" } })
                                .screen(NAV_SCHEDULE, |_| ui! { text(test_id = "nav-schedule") { "Schedule" } })
                                .build()
                        })
                    })
                    .build()
            })
        })
        .build()
}

/// `body` once a simulated fetch lands ([`NAV_LOAD_MS`]); a loading line
/// until then.
fn after_load(body: impl Fn() -> Element + 'static) -> Element {
    let loaded: Signal<bool> = signal(false);
    runtime_core::after_ms_scoped(NAV_LOAD_MS, move || loaded.set(true));
    ui! {
        if loaded.get() {
            body()
        } else {
            text() { "Loading…" }
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

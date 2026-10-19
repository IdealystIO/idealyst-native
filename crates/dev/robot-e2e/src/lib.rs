//! `robot-e2e` — a shared, **Playwright-flavoured E2E harness** built on the
//! framework's in-process [`runtime_vocabulary::robot`] API.
//!
//! One harness, every backend: the same `locate → act → assert` vocabulary
//! Playwright gives you for the web, but driving *our* introspection
//! registry instead of a browser — so an identical suite runs on web, iOS,
//! Android, macOS, and the terminal with no per-platform runner.
//!
//! | Playwright                 | here                          | Robot call          |
//! |----------------------------|-------------------------------|---------------------|
//! | `page.getByTestId`         | [`Page::get_by_test_id`]      | `find(TestId)`      |
//! | `page.getByText`           | [`Page::get_by_text`]         | `find(LabelContains)`|
//! | `page.getByRole`           | [`Page::get_by_role`]         | `find(Kind)`        |
//! | `locator.click()`          | [`Locator::click`]            | `Robot::click`      |
//! | `locator.fill()`           | [`Locator::fill`]             | `Robot::type_text`  |
//! | `locator.setChecked()`     | [`Locator::set_toggle`]       | `Robot::set_toggle` |
//! | (slider drag)              | [`Locator::set_slider`]       | `Robot::set_slider` |
//! | `mouse.down/move/up`       | [`Locator::drag`]             | `Robot::drag`       |
//! | `locator.focus()/.blur()`  | [`Locator::focus`]/[`blur`]   | `Robot::focus`/`blur`|
//! | `locator.hover()`          | [`Locator::hover`]            | `Robot::hover_native` |
//! | `mouse.down()/up()`        | [`Locator::press`]/[`release`]| `Robot::press_native`/`release_native` |
//! | `expect(l).toBeFocused()`  | [`Expect::to_be_focused`]     | `state().focused`   |
//! | `expect(l).toHaveValue()`  | [`Expect::to_have_value`]     | `state().value`     |
//! | (`:hover` / `:active`)     | [`Expect::to_be_hovered`]/[`to_be_pressed`] | `state().hovered`/`pressed` |
//! | `expect(l).toBeVisible()`  | [`Expect::to_be_visible`]     | `visibility().visible` |
//! | `expect(l).toHaveText()`   | [`Expect::to_have_text`]      | live `label`        |
//! | `expect(l).toHaveCount()`  | [`Expect::to_have_count`]     | `find_all().len()`  |
//!
//! ## Output
//!
//! Every action and assertion logs an `[e2e]` line to the platform console
//! (`console.*` on web, `__android_log_print` on Android, `NSLog`/stderr on
//! Apple, stderr in the terminal). At the end of a [`run_suites`] run the
//! harness emits **one machine-readable line**:
//!
//! ```text
//! [E2E-RESULT] {"suites":3,"tests":42,"pass":40,"fail":0,"skip":2,"failures":[],"skipped":[…]}
//! ```
//!
//! A test **skips** instead of failing when what it checks does not exist
//! on the platform — a touch screen cannot hover, CSS owns hover on web.
//! Native actions turn the robot's `Unsupported` into a skip on their own;
//! a test can also return [`skip`] itself. A skip is not a pass: the
//! summary counts it separately, with its reason.
//!
//! [`set_result_listener`] hands each test's outcome to the app as it
//! finishes, so a screen can show the run live (the conformance app's
//! system check does).
//!
//! A cross-platform orchestrator launches the app on each target, scrapes
//! that target's log channel for `[E2E-RESULT]`, and aggregates the verdict
//! — no per-platform driver needed.
//!
//! Assertions read the **live** registry label ([`Robot`] recomputes
//! reactive text on read), so a `toHaveText` immediately after a `click`
//! sees the post-update value with no explicit wait.
//!
//! [`blur`]: Locator::blur
//! [`release`]: Locator::release
//! [`to_be_pressed`]: Expect::to_be_pressed

// The introspection registry: `runtime_vocabulary::robot`.
// `ElementKind` is re-exported so suites written against the harness can
// spell `page.get_by_role(ElementKind::Button)` without naming the
// registry crate themselves.
use runtime_vocabulary::robot::{Element, ElementState, Query, Robot, RobotError, StateValue};
pub use runtime_vocabulary::robot::ElementKind;

/// Pacing between tests, in ms. Purely cosmetic: it lets the on-screen UI
/// visibly change and the console stream one test at a time, like watching
/// a real test run. Actions *within* a test are synchronous — the registry
/// recomputes reactive labels on read, so no intra-test wait is needed.
const STEP_PACING_MS: i32 = 180;

/// Flush any work the last action *deferred* to a microtask — navigator
/// screen builds, reactive effects bound through `schedule_microtask`, etc.
/// — so the next assertion sees the settled tree.
///
/// This is what makes the synchronous `act → assert` chain robust against
/// the framework's deferral points: a `pop()` that unmounts its screen on a
/// microtask, a `when` branch whose effect rebuild is buffered, and so on.
/// Without it, an assertion immediately after such an action can race the
/// deferred work and read the pre-action tree (the stack-pop "detail screen
/// still present" flake). Drains run synchronously on the calling (UI)
/// thread, mirroring what the browser does between tasks.
fn settle() {
    // Robot ACTIONS already settle themselves (the vocabulary Robot's
    // driver env flushes staged writes before returning), but
    // `Page::settle` is also the caller's tool after a DIRECT signal
    // write — flush that staged work too before draining microtasks.
    runtime_vocabulary::robot::settle();
    // Drain repeatedly: a deferred unit of work can schedule *another*
    // microtask (e.g. a stack pop's `release_screen` drops a scope, whose
    // `on_cleanup` deregisters robot entries on a follow-up tick). A single
    // drain flushes one wave; loop until the queue stays empty so cascading
    // teardown fully settles before the next assertion. Bounded so a
    // pathological self-rescheduling task can't spin forever.
    for _ in 0..SETTLE_MAX_DRAINS {
        runtime_shared::drain_buffered_microtasks();
    }
}

/// Upper bound on cascading microtask waves a single `settle()` flushes.
/// Teardown chains in practice are 2–3 deep; 8 is comfortable headroom
/// without risking an unbounded spin on a self-rescheduling task.
const SETTLE_MAX_DRAINS: usize = 8;

/// `[e2e]`-prefixed info line to the platform console.
fn log(line: impl AsRef<str>) {
    runtime_shared::log_info!("[e2e] {}", line.as_ref());
}

/// `[e2e]`-prefixed error line (failed assertions / actions).
fn log_fail(line: impl AsRef<str>) {
    runtime_shared::log_error!("[e2e] {}", line.as_ref());
}

// ---------------------------------------------------------------------------
// Skips + results
// ---------------------------------------------------------------------------

/// Marks an `Err` as a skip rather than a failure. Not printable, so no
/// real failure message starts with it.
const SKIP_MARK: &str = "\u{1}skip\u{1}";

/// The `Err` a test returns to skip: what it checks does not exist on this
/// platform (`return Err(skip("no hover on a touch screen"))`).
pub fn skip(reason: impl AsRef<str>) -> String {
    format!("{SKIP_MARK}{}", reason.as_ref())
}

fn skip_reason(e: &str) -> Option<&str> {
    e.strip_prefix(SKIP_MARK)
}

/// A robot error as a test `Err`: `Unsupported` (the platform has no such
/// input) is a skip, anything else a failure.
fn robot_err(desc: &str, action: &str, e: RobotError) -> String {
    match e {
        RobotError::Unsupported(what) => skip(format!("{desc}.{action}(): {what} is not available on this platform")),
        e => format!("{desc}.{action}() failed: {e:?}"),
    }
}

/// How a test ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::Skip => "skip",
        }
    }
}

/// One finished test, as [`set_result_listener`] reports it.
#[derive(Clone, Debug)]
pub struct TestResult {
    pub suite: String,
    pub test: String,
    pub status: Status,
    /// The failure message, or the skip reason.
    pub detail: Option<String>,
}

type Listener = std::rc::Rc<dyn Fn(&TestResult)>;

runtime_tls::thread_local! {
    static LISTENER: std::cell::RefCell<Option<Listener>> = const { std::cell::RefCell::new(None) };
}

/// Hand every finished test to `f` (UI thread), replacing any previous
/// listener. `None` stops reporting.
pub fn set_result_listener(f: Option<Listener>) {
    LISTENER.with(|l| *l.borrow_mut() = f);
}

fn report(result: TestResult) {
    let f = LISTENER.with(|l| l.borrow().clone());
    if let Some(f) = f {
        f(&result);
    }
}

// ---------------------------------------------------------------------------
// Page + Locator
// ---------------------------------------------------------------------------

/// The entry point — Playwright's `page`. A zero-cost handle to the
/// current process's Robot registry.
pub struct Page;

impl Page {
    pub fn new() -> Self {
        Page
    }

    /// Locate by `test_id` (the durable, refactor-proof hook).
    pub fn get_by_test_id(&self, id: &str) -> Locator {
        Locator::new(Sel::TestId(id.to_string()), format!("getByTestId({id:?})"))
    }

    /// Locate by visible text (substring match against the live label).
    pub fn get_by_text(&self, text: &str) -> Locator {
        Locator::new(Sel::Text(text.to_string()), format!("getByText({text:?})"))
    }

    /// Locate by primitive kind, e.g. every `Button`.
    pub fn get_by_role(&self, kind: ElementKind) -> Locator {
        Locator::new(Sel::Role(kind), format!("getByRole({kind:?})"))
    }

    /// Flush deferred work (microtask-buffered navigator builds, reactive
    /// effects) so a following assertion sees the settled tree. Actions
    /// already settle automatically; call this only when a *non-action*
    /// triggered deferred work (e.g. a signal you wrote directly).
    pub fn settle(&self) {
        settle();
    }
}

impl Default for Page {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
enum Sel {
    TestId(String),
    Text(String),
    Role(ElementKind),
}

/// A located (or to-be-located) element — Playwright's `Locator`. Lazy:
/// it re-resolves against the live registry on each action/assertion, so
/// it survives re-renders just like a Playwright locator survives DOM
/// churn.
#[derive(Clone)]
pub struct Locator {
    sel: Sel,
    /// Human-readable form for the console, e.g. `getByTestId('inc')`.
    desc: String,
}

impl Locator {
    fn new(sel: Sel, desc: String) -> Self {
        Locator { sel, desc }
    }

    fn query(&self) -> Query {
        match &self.sel {
            Sel::TestId(s) => Query::test_id(s.clone()),
            Sel::Text(s) => Query::label_contains(s.clone()),
            Sel::Role(k) => Query::kind(*k),
        }
    }

    fn resolve(&self) -> Result<Element, String> {
        Robot::new()
            .find(self.query())
            .ok_or_else(|| format!("{} resolved to no element", self.desc))
    }

    /// Click / press the element (buttons, pressables, links).
    pub fn click(&self) -> Result<(), String> {
        log(format!("  ▸ {}.click()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .click(&el)
            .map_err(|e| format!("{}.click() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Type text into a text input (clears + sets, like Playwright `fill`).
    pub fn fill(&self, text: &str) -> Result<(), String> {
        log(format!("  ▸ {}.fill({text:?})", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .type_text(&el, text)
            .map_err(|e| format!("{}.fill() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Set a toggle's checked state (Playwright `setChecked`).
    pub fn set_toggle(&self, value: bool) -> Result<(), String> {
        log(format!("  ▸ {}.set_toggle({value})", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .set_toggle(&el, value)
            .map_err(|e| format!("{}.set_toggle() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Set a slider's value (simulates a drag to `value`).
    pub fn set_slider(&self, value: f32) -> Result<(), String> {
        log(format!("  ▸ {}.set_slider({value})", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .set_slider(&el, value)
            .map_err(|e| format!("{}.set_slider() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Drag one pointer along `points` (element-relative logical px):
    /// draw on a signature pad, drag a handle. See `Robot::drag`.
    pub fn drag(&self, points: &[(f32, f32)]) -> Result<(), String> {
        self.drag_with_hold(points, 0)
    }

    /// [`drag`](Self::drag), holding at the first point for `hold_ms`
    /// first (a long-press pickup). See `Robot::drag_with_hold`.
    pub fn drag_with_hold(&self, points: &[(f32, f32)], hold_ms: u64) -> Result<(), String> {
        log(format!("  ▸ {}.drag({} points, hold {hold_ms} ms)", self.desc, points.len()));
        let el = self.resolve()?;
        let r = Robot::new()
            .drag_with_hold(&el, points, hold_ms)
            .map_err(|e| format!("{}.drag() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Move a real mouse over the element (Playwright `hover`). Skips on a
    /// platform with no hovering pointer.
    pub fn hover(&self) -> Result<(), String> {
        log(format!("  ▸ {}.hover()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new().hover_native(&el).map_err(|e| robot_err(&self.desc, "hover", e));
        settle();
        r
    }

    /// Press the primary button down on the element with real input and
    /// hold it (Playwright `mouse.down`). [`release`](Self::release) lifts
    /// it. Input may arrive on the next event-loop turn: check the pressed
    /// state from a `poll` step.
    pub fn press(&self) -> Result<(), String> {
        log(format!("  ▸ {}.press()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new().press_native(&el).map_err(|e| robot_err(&self.desc, "press", e));
        settle();
        r
    }

    /// Lift a [`press`](Self::press) at the element.
    pub fn release(&self) -> Result<(), String> {
        log(format!("  ▸ {}.release()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new().release_native(&el).map_err(|e| robot_err(&self.desc, "release", e));
        settle();
        r
    }

    /// Tap with real platform input (hit testing and overlays take part),
    /// where [`click`](Self::click) calls the handler directly.
    pub fn tap(&self) -> Result<(), String> {
        log(format!("  ▸ {}.tap()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new().tap_native(&el).map(|_| ()).map_err(|e| robot_err(&self.desc, "tap", e));
        settle();
        r
    }

    /// The element's own state (`Robot::state`), if it reports any.
    fn state(&self) -> Result<Option<ElementState>, String> {
        let el = self.resolve()?;
        Robot::new().state(&el).map_err(|e| format!("{}: {e:?}", self.desc))
    }

    /// Scroll a scroll view to the content offset `(x, y)`, logical px,
    /// through the native scroller (its scroll events fire as for a user).
    pub fn scroll_to(&self, x: f32, y: f32) -> Result<(), String> {
        log(format!("  ▸ {}.scroll_to({x}, {y})", self.desc));
        let el = self.resolve()?;
        let r = Robot::new().set_scroll(&el, x, y).map_err(|e| robot_err(&self.desc, "scroll_to", e));
        settle();
        r
    }

    /// Focus the element (text inputs).
    pub fn focus(&self) -> Result<(), String> {
        log(format!("  ▸ {}.focus()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .focus(&el)
            .map_err(|e| format!("{}.focus() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Blur the element.
    pub fn blur(&self) -> Result<(), String> {
        log(format!("  ▸ {}.blur()", self.desc));
        let el = self.resolve()?;
        let r = Robot::new()
            .blur(&el)
            .map_err(|e| format!("{}.blur() failed: {e:?}", self.desc));
        settle();
        r
    }

    /// Number of elements this locator currently matches.
    pub fn count(&self) -> usize {
        Robot::new().find_all(self.query()).len()
    }

    /// The element's laid-out rect in parent coordinates. `Err` if it isn't
    /// present or hasn't been laid out yet. Used to assert layout outcomes
    /// (e.g. that a content-sized container reflows when its children change).
    pub fn frame(&self) -> Result<runtime_shared::primitives::portal::ViewportRect, String> {
        let el = self.resolve()?;
        Robot::new()
            .frame(&el)
            .map_err(|e| format!("{}.frame() failed: {e:?}", self.desc))?
            .ok_or_else(|| format!("{}.frame() — element not laid out yet", self.desc))
    }

    /// The element's laid-out height in parent coordinates.
    pub fn height(&self) -> Result<f32, String> {
        Ok(self.frame()?.height)
    }

    /// The element's laid-out width in parent coordinates.
    pub fn width(&self) -> Result<f32, String> {
        Ok(self.frame()?.width)
    }
}

// ---------------------------------------------------------------------------
// Assertions — expect(...)
// ---------------------------------------------------------------------------

/// Wrap a locator in an assertion context — Playwright's `expect(...)`.
pub fn expect(locator: &Locator) -> Expect {
    Expect {
        loc: locator.clone(),
    }
}

pub struct Expect {
    loc: Locator,
}

impl Expect {
    /// `expect(locator).toBeVisible()` — the element is mounted and visible
    /// (shown by the toolkit, with a non-empty box).
    pub fn to_be_visible(self) -> Result<(), String> {
        let el = self
            .loc
            .resolve()
            .map_err(|_| format!("expect({}).toBeVisible() — element not found", self.loc.desc))?;
        if !visible(&el) {
            return Err(format!("expect({}).toBeVisible() — mounted but not visible", self.loc.desc));
        }
        log(format!("  ✓ expect({}).toBeVisible()", self.loc.desc));
        Ok(())
    }

    /// `expect(locator).not.toBeVisible()` — the element is absent, or
    /// mounted but not visible (a closed overlay, a navigated-away screen).
    pub fn not_to_be_visible(self) -> Result<(), String> {
        match self.loc.resolve() {
            Ok(el) if visible(&el) => Err(format!(
                "expect({}).not.toBeVisible() — element is visible",
                self.loc.desc
            )),
            _ => {
                log(format!("  ✓ expect({}).not.toBeVisible()", self.loc.desc));
                Ok(())
            }
        }
    }

    /// `expect(locator).toHaveText(expected)` — exact match against the
    /// element's live label.
    pub fn to_have_text(self, expected: &str) -> Result<(), String> {
        let el = self.loc.resolve()?;
        match el.label.as_deref() {
            Some(actual) if actual == expected => {
                log(format!(
                    "  ✓ expect({}).toHaveText({expected:?})",
                    self.loc.desc
                ));
                Ok(())
            }
            other => Err(format!(
                "expect({}).toHaveText({expected:?}) — actual text was {:?}",
                self.loc.desc, other
            )),
        }
    }

    /// `expect(locator).toContainText(substr)` — substring match against
    /// the element's live label.
    pub fn to_contain_text(self, substr: &str) -> Result<(), String> {
        let el = self.loc.resolve()?;
        match el.label.as_deref() {
            Some(actual) if actual.contains(substr) => {
                log(format!(
                    "  ✓ expect({}).toContainText({substr:?})",
                    self.loc.desc
                ));
                Ok(())
            }
            other => Err(format!(
                "expect({}).toContainText({substr:?}) — actual text was {:?}",
                self.loc.desc, other
            )),
        }
    }

    /// `expect(locator).toBeHovered()` — the element's `state hovered`
    /// style is on. Skips where the platform keeps hover in CSS (web),
    /// which the robot cannot read.
    pub fn to_be_hovered(self) -> Result<(), String> {
        self.interaction("toBeHovered", true, |s| s.hovered)
    }

    pub fn not_to_be_hovered(self) -> Result<(), String> {
        self.interaction("not.toBeHovered", false, |s| s.hovered)
    }

    /// `expect(locator).toBePressed()` — the element's `state pressed`
    /// style is on. Skips as [`to_be_hovered`](Self::to_be_hovered) does.
    pub fn to_be_pressed(self) -> Result<(), String> {
        self.interaction("toBePressed", true, |s| s.pressed)
    }

    pub fn not_to_be_pressed(self) -> Result<(), String> {
        self.interaction("not.toBePressed", false, |s| s.pressed)
    }

    fn interaction(&self, what: &str, want: bool, pick: fn(&ElementState) -> Option<bool>) -> Result<(), String> {
        let desc = &self.loc.desc;
        match self.loc.state()?.as_ref().and_then(pick) {
            Some(actual) if actual == want => {
                log(format!("  ✓ expect({desc}).{what}()"));
                Ok(())
            }
            Some(_) => Err(format!("expect({desc}).{what}() — it is {}", if want { "not" } else { "still" })),
            None => Err(skip(format!(
                "expect({desc}).{what}(): no interaction state to read (styled by the platform, e.g. CSS on web, \
                 or the element's style has no state variants)"
            ))),
        }
    }

    /// `expect(locator).toBeFocused()` — a text field holds focus.
    pub fn to_be_focused(self) -> Result<(), String> {
        self.focus_is(true)
    }

    pub fn not_to_be_focused(self) -> Result<(), String> {
        self.focus_is(false)
    }

    fn focus_is(&self, want: bool) -> Result<(), String> {
        let desc = &self.loc.desc;
        let what = if want { "toBeFocused" } else { "not.toBeFocused" };
        match self.loc.state()?.and_then(|s| s.focused) {
            Some(f) if f == want => {
                log(format!("  ✓ expect({desc}).{what}()"));
                Ok(())
            }
            other => Err(format!("expect({desc}).{what}() — focused is {other:?}")),
        }
    }

    /// `expect(locator).toHaveValue(text)` — a text field's text as the
    /// native widget holds it.
    pub fn to_have_value(self, expected: &str) -> Result<(), String> {
        let desc = &self.loc.desc;
        match self.loc.state()?.and_then(|s| s.value) {
            Some(StateValue::Text(t)) if t == expected => {
                log(format!("  ✓ expect({desc}).toHaveValue({expected:?})"));
                Ok(())
            }
            other => Err(format!("expect({desc}).toHaveValue({expected:?}) — value is {other:?}")),
        }
    }

    /// The text the NATIVE widget displays (`Robot::introspect_native`,
    /// the first `text` in its native subtree) equals `expected`. Where
    /// [`to_have_value`](Self::to_have_value) reads the robot's own record of
    /// the field — which follows the bound value whether or not the widget
    /// did — this reads the widget, so it catches a backend that drops a
    /// value written from code. Skips on a backend with no native readback.
    pub fn to_show_native_text(self, expected: &str) -> Result<(), String> {
        use runtime_shared::introspect::{keys, NativeNode, NativeValue};
        fn first_text(n: &NativeNode) -> Option<&str> {
            match n.props.get(keys::TEXT) {
                Some(NativeValue::Text(t)) => Some(t),
                _ => n.children.iter().find_map(first_text),
            }
        }
        let desc = &self.loc.desc;
        let el = self.loc.resolve()?;
        let node = match Robot::new().introspect_native(&el) {
            Ok(Some(node)) => node,
            Ok(None) | Err(RobotError::ActionNotAvailable(_)) => {
                return Err(skip(format!("expect({desc}).toShowNativeText(): no native readback on this platform")))
            }
            Err(e) => return Err(format!("expect({desc}).toShowNativeText(): {e:?}")),
        };
        match first_text(&node) {
            Some(t) if t == expected => {
                log(format!("  ✓ expect({desc}).toShowNativeText({expected:?})"));
                Ok(())
            }
            other => Err(format!("expect({desc}).toShowNativeText({expected:?}) — the widget shows {other:?}")),
        }
    }

    /// `expect(locator).toHaveCount(n)` — number of matches.
    pub fn to_have_count(self, n: usize) -> Result<(), String> {
        let actual = self.loc.count();
        if actual == n {
            log(format!("  ✓ expect({}).toHaveCount({n})", self.loc.desc));
            Ok(())
        } else {
            Err(format!(
                "expect({}).toHaveCount({n}) — actual count was {actual}",
                self.loc.desc
            ))
        }
    }

    /// `expect(locator).toHaveMinCount(n)` — at least `n` matches. Useful
    /// when a screen renders an unknown-but-bounded number of a kind.
    pub fn to_have_min_count(self, n: usize) -> Result<(), String> {
        let actual = self.loc.count();
        if actual >= n {
            log(format!(
                "  ✓ expect({}).toHaveMinCount({n}) (actual {actual})",
                self.loc.desc
            ));
            Ok(())
        } else {
            Err(format!(
                "expect({}).toHaveMinCount({n}) — actual count was {actual}",
                self.loc.desc
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Test + Suite + runner
// ---------------------------------------------------------------------------

/// One named test case — Playwright's `test('name', async ({page}) => …)`.
///
/// Two shapes:
/// - **Sync** ([`test`]): one closure run synchronously, `?`-chaining
///   actions and assertions. Right for purely-synchronous UI (reactive
///   signals, `when` branches — they settle by the next line).
/// - **Flow** ([`flow`]): an ordered list of steps the runner advances
///   **one per real scheduler tick**, so macrotask-/event-driven work (a
///   stack `pop` that completes on the browser's async `popstate`) settles
///   between an action and the assertion that checks it. `poll` steps retry
///   across ticks until they pass or time out (Playwright's auto-waiting).
pub struct Test {
    name: String,
    body: Body,
}

enum Body {
    Sync(Box<dyn Fn(&Page) -> Result<(), String>>),
    Flow(Vec<Step>),
}

struct Step {
    kind: StepKind,
    run: Box<dyn Fn(&Page) -> Result<(), String>>,
}

enum StepKind {
    /// Runs once; an `Err` fails the test immediately. Use for actions and
    /// synchronous assertions.
    Once,
    /// Re-runs across real ticks until it returns `Ok` or the attempt budget
    /// is exhausted (then the last `Err` fails the test). Use ONLY for
    /// side-effect-free assertions — a `poll` body re-executes, so it must
    /// not click/fill.
    Poll,
}

/// Define a synchronous test. The body uses `?` to bail on the first failed
/// action/assertion — same as `await expect(...)` throwing in Playwright.
pub fn test(
    name: &str,
    body: impl Fn(&Page) -> Result<(), String> + 'static,
) -> Test {
    Test {
        name: name.to_string(),
        body: Body::Sync(Box::new(body)),
    }
}

/// Begin a **flow** test — a sequence of steps advanced one per real
/// scheduler tick, so asynchronous (event-/timer-driven) UI changes settle
/// between steps. Chain [`Flow::act`] (run-once action/assertion) and
/// [`Flow::poll`] (assertion retried until it passes), then [`Flow::build`].
///
/// ```ignore
/// flow("push then pop")
///     .act(|p| p.get_by_test_id("push-detail").click())
///     .act(|p| expect(&p.get_by_test_id("detail-marker")).to_be_visible())
///     .act(|p| p.get_by_test_id("back").click())
///     // pop completes on async popstate — poll until the screen is gone:
///     .poll(|p| expect(&p.get_by_test_id("detail-marker")).not_to_be_visible())
///     .build()
/// ```
pub fn flow(name: &str) -> Flow {
    Flow {
        name: name.to_string(),
        steps: Vec::new(),
    }
}

/// Builder for a [`Body::Flow`] test — see [`flow`].
pub struct Flow {
    name: String,
    steps: Vec<Step>,
}

impl Flow {
    /// A run-once step: an action (`click`/`fill`/…) or a synchronous
    /// assertion. An `Err` fails the test immediately.
    pub fn act(mut self, body: impl Fn(&Page) -> Result<(), String> + 'static) -> Self {
        self.steps.push(Step {
            kind: StepKind::Once,
            run: Box::new(body),
        });
        self
    }

    /// A retried assertion: re-run across real ticks until it returns `Ok`
    /// or the attempt budget runs out. MUST be side-effect-free (no
    /// `click`/`fill` — the body re-executes).
    pub fn poll(mut self, body: impl Fn(&Page) -> Result<(), String> + 'static) -> Self {
        self.steps.push(Step {
            kind: StepKind::Poll,
            run: Box::new(body),
        });
        self
    }

    /// Finish the flow into a [`Test`] for a [`suite`].
    pub fn build(self) -> Test {
        Test {
            name: self.name,
            body: Body::Flow(self.steps),
        }
    }
}

/// A named group of tests — Playwright's `describe`.
pub struct Suite {
    name: String,
    tests: Vec<Test>,
}

/// Group tests under a suite name.
pub fn suite(name: &str, tests: Vec<Test>) -> Suite {
    Suite {
        name: name.to_string(),
        tests,
    }
}

struct Queued {
    suite: String,
    test: Test,
}

struct RunState {
    queue: Vec<Queued>,
    suite_count: usize,
    passed: std::cell::Cell<usize>,
    failed: std::cell::Cell<usize>,
    skipped: std::cell::RefCell<Vec<String>>,
    failures: std::cell::RefCell<Vec<String>>,
    /// Last suite name we logged a header for, so we print one header per
    /// suite as the flat queue crosses suite boundaries.
    last_suite: std::cell::RefCell<Option<String>>,
}

/// Run a single suite (back-compat convenience). Equivalent to
/// `run_suites(vec![suite(name, tests)])`.
pub fn run(name: &str, tests: Vec<Test>) {
    run_suites(vec![suite(name, tests)]);
}

/// Run several suites **sequentially**, paced so each test's UI changes are
/// visible on screen and its log lines stream one test at a time. Returns
/// immediately; the run advances on the framework scheduler and emits a
/// final `[E2E-RESULT]` machine-readable line when every suite is done.
///
/// Must be called on the UI thread, after the first render (so the registry
/// is populated) — see the `after_ms_detached` deferral in the app's entry.
pub fn run_suites(suites: Vec<Suite>) {
    let suite_count = suites.len();
    let mut queue = Vec::new();
    for s in suites {
        for t in s.tests {
            queue.push(Queued {
                suite: s.name.clone(),
                test: t,
            });
        }
    }
    log(format!(
        "▶ running {suite_count} suite(s), {} test(s) total",
        queue.len()
    ));
    let state = std::rc::Rc::new(RunState {
        queue,
        suite_count,
        passed: std::cell::Cell::new(0),
        failed: std::cell::Cell::new(0),
        skipped: std::cell::RefCell::new(Vec::new()),
        failures: std::cell::RefCell::new(Vec::new()),
        last_suite: std::cell::RefCell::new(None),
    });
    run_one(state, 0);
}

/// Between-step delay for flow tests, ms. Any real-timer delay yields a
/// macrotask to the event loop, which is what lets async UI work (browser
/// `popstate` after a stack pop, a deferred layout pass) complete before the
/// next step's assertion. Kept small so flows stay snappy.
const FLOW_STEP_MS: i32 = 60;
/// Poll retry interval for `poll` steps, ms.
const POLL_INTERVAL_MS: i32 = 60;
/// Max `poll` attempts before the step's last `Err` fails the test
/// (≈ `POLL_INTERVAL_MS * POLL_MAX_ATTEMPTS` of real wall-clock headroom).
const POLL_MAX_ATTEMPTS: usize = 25;

fn run_one(state: std::rc::Rc<RunState>, i: usize) {
    if i >= state.queue.len() {
        emit_summary(&state);
        return;
    }

    let q = &state.queue[i];

    // Print a suite header when we cross into a new suite.
    {
        let mut last = state.last_suite.borrow_mut();
        if last.as_deref() != Some(q.suite.as_str()) {
            log(format!("▶ suite: {}", q.suite));
            *last = Some(q.suite.clone());
        }
    }

    log(format!(
        "• test ({}/{}): {}",
        i + 1,
        state.queue.len(),
        q.test.name
    ));

    match &q.test.body {
        Body::Sync(body) => {
            let page = Page::new();
            match body(&page) {
                Ok(()) => record_pass(&state, i),
                Err(e) => record_fail(&state, i, &e),
            }
            advance(state, i);
        }
        // Flow tests drive themselves across real ticks, then advance.
        Body::Flow(_) => drive_flow(state, i, 0, 0),
    }
}

/// Drive step `step_idx` of flow test `i` on its `attempt`-th try. Each
/// transition reschedules on the real scheduler (`after_ms_detached`), so the
/// event loop turns between steps — async UI work settles in the gap.
fn drive_flow(state: std::rc::Rc<RunState>, i: usize, step_idx: usize, attempt: usize) {
    let steps = match &state.queue[i].test.body {
        Body::Flow(s) => s,
        // Unreachable: `run_one` only routes `Body::Flow` here.
        Body::Sync(_) => {
            advance(state, i);
            return;
        }
    };

    if step_idx >= steps.len() {
        record_pass(&state, i);
        advance(state, i);
        return;
    }

    let step = &steps[step_idx];
    let page = Page::new();
    let res = (step.run)(&page);

    match (&step.kind, res) {
        // A passed step (action or assertion) → next step after a real tick.
        (_, Ok(())) => schedule_step(state, i, step_idx + 1, 0, FLOW_STEP_MS),
        // A once-step failure ends the test.
        (StepKind::Once, Err(e)) => {
            record_fail(&state, i, &e);
            advance(state, i);
        }
        // A skip ends the test at once, poll or not: retrying cannot
        // make the platform grow the feature.
        (StepKind::Poll, Err(e)) if skip_reason(&e).is_some() => {
            record_fail(&state, i, &e);
            advance(state, i);
        }
        // A poll-step failure retries until the budget is spent.
        (StepKind::Poll, Err(e)) => {
            if attempt + 1 < POLL_MAX_ATTEMPTS {
                schedule_step(state, i, step_idx, attempt + 1, POLL_INTERVAL_MS);
            } else {
                record_fail(&state, i, &format!("timed out waiting: {e}"));
                advance(state, i);
            }
        }
    }
}

fn schedule_step(
    state: std::rc::Rc<RunState>,
    i: usize,
    step_idx: usize,
    attempt: usize,
    delay_ms: i32,
) {
    runtime_shared::after_ms_detached(delay_ms, move || drive_flow(state, i, step_idx, attempt));
}

fn record_pass(state: &RunState, i: usize) {
    let q = &state.queue[i];
    state.passed.set(state.passed.get() + 1);
    log(format!("  ✅ PASS: {}", q.test.name));
    report(TestResult { suite: q.suite.clone(), test: q.test.name.clone(), status: Status::Pass, detail: None });
}

/// A failed test — or a skipped one, when `e` is a [`skip`].
fn record_fail(state: &RunState, i: usize, e: &str) {
    let q = &state.queue[i];
    if let Some(reason) = skip_reason(e) {
        state.skipped.borrow_mut().push(format!("{}/{}: {}", q.suite, q.test.name, reason));
        log(format!("  ⏭ SKIP: {} — {reason}", q.test.name));
        report(TestResult {
            suite: q.suite.clone(),
            test: q.test.name.clone(),
            status: Status::Skip,
            detail: Some(reason.to_string()),
        });
        return;
    }
    state.failed.set(state.failed.get() + 1);
    state
        .failures
        .borrow_mut()
        .push(format!("{}/{}: {}", q.suite, q.test.name, e));
    log_fail(format!("  ❌ FAIL: {} — {e}", q.test.name));
    report(TestResult {
        suite: q.suite.clone(),
        test: q.test.name.clone(),
        status: Status::Fail,
        detail: Some(e.to_string()),
    });
}

/// Advance to the next test on the scheduler. `after_ms_detached` (vs
/// `after_ms`) is deliberate: the handle of `after_ms` would cancel on drop
/// at the end of this function, killing the loop before the next test fires —
/// the classic scheduled-handle pitfall.
fn advance(state: std::rc::Rc<RunState>, i: usize) {
    runtime_shared::after_ms_detached(STEP_PACING_MS, move || run_one(state, i + 1));
}

fn emit_summary(state: &RunState) {
    let (p, f) = (state.passed.get(), state.failed.get());
    let skipped = state.skipped.borrow();
    let s = skipped.len();
    let total = p + f + s;
    let verdict = if f == 0 { "✅ all green" } else { "❌ failures" };
    log(format!(
        "■ done — {verdict}: {p}/{total} passed, {s} skipped, across {} suite(s)",
        state.suite_count
    ));
    let failures = state.failures.borrow();
    for fail in failures.iter() {
        log_fail(format!("  • {fail}"));
    }
    // One machine-readable line for the cross-platform orchestrator to
    // scrape from each target's log channel.
    let failures_json = failures
        .iter()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .collect::<Vec<_>>()
        .join(",");
    let skipped_json = skipped
        .iter()
        .map(|s| format!("\"{}\"", json_escape(s)))
        .collect::<Vec<_>>()
        .join(",");
    runtime_shared::log_info!(
        "[E2E-RESULT] {{\"suites\":{},\"tests\":{},\"pass\":{},\"fail\":{},\"skip\":{},\"failures\":[{}],\"skipped\":[{}]}}",
        state.suite_count,
        total,
        p,
        f,
        s,
        failures_json,
        skipped_json
    );
}

/// Minimal JSON string escaping — enough for failure messages, which are
/// developer-authored and never contain control bytes beyond newlines.
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('\t', " ")
}

/// Whether `el` can be seen (`Robot::visibility`: shown by the toolkit, a
/// non-empty box); an element gone since it was found is not.
fn visible(el: &Element) -> bool {
    Robot::new().visibility(el).is_ok_and(|v| v.visible)
}
